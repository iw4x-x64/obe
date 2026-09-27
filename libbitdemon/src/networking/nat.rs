//! The UDP part of bdSocket's NAT support. It answers public address and NAT
//! type discovery and acts as the introducer for peers behind a strict NAT.
//!
//! The client resolves the stun.* and mw2-stun.* hosts and sends all of these
//! requests to port 3074 there from its game socket. The introducer has to be
//! the discovery server. The public address a client advertises is its
//! mapping towards us, and a strict NAT only admits packets that come from
//! here. So we send relayed introductions from this same socket.
//!
//! Every packet starts with a type byte and a u16 version (2). An address is
//! the four bytes of the IPv4 address in network order followed by the port
//! as a little-endian u16.

use log::{debug, info, warn};
use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const NAT_PRIMARY_PORT: u16 = 3074;
pub const NAT_SECONDARY_PORT: u16 = 3078;
pub const NAT_ALTERNATE_PORT: u16 = 3079;

const VERSION: u16 = 2;

const INTRO_NAT_REQUEST: u8 = 10;
const INTRO_NAT_RELAY: u8 = 11;
const INTRO_KEEP_ALIVE: u8 = 14;
const NAT_TYPE_REQUEST: u8 = 20;
const NAT_TYPE_REPLY: u8 = 21;
const IP_DISCOVERY_REQUEST: u8 = 30;
const IP_DISCOVERY_REPLY: u8 = 31;

const NAT_TYPE_TEST_1: u8 = 0;
const NAT_TYPE_TEST_3: u8 = 2;
const NAT_TYPE_TEST_2: u8 = 3;

// type, version, a 10 byte HMAC, a u32 identifier, then the address of the
// peer to introduce and the address of the peer asking.
const INTRO_PACKET_SIZE: usize = 1 + 2 + 10 + 4 + ADDR_SIZE * 2;
const INTRO_TARGET_OFFSET: usize = 1 + 2 + 10 + 4;

const ADDR_SIZE: usize = 6;

// How long we keep relaying to an address after we last heard from it.
// Clients keep their introducer mapping alive with type 14 packets and any
// packet from a client refreshes it. We only relay to addresses that talked
// to us recently, which keeps the introducer from reflecting packets at
// arbitrary hosts.
const PEER_LIFETIME: Duration = Duration::from_secs(10 * 60);

pub struct NatServer {
    primary: UdpSocket,
    secondary: UdpSocket,
    alternate: UdpSocket,
    // Our public address on the secondary port. Test 1 tells the client to
    // send test 3 here, and the client checks the test 2 reply against it.
    secondary_public: SocketAddrV4,
    peers: Mutex<HashMap<SocketAddrV4, Instant>>,
}

impl NatServer {
    pub fn bind(public_ip: Ipv4Addr) -> io::Result<NatServer> {
        let primary = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, NAT_PRIMARY_PORT))?;
        let secondary = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, NAT_SECONDARY_PORT))?;
        let alternate = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, NAT_ALTERNATE_PORT))?;

        info!(
            "NAT endpoint on udp/{NAT_PRIMARY_PORT}, udp/{NAT_SECONDARY_PORT} and \
             udp/{NAT_ALTERNATE_PORT} (public address {public_ip})"
        );

        Ok(NatServer {
            primary,
            secondary,
            alternate,
            secondary_public: SocketAddrV4::new(public_ip, NAT_SECONDARY_PORT),
            peers: Mutex::new(HashMap::new()),
        })
    }

    pub fn run(self: Arc<Self>) {
        let primary = self.clone();
        thread::spawn(move || primary.serve(&primary.primary, Self::handle_primary));

        thread::spawn(move || self.serve(&self.secondary, Self::handle_secondary));
    }

    fn serve(&self, socket: &UdpSocket, handle: fn(&Self, &[u8], SocketAddrV4)) {
        let mut buffer = [0u8; 1500];

        loop {
            match socket.recv_from(&mut buffer) {
                Ok((n, SocketAddr::V4(from))) => handle(self, &buffer[..n], from),
                Ok(_) => {}
                // A previous send was refused by the peer (ICMP port
                // unreachable), which Windows and some Linux setups report on
                // the next receive. We ignore it.
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {}
                Err(e) => {
                    warn!("NAT endpoint stopped: {e}");
                    return;
                }
            }
        }
    }

    fn handle_primary(&self, packet: &[u8], from: SocketAddrV4) {
        let Some(&kind) = packet.first() else {
            return;
        };

        self.seen(from);

        match kind {
            IP_DISCOVERY_REQUEST => {
                let mut reply = header(IP_DISCOVERY_REPLY);
                write_addr(&mut reply, from);

                send(&self.primary, &reply, from);
            }
            NAT_TYPE_REQUEST => match packet.get(3) {
                Some(&NAT_TYPE_TEST_1) => {
                    send(&self.primary, &self.nat_type_reply(from), from);
                }
                // Sent from a port the client has not sent anything to, so it
                // only arrives through a NAT that lets anything in.
                Some(&NAT_TYPE_TEST_2) => {
                    send(&self.alternate, &self.nat_type_reply(from), from);
                }
                test => debug!("NAT type test {test:?} from {from} on the primary port"),
            },
            INTRO_NAT_REQUEST => self.relay(packet, from),
            INTRO_KEEP_ALIVE => {}
            _ => debug!("Unknown NAT packet type {kind} from {from}"),
        }
    }

    fn handle_secondary(&self, packet: &[u8], from: SocketAddrV4) {
        // Test 3 is answered from where it was sent. The client compares the
        // address we saw with the one test 1 saw. If the NAT picked a new
        // mapping for the new destination port, then it is strict.
        if packet.first() == Some(&NAT_TYPE_REQUEST) && packet.get(3) == Some(&NAT_TYPE_TEST_3) {
            send(&self.secondary, &self.nat_type_reply(from), from);
        }
    }

    fn nat_type_reply(&self, from: SocketAddrV4) -> Vec<u8> {
        let mut reply = header(NAT_TYPE_REPLY);
        write_addr(&mut reply, from);
        write_addr(&mut reply, self.secondary_public);

        reply
    }

    // Stage 2 of a traversal. The asking peer could not reach the target
    // directly, so we tell the target to send to the asking peer, which opens
    // the target's NAT on the way out. The target answers the asking peer
    // itself and only the asking peer checks the HMAC. So we forward the
    // packet unchanged except for its type.
    fn relay(&self, packet: &[u8], from: SocketAddrV4) {
        if packet.len() < INTRO_PACKET_SIZE {
            debug!("Short introduction request from {from}");
            return;
        }

        let target = read_addr(&packet[INTRO_TARGET_OFFSET..]);

        if !self.is_live(target) {
            debug!("Introduction from {from} to {target}, which we have not heard from");
            return;
        }

        let mut relayed = packet.to_vec();
        relayed[0] = INTRO_NAT_RELAY;

        debug!("Introducing {from} to {target}");
        send(&self.primary, &relayed, target);
    }

    fn seen(&self, addr: SocketAddrV4) {
        let now = Instant::now();
        let mut peers = self.peers.lock().unwrap();

        peers.insert(addr, now);

        // Every 1024 entries, drop the ones that have gone quiet.
        if peers.len() % 1024 == 0 {
            peers.retain(|_, last| now.duration_since(*last) < PEER_LIFETIME);
        }
    }

    fn is_live(&self, addr: SocketAddrV4) -> bool {
        self.peers
            .lock()
            .unwrap()
            .get(&addr)
            .is_some_and(|last| last.elapsed() < PEER_LIFETIME)
    }
}

