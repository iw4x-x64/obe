use crate::lobby::response::push_message::PushMessage;
use crate::messaging::bd_response::ResponseCreator;
use crate::networking::bd_session::{BdSession, SessionId, SessionWriter};
use log::{debug, warn};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

struct Recipient {
    session: SessionId,
    writer: SessionWriter,
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

        debug!(
            "Reachable for messages: user {} as '{}'",
            auth.user_id, auth.username
        );

        self.recipients.lock().unwrap().insert(
            auth.user_id,
            Recipient {
                session: session.id,
                writer: session.writer(),
                key: auth.session_key,
            },
        );
    }

    pub fn unregister(&self, session: &BdSession) {
        self.recipients
            .lock()
            .unwrap()
            .retain(|_, r| r.session != session.id);
    }

    pub fn online(&self) -> usize {
        self.recipients.lock().unwrap().len()
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

        let recipients = self.recipients.lock().unwrap();

        let Some(r) = recipients.get(&recipient) else {
            debug!("No session for user {recipient}; message dropped");
            return false;
        };

        match message
            .to_response()
            .and_then(|mut r2| r2.send_to(&mut *r.writer.lock().unwrap(), Some(&r.key)))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::authentication::SessionAuthentication;
    use crate::domain::title::Title;
    use std::net::{TcpListener, TcpStream};

    fn session(id: SessionId, user_id: Option<u64>) -> BdSession {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let mut session = BdSession::new(stream).unwrap();
        session.id = id;

        if let Some(user_id) = user_id {
            session.set_authentication(SessionAuthentication {
                user_id,
                username: format!("user{user_id}"),
                session_key: [0; 24],
                title: Title::Iw4,
            });
        }

        session
    }

    #[test]
    fn online_counts_signed_in_users() {
        let router = MessageRouter::new();
        router.register(&session(1, Some(10)));
        router.register(&session(2, Some(20)));
        router.register(&session(3, None));

        assert_eq!(router.online(), 2);
    }

    #[test]
    fn a_reconnect_counts_once_and_outlives_the_old_connection() {
        let router = MessageRouter::new();
        let old = session(1, Some(10));
        let new = session(2, Some(10));

        router.register(&old);
        router.register(&new);
        assert_eq!(router.online(), 1);

        router.unregister(&old);
        assert_eq!(router.online(), 1);

        router.unregister(&new);
        assert_eq!(router.online(), 0);
    }
}
