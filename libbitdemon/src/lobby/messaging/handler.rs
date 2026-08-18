use crate::lobby::LobbyHandler;
use crate::lobby::messaging::router::MessageRouter;
use crate::lobby::response::task_reply::TaskReply;
use crate::messaging::BdErrorCode;
use crate::messaging::bd_message::BdMessage;
use crate::messaging::bd_reader::BdReader;
use crate::messaging::bd_response::{BdResponse, ResponseCreator};
use crate::networking::bd_session::BdSession;
use log::{trace, warn};
use num_traits::FromPrimitive;
use std::error::Error;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Eq, PartialEq, Hash, Copy, Clone, FromPrimitive, ToPrimitive)]
#[repr(u8)]
enum MessagingTaskId {
    SendMessage = 8,
}

const RECIPIENT_BITS: usize = 5 + 64;

pub struct MessagingHandler {
    router: Arc<MessageRouter>,
}

impl LobbyHandler for MessagingHandler {
    fn handle_message(
        &self,
        session: &mut BdSession,
        mut message: BdMessage,
    ) -> Result<BdResponse, Box<dyn Error>> {
        let task_id_value = message.reader.read_u8()?;

        let unrecovered = message.reader.read_u8()?;
        if unrecovered != 0 {
            trace!("Messaging request leading byte is {unrecovered}, not the usual 0");
        }

        let Some(task_id) = MessagingTaskId::from_u8(task_id_value) else {
            warn!("Client called unknown messaging task {task_id_value}");
            return TaskReply::with_only_error_code(BdErrorCode::ServiceNotAvailable, task_id_value)
                .to_response();
        };

        match task_id {
            MessagingTaskId::SendMessage => self.send_message(session, &mut message.reader),
        }
    }
}

impl MessagingHandler {
    pub fn new(router: Arc<MessageRouter>) -> MessagingHandler {
        MessagingHandler { router }
    }

    fn send_message(
        &self,
        session: &BdSession,
        reader: &mut BdReader,
    ) -> Result<BdResponse, Box<dyn Error>> {
        let payload = reader.read_blob()?;
        let unrecovered = reader.read_u32()?;
        let single = reader.read_bool()?;

        let mut recipients = Vec::new();

        if single {
            recipients.push(reader.read_u64()?);
        } else {
            while reader.remaining_bits()? >= RECIPIENT_BITS && reader.next_is_u64()? {
                recipients.push(reader.read_u64()?);
            }
        }

        let auth = session.authentication().unwrap();

        trace!(
            "Send {} bytes from {} to {:?} (single {single}, unrecovered {unrecovered})",
            payload.len(),
            auth.user_id,
            recipients
        );

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(0);

        for recipient in &recipients {
            self.router.deliver(
                *recipient,
                auth.user_id,
                auth.username.as_str(),
                now,
                payload.as_slice(),
            );
        }

        TaskReply::with_only_error_code(BdErrorCode::NoError, MessagingTaskId::SendMessage as u8)
            .to_response()
    }
}