fn send(socket: &UdpSocket, packet: &[u8], to: SocketAddrV4) {
    if let Err(e) = socket.send_to(packet, to) {
        debug!("Failed to send NAT packet to {to}: {e}");
    }
}

fn header(kind: u8) -> Vec<u8> {
    let mut packet = vec![kind];
    packet.extend_from_slice(&VERSION.to_le_bytes());

    packet
}

fn write_addr(packet: &mut Vec<u8>, addr: SocketAddrV4) {
    packet.extend_from_slice(&addr.ip().octets());
    packet.extend_from_slice(&addr.port().to_le_bytes());
}

fn read_addr(bytes: &[u8]) -> SocketAddrV4 {
    let ip = Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]);
    let port = u16::from_le_bytes([bytes[4], bytes[5]]);

    SocketAddrV4::new(ip, port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_round_trips() {
        let addr = SocketAddrV4::new(Ipv4Addr::new(104, 28, 232, 200), 22192);
        let mut packet = Vec::new();

        write_addr(&mut packet, addr);

        assert_eq!(packet, [104, 28, 232, 200, 0xB0, 0x56]);
        assert_eq!(read_addr(&packet), addr);
    }

    #[test]
    fn nat_type_reply_carries_both_addresses() {
        let server = NatServer {
            primary: UdpSocket::bind("127.0.0.1:0").unwrap(),
            secondary: UdpSocket::bind("127.0.0.1:0").unwrap(),
            alternate: UdpSocket::bind("127.0.0.1:0").unwrap(),
            secondary_public: SocketAddrV4::new(Ipv4Addr::new(147, 135, 10, 99), 3078),
            peers: Mutex::new(HashMap::new()),
        };

        let from = SocketAddrV4::new(Ipv4Addr::new(62, 213, 14, 153), 3074);
        let reply = server.nat_type_reply(from);

        assert_eq!(reply[0], NAT_TYPE_REPLY);
        assert_eq!(u16::from_le_bytes([reply[1], reply[2]]), 2);
        assert_eq!(read_addr(&reply[3..]), from);
        assert_eq!(read_addr(&reply[9..]), server.secondary_public);
    }

    #[test]
    fn relays_only_to_known_peers() {
        let target = UdpSocket::bind("127.0.0.1:0").unwrap();
        target.set_read_timeout(Some(Duration::from_secs(1))).unwrap();

        let SocketAddr::V4(target_addr) = target.local_addr().unwrap() else {
            unreachable!()
        };

        let server = NatServer {
            primary: UdpSocket::bind("127.0.0.1:0").unwrap(),
            secondary: UdpSocket::bind("127.0.0.1:0").unwrap(),
            alternate: UdpSocket::bind("127.0.0.1:0").unwrap(),
            secondary_public: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 3078),
            peers: Mutex::new(HashMap::new()),
        };

        let requester = SocketAddrV4::new(Ipv4Addr::new(62, 213, 14, 153), 3074);

        let mut request = header(INTRO_NAT_REQUEST);
        request.extend_from_slice(&[0xAA; 10]);
        request.extend_from_slice(&7u32.to_le_bytes());
        write_addr(&mut request, target_addr);
        write_addr(&mut request, requester);

        // We have not heard from the target, so nothing is sent.
        server.relay(&request, requester);
        target.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        assert!(target.recv_from(&mut [0u8; 64]).is_err());

        // After a keep-alive from the target, the request is relayed.
        server.seen(target_addr);
        server.relay(&request, requester);

        target.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let mut buffer = [0u8; 64];
        let (n, _) = target.recv_from(&mut buffer).unwrap();

        assert_eq!(n, INTRO_PACKET_SIZE);
        assert_eq!(buffer[0], INTRO_NAT_RELAY);
        assert_eq!(&buffer[1..n], &request[1..]);
    }
}
