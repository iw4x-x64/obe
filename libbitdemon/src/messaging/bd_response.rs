use crate::crypto::{encrypt_buffer_in_place, generate_iv_from_seed, generate_iv_seed};
use crate::networking::bd_session::BdSession;
use byteorder::{LittleEndian, WriteBytesExt};
use std::error::Error;
use std::io::Write;

pub struct BdResponse {
    should_encrypt: bool,
    data: Vec<u8>,
}

pub trait ResponseCreator {
    fn to_response(&self) -> Result<BdResponse, Box<dyn Error>>;
}

const RESPONSE_SIGNATURE: u32 = 0xDEADBEEF;

impl BdResponse {
    pub fn unencrypted(data: Vec<u8>) -> Self {
        BdResponse {
            should_encrypt: false,
            data,
        }
    }
    pub fn encrypted_if_available(data: Vec<u8>) -> Self {
        BdResponse {
            should_encrypt: true,
            data,
        }
    }

    pub fn send(&mut self, session: &mut BdSession) -> Result<(), Box<dyn Error>> {
        let key = session
            .authentication()
            .map(|a| a.session_key)
            .filter(|_| self.should_encrypt);

        self.send_to(session, key.as_ref())
    }

    pub fn send_to<W: Write>(
        &mut self,
        writer: &mut W,
        key: Option<&[u8; 24]>,
    ) -> Result<(), Box<dyn Error>> {
        log::trace!("Response ({} bytes): {:02x?}", self.data.len(), self.data);

        if let Some(key) = key {
            let seed = generate_iv_seed();
            let iv = generate_iv_from_seed(seed);

            self.data
                .splice(0..0, RESPONSE_SIGNATURE.to_le_bytes().iter().cloned());
            encrypt_buffer_in_place(&mut self.data, key, &iv);

            // Written length minus length field itself
            // 1 byte (encrypted) + 4 byte (seed)
            let message_length = self.data.len() + 5;
            writer.write_u32::<LittleEndian>(message_length as u32)?;
            writer.write_u8(1u8)?; // Encrypted
            writer.write_u32::<LittleEndian>(seed)?;
            writer.write_all(self.data.as_slice())?;
        } else {
            // Written length minus length field itself
            let message_length = self.data.len() + 1;
            writer.write_u32::<LittleEndian>(message_length as u32)?;
            writer.write_u8(0u8)?; // Encrypted
            writer.write_all(self.data.as_slice())?;
        }

        Ok(())
    }
}
