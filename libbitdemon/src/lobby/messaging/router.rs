use crate::lobby::response::push_message::PushMessage;
use crate::messaging::bd_response::ResponseCreator;
use crate::networking::bd_session::{BdSession, SessionId};
use log::{debug, warn};
use std::collections::HashMap;
use std::net::TcpStream;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

struct Recipient {
    session: SessionId,
    stream: TcpStream,
    key: [u8; 24],
}

pub struct MessageRouter {
    recipients: Mutex<HashMap<u64, Recipient>>,
    next_id: AtomicU64,
}

impl MessageRouter {
    pub fn new() -> MessageRouter {
        MessageRouter {
            recipients: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn register(&self, session: &BdSession) {
        let Some(auth) = session.authentication() else {
            return;
        };

        match session.try_clone_stream() {
            Ok(stream) => {
                debug!(
                    "Reachable for messages: user {} as '{}'",
                    auth.user_id, auth.username
                );

                self.recipients.lock().unwrap().insert(
                    auth.user_id,
                    Recipient {
                        session: session.id,
                        stream,
                        key: auth.session_key,
                    },
                );
            }
            Err(e) => warn!("Cannot reach user {} for messages: {e}", auth.user_id),
        }
    }

    pub fn unregister(&self, session: &BdSession) {
        self.recipients
            .lock()
            .unwrap()
            .retain(|_, r| r.session != session.id);
    }

    pub fn deliver(
        &self,
        recipient: u64,
        sender: u64,
        sender_name: &str,
        timestamp: u32,
        payload: &[u8],
    ) -> bool {
        let message = PushMessage {
            recipient,
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            timestamp,
            sender,
            sender_name: sender_name.to_string(),
            payload: payload.to_vec(),
        };

        let mut recipients = self.recipients.lock().unwrap();

        let Some(r) = recipients.get_mut(&recipient) else {
            debug!("No session for user {recipient}; message dropped");
            return false;
        };

        match message
            .to_response()
            .and_then(|mut r2| r2.send_to(&mut r.stream, Some(&r.key)))
        {
            Ok(()) => {
                debug!("Delivered {} bytes to user {recipient}", payload.len());
                true
            }
            Err(e) => {
                warn!("Could not deliver to user {recipient}: {e}");
                false
            }
        }
    }
}

impl Default for MessageRouter {
    fn default() -> Self {
        Self::new()
    }
}
