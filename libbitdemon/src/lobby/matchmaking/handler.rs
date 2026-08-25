use crate::lobby::LobbyHandler;
use crate::lobby::matchmaking::session::{
    MatchMakingInfo, SessionCreateResult, SessionQuery, SessionRegistry,
};
use crate::lobby::response::task_reply::TaskReply;
use crate::messaging::BdErrorCode;
use crate::messaging::bd_message::BdMessage;
use crate::messaging::bd_reader::BdReader;
use crate::messaging::bd_response::{BdResponse, ResponseCreator};
use crate::messaging::bd_serialization::BdSerialize;
use crate::networking::bd_session::BdSession;
use log::{trace, warn};
use num_traits::FromPrimitive;
use std::error::Error;
use std::sync::Arc;

#[derive(Debug, Eq, PartialEq, Hash, Copy, Clone, FromPrimitive, ToPrimitive)]
#[repr(u8)]
enum MatchMakingTaskId {
    CreateSession = 1,
    UpdateSession = 2,
    DeleteSession = 3,
    FindSessions = 5,
}

pub struct MatchMakingHandler {
    registry: Arc<SessionRegistry>,
}

impl LobbyHandler for MatchMakingHandler {
    fn handle_message(
        &self,
        session: &mut BdSession,
        mut message: BdMessage,
    ) -> Result<BdResponse, Box<dyn Error>> {
        let task_id_value = message.reader.read_u8()?;

        let unrecovered = message.reader.read_u8()?;
        if unrecovered != 0 {
            trace!("Session request leading byte is {unrecovered}, not the usual 0");
        }

        let Some(task_id) = MatchMakingTaskId::from_u8(task_id_value) else {
            warn!("Client called unknown session task {task_id_value}");
            return TaskReply::with_only_error_code(BdErrorCode::ServiceNotAvailable, task_id_value)
                .to_response();
        };

        match task_id {
            MatchMakingTaskId::CreateSession => self.create_session(session, &mut message.reader),
            MatchMakingTaskId::UpdateSession => self.update_session(session, &mut message.reader),
            MatchMakingTaskId::DeleteSession => self.delete_session(&mut message.reader),
            MatchMakingTaskId::FindSessions => self.find_sessions(&mut message.reader),
        }
    }
}

impl MatchMakingHandler {
    pub fn new(registry: Arc<SessionRegistry>) -> MatchMakingHandler {
        MatchMakingHandler { registry }
    }

    fn create_session(
        &self,
        session: &BdSession,
        reader: &mut BdReader,
    ) -> Result<BdResponse, Box<dyn Error>> {
        let info = read_advertisement(reader)?;

        let (id, secret) = self.registry.create(session.id, info);

        trace!("Created session {} on connection {}", hex(&id), session.id);

        let result: Box<dyn BdSerialize> = Box::new(SessionCreateResult {
            id: id.to_vec(),
            secret: secret.to_vec(),
        });

        TaskReply::with_results(MatchMakingTaskId::CreateSession as u8, vec![result]).to_response()
    }

    fn update_session(
        &self,
        session: &BdSession,
        reader: &mut BdReader,
    ) -> Result<BdResponse, Box<dyn Error>> {
        let info = read_advertisement(reader)?;

        trace!(
            "Updated session, {} of {} public slots used",
            info.used_public_slots,
            info.used_public_slots + info.free_public_slots
        );

        self.registry.update(session.id, info);

        TaskReply::with_only_error_code(BdErrorCode::NoError, MatchMakingTaskId::UpdateSession as u8)
            .to_response()
    }

    fn delete_session(&self, reader: &mut BdReader) -> Result<BdResponse, Box<dyn Error>> {
        let id = reader.read_blob()?;

        let removed = self.registry.delete(id.as_slice());
        trace!("Delete session, removed: {removed}");

        TaskReply::with_only_error_code(BdErrorCode::NoError, MatchMakingTaskId::DeleteSession as u8)
            .to_response()
    }

    fn find_sessions(&self, reader: &mut BdReader) -> Result<BdResponse, Box<dyn Error>> {
        let query = SessionQuery::deserialize(reader)?;
        read_trailer(reader)?;

        trace!(
            "Find sessions, query kind {} filters {:?}",
            query.kind, query.filters
        );

        let results: Vec<Box<dyn BdSerialize>> = self
            .registry
            .list()
            .into_iter()
            .map(|info| Box::new(info) as Box<dyn BdSerialize>)
            .collect();

        trace!("Answering with {} sessions", results.len());

        TaskReply::with_results(MatchMakingTaskId::FindSessions as u8, results).to_response()
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn read_advertisement(reader: &mut BdReader) -> Result<MatchMakingInfo, Box<dyn Error>> {
    let info = MatchMakingInfo::deserialize(reader)?;
    read_trailer(reader)?;

    Ok(info)
}

fn read_trailer(reader: &mut BdReader) -> Result<(), Box<dyn Error>> {
    let mut byte = [0u8; 1];

    for _ in 0..2 {
        reader.read_bits(&mut byte, 8)?;

        if byte[0] != 0 {
            trace!("Session request trailer byte is {}, not the usual 0", byte[0]);
        }
    }

    Ok(())
}
