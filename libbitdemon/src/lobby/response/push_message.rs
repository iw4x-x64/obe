use crate::lobby::response::BdMessageType;
use crate::messaging::StreamMode;
use crate::messaging::bd_response::{BdResponse, ResponseCreator};
use crate::messaging::bd_writer::BdWriter;
use num_traits::ToPrimitive;
use std::error::Error;

const FRIEND_CUSTOM_MESSAGE: u32 = 40;

pub struct PushMessage {
    pub recipient: u64,

    pub id: u64,

    pub timestamp: u32,

    pub sender: u64,
    pub sender_name: String,

    pub payload: Vec<u8>,
}

impl ResponseCreator for PushMessage {
    fn to_response(&self) -> Result<BdResponse, Box<dyn Error>> {
        let mut data = Vec::new();

        {
            let mut writer = BdWriter::new(&mut data);
            writer.set_type_checked(false);
            writer.set_mode(StreamMode::ByteMode);

            writer.write_u8(BdMessageType::LobbyServicePushMessage.to_u8().unwrap())?;

            writer.set_mode(StreamMode::BitMode);
            writer.set_type_checked(true);
            writer.write_type_checked_bit()?;

            writer.write_u32(FRIEND_CUSTOM_MESSAGE)?;

            writer.write_u64(self.recipient)?;
            writer.write_u64(self.id)?;
            writer.write_u32(self.timestamp)?;

            writer.write_bool(false)?;

            writer.write_u64(self.sender)?;
            writer.write_str(self.sender_name.as_str())?;

            writer.write_blob(self.payload.as_slice())?;

            writer.flush()?;
        }

        Ok(BdResponse::encrypted_if_available(data))
    }
}
