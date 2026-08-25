use crate::messaging::bd_reader::BdReader;
use crate::networking::bd_session::SessionId;
use crate::messaging::bd_serialization::BdSerialize;
use crate::messaging::bd_writer::BdWriter;
use snafu::{Snafu, ensure};
use std::collections::HashMap;
use std::error::Error;
use std::sync::Mutex;

const ADDRESS_LEN: usize = 37;
const HOST_LEN: usize = 8;
const KEY_LEN: usize = 16;

const ID_LEN: usize = HOST_LEN;
const SECRET_LEN: usize = KEY_LEN;

#[derive(Debug, Snafu)]
enum MatchMakingError {
    #[snafu(display(
        "Session advertisement blobs are {address}/{host}/{key} bytes, \
         not {ADDRESS_LEN}/{HOST_LEN}/{KEY_LEN}"
    ))]
    MalformedAdvertisement {
        address: usize,
        host: usize,
        key: usize,
    },
}

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

        ensure!(
            address.len() == ADDRESS_LEN && host.len() == HOST_LEN && key.len() == KEY_LEN,
            MalformedAdvertisementSnafu {
                address: address.len(),
                host: host.len(),
                key: key.len(),
            }
        );

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
    state: Mutex<RegistryState>,
}

#[derive(Default)]
struct RegistryState {
    sessions: HashMap<SessionId, MatchMakingInfo>,
    by_host: HashMap<[u8; ID_LEN], SessionId>,
}

impl RegistryState {
    fn forget(&mut self, connection: SessionId) -> Option<MatchMakingInfo> {
        let info = self.sessions.remove(&connection)?;

        if let Ok(host) = <[u8; ID_LEN]>::try_from(info.host.as_slice())
            && self.by_host.get(&host) == Some(&connection)
        {
            self.by_host.remove(&host);
        }

        Some(info)
    }

    fn remember(&mut self, connection: SessionId, info: MatchMakingInfo) {
        if let Ok(host) = <[u8; ID_LEN]>::try_from(info.host.as_slice()) {
            self.by_host.insert(host, connection);
        }

        self.sessions.insert(connection, info);
    }
}

impl SessionRegistry {
    pub fn new() -> SessionRegistry {
        SessionRegistry {
            state: Mutex::new(RegistryState::default()),
        }
    }

    pub fn create(
        &self,
        connection: SessionId,
        info: MatchMakingInfo,
    ) -> ([u8; ID_LEN], [u8; SECRET_LEN]) {
        let id = <[u8; ID_LEN]>::try_from(info.host.as_slice()).unwrap_or([0u8; ID_LEN]);
        let secret = <[u8; SECRET_LEN]>::try_from(info.key.as_slice()).unwrap_or([0u8; SECRET_LEN]);

        let mut state = self.state.lock().unwrap();
        state.forget(connection);
        state.remember(connection, info);

        (id, secret)
    }

    pub fn update(&self, connection: SessionId, info: MatchMakingInfo) -> bool {
        let mut state = self.state.lock().unwrap();
        let had = state.forget(connection).is_some();
        state.remember(connection, info);

        had
    }

    pub fn delete(&self, connection: SessionId, id: &[u8]) -> bool {
        let Ok(host) = <[u8; ID_LEN]>::try_from(id) else {
            return false;
        };

        let mut state = self.state.lock().unwrap();

        if state.by_host.get(&host) != Some(&connection) {
            return false;
        }

        state.forget(connection).is_some()
    }

    pub fn list_for(&self, connection: SessionId) -> Vec<MatchMakingInfo> {
        let state = self.state.lock().unwrap();

        state
            .sessions
            .iter()
            .filter(|(c, _)| **c != connection)
            .map(|(_, info)| info.clone())
            .collect()
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
