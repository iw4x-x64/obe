use log::{debug, info, warn};
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::sync::Mutex;
use std::thread;

#[derive(Default, Clone, Copy)]
pub struct Measurement {
    pub packets: u64,
    pub bytes: u64,
}

pub struct BandwidthTestServer {
    socket: UdpSocket,
    port: u16,
    seen: Mutex<HashMap<IpAddr, Measurement>>,
}

impl BandwidthTestServer {
    pub fn bind(port: u16) -> io::Result<BandwidthTestServer> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port))?;
        let port = socket.local_addr()?.port();

        info!("Bandwidth test endpoint on udp/{port}");

        Ok(BandwidthTestServer {
            socket,
            port,
            seen: Mutex::new(HashMap::new()),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn run(self: std::sync::Arc<Self>) {
        thread::spawn(move || {
            let mut buffer = [0u8; 2048];

            loop {
                match self.socket.recv_from(&mut buffer) {
                    Ok((n, from)) => {
                        let mut seen = self.seen.lock().unwrap();
                        let m = seen.entry(from.ip()).or_default();

                        m.packets += 1;
                        m.bytes += n as u64;

                        if m.packets == 1 {
                            debug!("Bandwidth test traffic from {from}");
                        }
                    }
                    Err(e) => {
                        warn!("Bandwidth test endpoint stopped: {e}");
                        return;
                    }
                }
            }
        });
    }

    pub fn take(&self, who: IpAddr) -> Measurement {
        let mut seen = self.seen.lock().unwrap();

        seen.remove(&who).unwrap_or_default()
    }
}
