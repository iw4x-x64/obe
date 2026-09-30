use crate::messaging::bd_message::BdMessage;
use crate::networking::bd_session::BdSession;
use crate::networking::session_manager::SessionManager;
use byteorder::{LittleEndian, ReadBytesExt};
use log::{debug, error, info};
use snafu::{Snafu, ensure};
use std::error::Error;
use std::io::{ErrorKind, Read};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use std::{io, thread};

const MAX_MESSAGE_SIZE: u32 = 0x4000000;

const IDLE_TIMEOUT: Duration = Duration::from_secs(180);

const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

const BUFFER_SPACE_HEADER: u32 = 180;

#[derive(Debug, Snafu)]
enum BdSocketError {
    #[snafu(display("Message was too large (size={msg_size}, max={MAX_MESSAGE_SIZE})"))]
    MessageTooLargeError { msg_size: u32 },
    #[snafu(display("The client sent an incomplete message header"))]
    IncompleteMessageHeaderError {},
}

struct Registered<'a> {
    session: BdSession,
    session_manager: &'a SessionManager,
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        self.session_manager.unregister_session(&self.session);
    }
}

pub trait BdMessageHandler {
    fn handle_message(
        &self,
        session: &mut BdSession,
        message: BdMessage,
    ) -> Result<(), Box<dyn Error>>;
}

pub struct BdSocket {
    session_manager: Arc<SessionManager>,
    listener: Option<TcpListener>,
    idle_timeout: Duration,
}

impl BdSocket {
    /// Creates a new BdSocket instance and binds it to the specified port.
    pub fn new(port: u16) -> Result<BdSocket, io::Error> {
        Self::new_with_session_manager(port, Arc::new(SessionManager::new()))
    }

    /// Creates a new BdSocket instance and binds it to the specified port.
    pub fn new_with_session_manager(
        port: u16,
        session_manager: Arc<SessionManager>,
    ) -> Result<BdSocket, io::Error> {
        let listener = TcpListener::bind(format!("0.0.0.0:{port}"))?;

        info!("Opened bitdemon socket on port {port}");

        Ok(BdSocket {
            listener: Some(listener),
            session_manager,
            idle_timeout: IDLE_TIMEOUT,
        })
    }

    pub fn with_idle_timeout(mut self, idle_timeout: Duration) -> BdSocket {
        self.idle_timeout = idle_timeout;
        self
    }

    pub fn local_addr(&self) -> io::Result<std::net::SocketAddr> {
        self.listener.as_ref().unwrap().local_addr()
    }

    fn listen(
        listener: &TcpListener,
        session_manager: &Arc<SessionManager>,
        message_handler: Arc<dyn BdMessageHandler + Send + Sync>,
        idle_timeout: Duration,
    ) -> Result<(), io::Error> {
        for stream in listener.incoming() {
            let stream = stream?;

            let session_manager = Arc::clone(session_manager);
            let message_handler = Arc::clone(&message_handler);
            thread::spawn(move || {
                if let Err(e) = stream
                    .set_read_timeout(Some(idle_timeout))
                    .and_then(|()| stream.set_write_timeout(Some(WRITE_TIMEOUT)))
                {
                    error!("Could not set socket timeouts: {e}");
                    return;
                }

                let mut session = match BdSession::new(stream) {
                    Ok(session) => session,
                    Err(e) => {
                        error!("Could not set up a session: {e}");
                        return;
                    }
                };

                session_manager.register_session(&mut session);

                let mut session = Registered {
                    session,
                    session_manager: &session_manager,
                };

                BdSocket::handle_connection(&mut session.session, message_handler.as_ref());
            });
        }

        Ok(())
    }

    pub fn run_sync(
        &mut self,
        message_handler: Arc<dyn BdMessageHandler + Send + Sync>,
    ) -> Result<(), io::Error> {
        Self::listen(
            self.listener.as_ref().unwrap(),
            &self.session_manager,
            message_handler,
            self.idle_timeout,
        )
    }

