use crate::messaging::bd_message::BdMessage;
use crate::networking::bd_session::BdSession;
use crate::networking::session_manager::SessionManager;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use log::{debug, error, info};
use snafu::{Snafu, ensure};
use std::error::Error;
use std::io::{ErrorKind, Read};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::{io, thread};

const MAX_MESSAGE_SIZE: u32 = 0x4000000;

const BUFFER_SPACE_HEADER: u32 = 180;

#[derive(Debug, Snafu)]
enum BdSocketError {
    #[snafu(display("Message was too large (size={msg_size}, max={MAX_MESSAGE_SIZE})"))]
    MessageTooLargeError { msg_size: u32 },
    #[snafu(display("The client sent an incomplete message header"))]
    IncompleteMessageHeaderError {},
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
        })
    }

    fn listen(
        listener: &TcpListener,
        session_manager: &Arc<SessionManager>,
        message_handler: Arc<dyn BdMessageHandler + Send + Sync>,
    ) -> Result<(), io::Error> {
        for stream in listener.incoming() {
            let stream = stream?;

            let session_manager = Arc::clone(session_manager);
            let message_handler = Arc::clone(&message_handler);
            thread::spawn(move || {
                let mut session = BdSession::new(stream);
                session_manager.register_session(&mut session);
                BdSocket::handle_connection(&mut session, message_handler.as_ref());
                session_manager.unregister_session(&session);
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
        )
    }

    pub fn run_async(
        &mut self,
        message_handler: Arc<dyn BdMessageHandler + Send + Sync>,
    ) -> JoinHandle<Result<(), io::Error>> {
        let message_handler = Arc::clone(&message_handler);
        let listener = self.listener.take();
        let session_manager = self.session_manager.clone();
        thread::spawn(move || -> Result<(), io::Error> {
            let session_manager = session_manager;
            Self::listen(
                listener.as_ref().unwrap(),
                &session_manager,
                message_handler,
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
                let len = session.read(&mut b)?;
                if len == 0 {
                    return Ok(());
                }

                ensure!(len == 4, IncompleteMessageHeaderSnafu {});
                let header = u32::from_le_bytes(b);

                match header {
                    0 => {
                        debug!("Ping");
                        session.write_u32::<LittleEndian>(0)?;
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
                    _ => error!("Connection terminated: {}: {e}", e0.kind()),
                }
            } else {
                error!("Session terminated with error: {e}")
            }
        }
    }
}
