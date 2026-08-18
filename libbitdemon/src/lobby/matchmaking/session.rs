use crate::messaging::bd_reader::BdReader;
use crate::networking::bd_session::SessionId;
use crate::messaging::bd_serialization::BdSerialize;
use crate::messaging::bd_writer::BdWriter;
use log::debug;
use std::collections::HashMap;
use std::error::Error;
use std::sync::Mutex;

const ADDRESS_LEN: usize = 37;
const HOST_LEN: usize = 8;
const KEY_LEN: usize = 16;

const ID_LEN: usize = 8;
const SECRET_LEN: usize = 16;

#[derive(Clone, Debug)]
pub struct MatchMakingInfo {
    pub address: Vec<u8>,
    pub host: Vec<u8>,
    pub key: Vec<u8>,

    pub free_public_slots: i32,
    pub used_public_slots: i32,
    pub free_private_slots: i32,
    pub used_private_slots: i32,

    pub title_data: [i32; 9],
}

impl MatchMakingInfo {
    pub fn deserialize(reader: &mut BdReader) -> Result<MatchMakingInfo, Box<dyn Error>> {
        let address = reader.read_blob()?;
        let host = reader.read_blob()?;
        let key = reader.read_blob()?;

        if address.len() != ADDRESS_LEN || host.len() != HOST_LEN || key.len() != KEY_LEN {
            debug!(
                "Session advertisement blobs are {}/{}/{} bytes, not {ADDRESS_LEN}/{HOST_LEN}/{KEY_LEN}",
                address.len(),
                host.len(),
                key.len()
            );
        }

        let free_public_slots = reader.read_i32()?;
        let used_public_slots = reader.read_i32()?;
        let free_private_slots = reader.read_i32()?;
        let used_private_slots = reader.read_i32()?;

        let mut title_data = [0i32; 9];
        for field in title_data.iter_mut() {
            *field = reader.read_i32()?;
        }

        Ok(MatchMakingInfo {
            address,
            host,
            key,
            free_public_slots,
            used_public_slots,
            free_private_slots,
            used_private_slots,
            title_data,
        })
    }
}

impl BdSerialize for MatchMakingInfo {
    fn serialize(&self, writer: &mut BdWriter) -> Result<(), Box<dyn Error>> {
        writer.write_blob(self.address.as_slice())?;
        writer.write_blob(self.host.as_slice())?;
        writer.write_blob(self.key.as_slice())?;

        writer.write_i32(self.free_public_slots)?;
        writer.write_i32(self.used_public_slots)?;
        writer.write_i32(self.free_private_slots)?;
        writer.write_i32(self.used_private_slots)?;

        for field in self.title_data.iter() {
            writer.write_i32(*field)?;
        }

        Ok(())
    }
}

pub struct SessionCreateResult {
    pub id: Vec<u8>,
    pub secret: Vec<u8>,
}

impl BdSerialize for SessionCreateResult {
    fn serialize(&self, writer: &mut BdWriter) -> Result<(), Box<dyn Error>> {
        writer.write_blob(self.id.as_slice())?;
        writer.write_blob(self.secret.as_slice())
    }
}

pub struct SessionRegistry {
    sessions: Mutex<HashMap<SessionId, MatchMakingInfo>>,
}

impl SessionRegistry {
    pub fn new() -> SessionRegistry {
        SessionRegistry {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn create(
        &self,
        connection: SessionId,
        info: MatchMakingInfo,
    ) -> ([u8; ID_LEN], [u8; SECRET_LEN]) {
        let mut secret = [0u8; SECRET_LEN];
        let n = secret.len().min(info.key.len());
        secret[..n].copy_from_slice(&info.key[..n]);

        self.sessions.lock().unwrap().insert(connection, info);

        (connection.to_le_bytes(), secret)
    }

    pub fn update(&self, connection: SessionId, info: MatchMakingInfo) -> bool {
        self.sessions
            .lock()
            .unwrap()
            .insert(connection, info)
            .is_some()
    }

    pub fn delete(&self, id: &[u8]) -> bool {
        let Ok(key) = <[u8; ID_LEN]>::try_from(id) else {
            return false;
        };

        self.sessions
            .lock()
            .unwrap()
            .remove(&SessionId::from_le_bytes(key))
            .is_some()
    }

    pub fn list(&self) -> Vec<MatchMakingInfo> {
        self.sessions.lock().unwrap().values().cloned().collect()
    }
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub struct SessionQuery {
    pub kind: i32,

    pub unrecovered: i32,

    pub filters: [i32; 6],

    pub extra: Option<i32>,
}

impl SessionQuery {
    pub fn deserialize(reader: &mut BdReader) -> Result<SessionQuery, Box<dyn Error>> {
        let kind = reader.read_i32()?;
        let unrecovered = reader.read_i32()?;

        let mut filters = [0i32; 6];
        for filter in filters.iter_mut() {
            *filter = reader.read_i32()?;
        }

        let extra = if kind == 2 {
            Some(reader.read_i32()?)
        } else {
            None
        };

        Ok(SessionQuery {
            kind,
            unrecovered,
            filters,
            extra,
        })
    }
}