    pub fn run_async(
        &mut self,
        message_handler: Arc<dyn BdMessageHandler + Send + Sync>,
    ) -> JoinHandle<Result<(), io::Error>> {
        let message_handler = Arc::clone(&message_handler);
        let listener = self.listener.take();
        let session_manager = self.session_manager.clone();
        let idle_timeout = self.idle_timeout;
        thread::spawn(move || -> Result<(), io::Error> {
            let session_manager = session_manager;
            Self::listen(
                listener.as_ref().unwrap(),
                &session_manager,
                message_handler,
                idle_timeout,
            )
        })
    }

    fn sniff_connection(session: &mut BdSession) {
        let mut all: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];

        loop {
            match session.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    all.extend_from_slice(&buf[..n]);
                    info!("Sniffed {n} bytes (total {})", all.len());
                    info!("  {:02x?}", &all[..all.len().min(512)]);
                }
            }
        }
    }

    fn handle_connection(session: &mut BdSession, message_handler: &dyn BdMessageHandler) {
        if std::env::var("IW4X_SNIFF").is_ok() {
            Self::sniff_connection(session);
            return;
        }

        let connection_loop = |session: &mut BdSession| -> Result<(), Box<dyn Error>> {
            loop {
                let mut b: [u8; 4] = [0; 4];
                if session.read(&mut b[..1])? == 0 {
                    return Ok(());
                }

                session
                    .read_exact(&mut b[1..])
                    .map_err(|_| IncompleteMessageHeaderSnafu {}.build())?;
                let header = u32::from_le_bytes(b);

                match header {
                    0 => {
                        debug!("Ping");
                        session.send_frame(&0u32.to_le_bytes())?;
                    }
                    BUFFER_SPACE_HEADER => {
                        let available_buffer_size = session.read_u32::<LittleEndian>()?;
                        debug!("Buffer available: {available_buffer_size}");
                    }
                    _ => {
                        ensure!(
                            header <= MAX_MESSAGE_SIZE,
                            MessageTooLargeSnafu { msg_size: header }
                        );

                        debug!("Message with size {header}");
                        let mut msg = vec![0; header as usize];
                        session.read_exact(msg.as_mut_slice())?;
                        let message = BdMessage::new(session, msg)?;
                        message_handler.handle_message(session, message)?;
                    }
                }
            }
        };

        let connection_result = connection_loop(session);
        if let Err(e) = connection_result {
            if let Some(e0) = e.downcast_ref::<io::Error>() {
                match e0.kind() {
                    ErrorKind::Interrupted | ErrorKind::ConnectionReset => {}
                    ErrorKind::WouldBlock | ErrorKind::TimedOut => {
                        info!("Connection went silent; closing")
                    }
                    _ => error!("Connection terminated: {}: {e}", e0.kind()),
                }
            } else {
                error!("Session terminated with error: {e}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpStream;
    use std::sync::mpsc;
    use std::time::Instant;

    struct Ignore;

    impl BdMessageHandler for Ignore {
        fn handle_message(&self, _: &mut BdSession, _: BdMessage) -> Result<(), Box<dyn Error>> {
            Ok(())
        }
    }

    fn listening(idle_timeout: Duration) -> (TcpStream, mpsc::Receiver<()>) {
        let session_manager = Arc::new(SessionManager::new());
        let (ended, on_end) = mpsc::channel();
        session_manager.on_session_unregistered(move |_| {
            let _ = ended.send(());
        });

        let mut socket = BdSocket::new_with_session_manager(0, session_manager)
            .unwrap()
            .with_idle_timeout(idle_timeout);
        let port = socket.local_addr().unwrap().port();
        socket.run_async(Arc::new(Ignore));

        (TcpStream::connect(("127.0.0.1", port)).unwrap(), on_end)
    }

    #[test]
    fn a_silent_connection_is_dropped() {
        let (_client, on_end) = listening(Duration::from_millis(200));

        assert!(on_end.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn keepalives_hold_a_connection_open() {
        let (mut client, on_end) = listening(Duration::from_millis(300));

        let until = Instant::now() + Duration::from_millis(900);
        while Instant::now() < until {
            client.write_all(&0u32.to_le_bytes()).unwrap();
            thread::sleep(Duration::from_millis(100));
        }

        assert!(on_end.try_recv().is_err());
    }
}
