use crate::messaging::bd_reader::BdReader;
use crate::networking::bd_session::SessionId;
use crate::messaging::bd_serialization::BdSerialize;
use crate::messaging::bd_writer::BdWriter;
use snafu::{Snafu, ensure};
use std::collections::{BTreeMap, HashMap};
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

impl MatchMakingInfo {
    pub fn playlist(&self) -> u32 {
        self.title_data[1] as u32
    }

    pub fn players(&self) -> usize {
        (self.used_public_slots.max(0) + self.used_private_slots.max(0)) as usize
    }

    fn free_slots(&self) -> i32 {
        self.free_public_slots.max(0) + self.free_private_slots.max(0)
    }

    fn matches(&self, query: &SessionQuery) -> bool {
        let wants = |filter: i32, value: i32| filter == ANY || filter == value;

        wants(query.filters[1], self.title_data[1])
            && wants(query.filters[2], self.title_data[4])
            && wants(query.filters[4], self.title_data[2])
            && (query.filters[5] == ANY || self.free_slots() >= query.filters[5])
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

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Population {
    pub players: usize,
    pub playlists: BTreeMap<u32, usize>,
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

    pub fn remove_connection(&self, connection: SessionId) -> bool {
        self.state.lock().unwrap().forget(connection).is_some()
    }

    pub fn list_for(&self, connection: SessionId, query: &SessionQuery) -> Vec<MatchMakingInfo> {
        let state = self.state.lock().unwrap();

        state
            .sessions
            .iter()
            .filter(|(c, info)| **c != connection && info.matches(query))
            .map(|(_, info)| info.clone())
            .take(query.max_results())
            .collect()
    }

    pub fn population(&self) -> Population {
        let state = self.state.lock().unwrap();
        let mut population = Population::default();

        for info in state.sessions.values() {
            let players = info.players();

            if players > 0 {
                *population.playlists.entry(info.playlist()).or_default() += players;
                population.players += players;
            }
        }

        population
    }
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

const ANY: i32 = i32::MAX;

pub struct SessionQuery {
    pub kind: i32,

    pub limit: i32,

    pub filters: [i32; 6],

    pub extra: Option<i32>,
}

impl SessionQuery {
    fn max_results(&self) -> usize {
        match usize::try_from(self.limit) {
            Ok(limit) if limit > 0 => limit,
            _ => usize::MAX,
        }
    }

    pub fn deserialize(reader: &mut BdReader) -> Result<SessionQuery, Box<dyn Error>> {
        let kind = reader.read_i32()?;
        let limit = reader.read_i32()?;

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
            limit,
            filters,
            extra,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advertisement(host: u8, key: u8) -> MatchMakingInfo {
        MatchMakingInfo {
            address: vec![0u8; ADDRESS_LEN],
            host: vec![host; HOST_LEN],
            key: vec![key; KEY_LEN],
            free_public_slots: 0,
            used_public_slots: 0,
            free_private_slots: 0,
            used_private_slots: 0,
            title_data: [0i32; 9],
        }
    }

    fn lobby(host: u8, playlist: i32, public: i32, private: i32) -> MatchMakingInfo {
        let mut info = advertisement(host, host);
        info.used_public_slots = public;
        info.used_private_slots = private;
        info.title_data[1] = playlist;

        info
    }

    fn any() -> SessionQuery {
        SessionQuery {
            kind: 2,
            limit: 50,
            filters: [ANY; 6],
            extra: None,
        }
    }

    fn search(playlist: i32, size: i32) -> SessionQuery {
        let mut query = any();
        query.filters = [8, playlist, 142, 0, 3, size];

        query
    }

    fn open_lobby(host: u8, playlist: i32, free: i32) -> MatchMakingInfo {
        let mut info = advertisement(host, host);
        info.free_public_slots = free;
        info.title_data = [8, playlist, 3, 0, 142, 0, playlist, 0, 0];

        info
    }

    fn counts(population: &Population) -> Vec<(u32, usize)> {
        population.playlists.iter().map(|(k, v)| (*k, *v)).collect()
    }

    #[test]
    fn population_sums_used_slots_by_playlist() {
        let registry = SessionRegistry::new();

        registry.create(1, lobby(1, 5, 3, 1));
        registry.create(2, lobby(2, 5, 2, 0));
        registry.create(3, lobby(3, 7, 6, 0));

        let population = registry.population();

        assert_eq!(population.players, 12);
        assert_eq!(counts(&population), vec![(5, 6), (7, 6)]);
    }

    #[test]
    fn population_follows_updates_and_departures() {
        let registry = SessionRegistry::new();

        registry.create(1, lobby(1, 5, 3, 0));
        registry.create(2, lobby(2, 7, 4, 0));
        registry.update(1, lobby(1, 9, 2, 0));
        registry.remove_connection(2);

        let population = registry.population();

        assert_eq!(population.players, 2);
        assert_eq!(counts(&population), vec![(9, 2)]);
    }

    #[test]
    fn an_empty_lobby_is_left_out() {
        let registry = SessionRegistry::new();

        registry.create(1, lobby(1, 5, 0, 0));

        assert_eq!(registry.population(), Population::default());
    }

    #[test]
    fn create_names_the_session_by_what_the_client_advertised() {
        let registry = SessionRegistry::new();

        let (id, secret) = registry.create(7, advertisement(0xab, 0xcd));

        assert_eq!(id, [0xabu8; ID_LEN]);
        assert_eq!(secret, [0xcdu8; SECRET_LEN]);
    }

    #[test]
    fn a_connection_cannot_delete_a_session_it_does_not_own() {
        let registry = SessionRegistry::new();

        let (id, _) = registry.create(1, advertisement(0xab, 0xcd));

        assert!(!registry.delete(2, id.as_slice()));
        assert_eq!(registry.list_for(2, &any()).len(), 1);

        assert!(registry.delete(1, id.as_slice()));
        assert_eq!(registry.list_for(2, &any()).len(), 0);
    }

    #[test]
    fn a_lost_connection_takes_its_session_with_it() {
        let registry = SessionRegistry::new();

        registry.create(1, advertisement(0xab, 0xcd));

        assert!(registry.remove_connection(1));
        assert!(!registry.remove_connection(1));
        assert_eq!(registry.list_for(2, &any()).len(), 0);
    }

    #[test]
    fn a_client_is_never_shown_its_own_session() {
        let registry = SessionRegistry::new();

        registry.create(1, advertisement(0xab, 0xcd));
        registry.create(2, advertisement(0x12, 0x34));

        assert_eq!(registry.list_for(1, &any()).len(), 1);
        assert_eq!(registry.list_for(1, &any())[0].host, vec![0x12u8; HOST_LEN]);
    }

    #[test]
    fn re_advertising_replaces_rather_than_accumulates() {
        let registry = SessionRegistry::new();

        let (first, _) = registry.create(1, advertisement(0xab, 0xcd));
        assert!(registry.update(1, advertisement(0x99, 0x88)));

        assert_eq!(registry.list_for(2, &any()).len(), 1);
        assert!(!registry.delete(1, first.as_slice()));
        assert!(registry.delete(1, [0x99u8; ID_LEN].as_slice()));
    }

    #[test]
    fn a_search_finds_only_lobbies_it_could_join() {
        let registry = SessionRegistry::new();

        registry.create(1, open_lobby(1, 5, 4));

        let mut other_playlist = open_lobby(2, 7, 4);
        other_playlist.title_data[6] = 7;
        registry.create(2, other_playlist);

        let mut old_playlists = open_lobby(3, 5, 4);
        old_playlists.title_data[2] = 2;
        registry.create(3, old_playlists);

        let mut old_protocol = open_lobby(4, 5, 4);
        old_protocol.title_data[4] = 141;
        registry.create(4, old_protocol);

        registry.create(5, open_lobby(5, 5, 1));

        let found = registry.list_for(9, &search(5, 2));

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].host, vec![1u8; HOST_LEN]);
    }

    #[test]
    fn private_slots_count_toward_room() {
        let registry = SessionRegistry::new();

        let mut lobby = open_lobby(1, 5, 1);
        lobby.free_private_slots = 1;
        registry.create(1, lobby);

        assert_eq!(registry.list_for(9, &search(5, 2)).len(), 1);
    }
}
