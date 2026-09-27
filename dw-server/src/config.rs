use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, ToSocketAddrs};

const DEFAULT_CONTENT_PORT: u16 = 3076;
const DEFAULT_HOSTNAME: &str = "localhost";

#[derive(Serialize, Deserialize, Default)]
pub struct DwServerConfig {
    content_port: Option<u16>,
    /// The hostname under which the server can be reached
    hostname: Option<String>,
    /// The IPv4 address clients reach the NAT endpoint at, when it is not
    /// what the hostname resolves to
    public_ip: Option<Ipv4Addr>,
}

impl DwServerConfig {
    pub fn content_port(&self) -> u16 {
        self.content_port.unwrap_or(DEFAULT_CONTENT_PORT)
    }

    pub fn hostname(&self) -> &str {
        self.hostname.as_deref().unwrap_or(DEFAULT_HOSTNAME)
    }

    pub fn public_ip(&self) -> Option<Ipv4Addr> {
        self.public_ip.or_else(|| {
            (self.hostname(), 0)
                .to_socket_addrs()
                .ok()?
                .find_map(|a| match a.ip() {
                    std::net::IpAddr::V4(ip) => Some(ip),
                    _ => None,
                })
        })
    }
}
