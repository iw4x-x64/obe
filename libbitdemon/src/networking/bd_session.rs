use crate::auth::authentication::SessionAuthentication;
use std::io;
use std::io::{BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};

pub type SessionId = u64;

pub type SessionWriter = Arc<Mutex<TcpStream>>;

pub struct BdSession {
    pub id: SessionId,
    authentication: Option<SessionAuthentication>,
    stream: BufReader<TcpStream>,
    writer: SessionWriter,
}

impl io::Read for BdSession {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.read(buf)
    }
}

impl io::Write for BdSession {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.lock().unwrap().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.lock().unwrap().flush()
    }
}

impl BdSession {
    pub fn new(stream: TcpStream) -> io::Result<Self> {
        let writer = Arc::new(Mutex::new(stream.try_clone()?));
        let reader = BufReader::new(stream);

        Ok(BdSession {
            id: 0,
            authentication: None,
            stream: reader,
            writer,
        })
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.stream.get_ref().peer_addr()
    }

    pub fn writer(&self) -> SessionWriter {
        self.writer.clone()
    }

    pub fn send_frame(&self, frame: &[u8]) -> io::Result<()> {
        self.writer.lock().unwrap().write_all(frame)
    }

    pub fn authentication(&self) -> Option<&SessionAuthentication> {
        self.authentication.as_ref()
    }

    pub fn set_authentication(&mut self, authentication: SessionAuthentication) {
        debug_assert!(self.authentication.is_none());
        self.authentication = Some(authentication);
    }
}
