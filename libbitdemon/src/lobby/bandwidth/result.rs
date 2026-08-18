use crate::lobby::response::lsg_reply::LsgServiceTaskReply;
use crate::messaging::BdErrorCode;
use crate::messaging::bd_writer::BdWriter;
use num_traits::ToPrimitive;
use std::error::Error;

pub struct BandwidthTestRejected {
    reason: BdErrorCode,
}

impl BandwidthTestRejected {
    pub fn with_reason(reason: BdErrorCode) -> BandwidthTestRejected {
        BandwidthTestRejected { reason }
    }
}

impl LsgServiceTaskReply for BandwidthTestRejected {
    fn write_task_reply_data(&self, mut writer: BdWriter) -> Result<(), Box<dyn Error>> {
        // Test rejected
        writer.write_bool(true)?;

        // Rejected reason
        writer.write_u16(self.reason.to_u16().unwrap())?;

        Ok(())
    }
}

pub struct BandwidthTestAccepted {
    pub packet_size: u32,
    pub packet_count: u32,
    pub duration_ms: u32,
    pub port: u16,
    pub address: u32,
    pub token: [u8; 8],
}

impl LsgServiceTaskReply for BandwidthTestAccepted {
    fn write_task_reply_data(&self, mut writer: BdWriter) -> Result<(), Box<dyn Error>> {
        writer.write_bool(false)?;

        writer.write_u32(self.packet_size)?;
        writer.write_u32(self.packet_count)?;
        writer.write_u32(0)?;
        writer.write_u32(self.duration_ms)?;
        writer.write_u32(0)?;
        writer.write_u32(0)?;
        writer.write_u32(0)?;

        writer.write_u16(self.port)?;
        writer.write_u32(self.address)?;

        for b in self.token {
            writer.write_u8(b)?;
        }

        Ok(())
    }
}
