use crate::lobby::LobbyHandler;
use crate::lobby::response::task_reply::TaskReply;
use crate::messaging::BdErrorCode;
use crate::messaging::bd_message::BdMessage;
use crate::messaging::bd_reader::BdReader;
use crate::messaging::bd_response::{BdResponse, ResponseCreator};
use crate::networking::bd_session::BdSession;
use log::{trace, warn};
use num_traits::FromPrimitive;
use std::error::Error;

#[derive(Debug, Eq, PartialEq, Hash, Copy, Clone, FromPrimitive, ToPrimitive)]
#[repr(u8)]
enum PerformanceTaskId {
    ReportInfo = 1,
    ReportPlayers = 2,
}

struct PerformanceInfo {
    id: u64,
    value: i32,
}

const INFO_BITS: usize = 5 + 64 + 5 + 32;
const PLAYER_BITS: usize = 5 + 64;

pub struct PerformanceHandler {}

impl LobbyHandler for PerformanceHandler {
    fn handle_message(
        &self,
        _session: &mut BdSession,
        mut message: BdMessage,
    ) -> Result<BdResponse, Box<dyn Error>> {
        let task_id_value = message.reader.read_u8()?;

        let Some(task_id) = PerformanceTaskId::from_u8(task_id_value) else {
            warn!("Client called unknown performance task {task_id_value}");
            return TaskReply::with_only_error_code(BdErrorCode::ServiceNotAvailable, task_id_value)
                .to_response();
        };

        match task_id {
            PerformanceTaskId::ReportInfo => self.report_info(&mut message.reader),
            PerformanceTaskId::ReportPlayers => self.report_players(&mut message.reader),
        }
    }
}

impl PerformanceHandler {
    pub fn new() -> PerformanceHandler {
        PerformanceHandler {}
    }

    fn report_info(&self, reader: &mut BdReader) -> Result<BdResponse, Box<dyn Error>> {
        let context = reader.read_u32()?;

        let mut entries = Vec::new();
        while reader.remaining_bits()? >= INFO_BITS && reader.next_is_u64()? {
            let id = reader.read_u64()?;
            let value = reader.read_i32()?;

            entries.push(PerformanceInfo { id, value });
        }

        trace!(
            "Performance report for {context}: {}",
            entries
                .iter()
                .map(|e| format!("{}={}", e.id, e.value))
                .collect::<Vec<_>>()
                .join(", ")
        );

        TaskReply::with_only_error_code(BdErrorCode::NoError, PerformanceTaskId::ReportInfo as u8)
            .to_response()
    }

    fn report_players(&self, reader: &mut BdReader) -> Result<BdResponse, Box<dyn Error>> {
        let playlist = reader.read_u32()?;

        let mut players = Vec::new();
        while reader.remaining_bits()? >= PLAYER_BITS && reader.next_is_u64()? {
            players.push(reader.read_u64()?);
        }

        trace!("Playlist {playlist} has players {players:?}");

        TaskReply::with_results(PerformanceTaskId::ReportPlayers as u8, Vec::new()).to_response()
    }
}

impl Default for PerformanceHandler {
    fn default() -> Self {
        Self::new()
    }
}
