use crate::lobby::{Framing, LobbyHandler};
use crate::lobby::bandwidth::result::{BandwidthTestAccepted, BandwidthTestRejected};
use crate::lobby::bandwidth::server::BandwidthTestServer;
use crate::lobby::response::lsg_reply::LsgResponseCreator;
use crate::lobby::response::task_reply::TaskReply;
use crate::messaging::BdErrorCode;
use crate::messaging::BdErrorCode::NoError;
use crate::messaging::bd_message::BdMessage;
use crate::messaging::bd_reader::BdReader;
use crate::messaging::bd_response::{BdResponse, ResponseCreator};
use crate::networking::bd_session::BdSession;
use log::{debug, warn};
use std::net::SocketAddr;
use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::FromPrimitive;
use std::error::Error;
use std::sync::Arc;

pub struct BandwidthHandler {
    endpoint: Arc<BandwidthTestServer>,
}

const PACKET_SIZE: u32 = 1024;
const PACKET_COUNT: u32 = 400;
const DURATION_MS: u32 = 2000;

#[derive(Debug, Eq, PartialEq, Hash, Copy, Clone, FromPrimitive, ToPrimitive)]
#[repr(u8)]
enum BandwidthTaskId {
    BandwidthTask = 1,
}

#[derive(Debug, Eq, PartialEq, Hash, Copy, Clone, FromPrimitive, ToPrimitive)]
#[repr(u8)]
enum BandwidthOperation {
    Start = 0,
    Finalize = 1,
}

#[derive(Debug, Eq, PartialEq, Hash, Copy, Clone, FromPrimitive, ToPrimitive)]
#[repr(u8)]
enum BandwidthTestType {
    UploadTest = 0,
    UploadDownloadTest = 1,
}

impl LobbyHandler for BandwidthHandler {
    fn framing(&self) -> Framing {
        Framing::Bytes
    }

    fn handle_message(
        &self,
        session: &mut BdSession,
        mut message: BdMessage,
    ) -> Result<BdResponse, Box<dyn Error>> {
        message.reader.set_type_checked(false);

        let task_id_value = message.reader.read_u8()?;
        let maybe_task_id = BandwidthTaskId::from_u8(task_id_value);
        if maybe_task_id.is_none() {
            warn!("Client called unknown task {task_id_value}");
            return TaskReply::with_only_error_code(NoError, task_id_value).to_response();
        }
        let task_id = maybe_task_id.unwrap();

        match task_id {
            BandwidthTaskId::BandwidthTask => {
                self.handle_bandwidth_task(session, &mut message.reader)
            }
        }
    }
}

impl BandwidthHandler {
    pub fn new(endpoint: Arc<BandwidthTestServer>) -> BandwidthHandler {
        BandwidthHandler { endpoint }
    }

    fn handle_bandwidth_task(
        &self,
        session: &mut BdSession,
        reader: &mut BdReader,
    ) -> Result<BdResponse, Box<dyn Error>> {
        let operation = reader.read_u8()?;

        match BandwidthOperation::from_u8(operation) {
            Some(BandwidthOperation::Start) => {
                let test_type_value = reader.read_u8()?;

                match BandwidthTestType::from_u8(test_type_value) {
                    Some(test_type) => debug!("Bandwidth test requested: {test_type:?}"),
                    None => warn!("Bandwidth test of unknown type {test_type_value}"),
                }

                let address = match session.peer_addr() {
                    Ok(SocketAddr::V4(a)) => u32::from_le_bytes(a.ip().octets()),
                    _ => {
                        warn!("Bandwidth test for a client with no IPv4 address");
                        return BandwidthTestRejected::with_reason(
                            BdErrorCode::ServiceNotAvailable,
                        )
                        .to_response();
                    }
                };

                let token: [u8; 8] = rand::random();

                debug!(
                    "Bandwidth test: {PACKET_COUNT} packets of {PACKET_SIZE} bytes \
                     over {DURATION_MS}ms to udp/{}",
                    self.endpoint.port()
                );

                BandwidthTestAccepted {
                    packet_size: PACKET_SIZE,
                    packet_count: PACKET_COUNT,
                    duration_ms: DURATION_MS,
                    port: self.endpoint.port(),
                    address,
                    token,
                }
                .to_response()
            }
            Some(BandwidthOperation::Finalize) => {
                let reported = reader.remaining_bytes().unwrap_or(0);

                let measured = session
                    .peer_addr()
                    .map(|a| self.endpoint.take(a.ip()))
                    .unwrap_or_default();

                debug!(
                    "Bandwidth test finished: {} packets, {} bytes arrived \
                     ({reported} bytes of client results)",
                    measured.packets, measured.bytes
                );

                TaskReply::with_only_error_code(NoError, BandwidthTaskId::BandwidthTask as u8)
                    .to_response()
            }
            None => {
                warn!("Unknown bandwidth operation {operation}");

                TaskReply::with_only_error_code(BdErrorCode::ServiceNotAvailable, operation)
                    .to_response()
            }
        }
    }
}
