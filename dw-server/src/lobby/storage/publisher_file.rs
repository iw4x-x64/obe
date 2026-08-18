use bitdemon::domain::result_slice::ResultSlice;
use bitdemon::domain::title::Title;
use bitdemon::lobby::storage::{
    FileVisibility, PublisherStorageService, StorageFile, StorageFileInfo, StorageServiceError,
};
use bitdemon::networking::bd_session::BdSession;
use log::{info, warn};
use num_traits::ToPrimitive;
use std::fs;
use std::fs::DirEntry;
use std::path::{Component, PathBuf};
use std::str::FromStr;
use std::time::UNIX_EPOCH;

pub struct DwPublisherStorageService {}

impl PublisherStorageService for DwPublisherStorageService {
    fn get_publisher_file_data(
        &self,
        session: &BdSession,
        filename: String,
    ) -> Result<Vec<u8>, StorageServiceError> {
        info!("Requesting publisher file {}", filename.as_str());

        let path_buf = PathBuf::from_str(&filename)
            .map_err(|_| StorageServiceError::StorageFileNotFoundError)?;

        let directory_traversal = path_buf
            .components()
            .any(|component| component == Component::ParentDir);

        if directory_traversal {
            warn!("User attempted directory traversal!",);
            return Err(StorageServiceError::StorageFileNotFoundError);
        }

        let full_file_path = format!(
            "storage/publisher/{}/{filename}",
            session.authentication().unwrap().title.to_u32().unwrap()
        );

        fs::read(full_file_path).map_err(|_| {
            warn!("Requested publisher file could not be found",);
            StorageServiceError::StorageFileNotFoundError
        })
    }

    fn get_publisher_file_data_by_id(
        &self,
        session: &BdSession,
        file_id: u64,
    ) -> Result<StorageFile, StorageServiceError> {
        info!("Requesting publisher file {file_id}");

        let title = session.authentication().unwrap().title;
        let full_dir_path = format!("storage/publisher/{}", title.to_u32().unwrap());

        let entry = fs::read_dir(full_dir_path)
            .map_err(|_| StorageServiceError::StorageFileNotFoundError)?
            .filter_map(|entry| entry.ok())
            .find(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| Self::file_id(name) == file_id)
            })
            .ok_or(StorageServiceError::StorageFileNotFoundError)?;

        let data =
            fs::read(entry.path()).map_err(|_| StorageServiceError::StorageFileNotFoundError)?;

        Ok(StorageFile {
            info: Self::map_info_info(title, entry),
            data,
        })
    }

    fn list_publisher_files(
        &self,
        session: &BdSession,
        min_date_time: i64,
        item_offset: usize,
        item_count: usize,
    ) -> Result<ResultSlice<StorageFileInfo>, StorageServiceError> {
        info!(
            "Listing publisher files min_date_time={min_date_time} item_offset={item_offset} item_count={item_count}"
        );

        let title = session.authentication().unwrap().title;
        let full_dir_path = format!("storage/publisher/{}", title.to_u32().unwrap());

        let dir = fs::read_dir(full_dir_path);
        if dir.is_err() {
            return Ok(ResultSlice::new(Vec::new(), item_offset));
        }

        let file_info: Vec<StorageFileInfo> = dir
            .unwrap()
            .filter(|entry| entry.is_ok())
            .skip(item_offset)
            .map(|entry| entry.unwrap())
            .map(|entry| Self::map_info_info(title, entry))
            .filter(|info| info.created >= min_date_time)
            .take(item_count)
            .collect();

        if !file_info.is_empty() {
            Ok(ResultSlice::new(file_info, item_offset))
        } else {
            Err(StorageServiceError::StorageFileNotFoundError)
        }
    }

    fn filter_publisher_files(
        &self,
        session: &BdSession,
        min_date_time: i64,
        item_offset: usize,
        item_count: usize,
        filter: String,
    ) -> Result<ResultSlice<StorageFileInfo>, StorageServiceError> {
        info!(
            "Filtering publisher files min_date_time={min_date_time} item_offset={item_offset} item_count={item_count} filter={filter}"
        );

        let title = session.authentication().unwrap().title;
        let full_dir_path = format!("storage/publisher/{}", title.to_u32().unwrap());

        let dir = fs::read_dir(full_dir_path);
        if dir.is_err() {
            return Ok(ResultSlice::new(Vec::new(), item_offset));
        }

        let file_info: Vec<StorageFileInfo> = dir
            .unwrap()
            .filter(|entry| entry.is_ok())
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_str()
                    .unwrap()
                    .starts_with(&filter)
            })
            .skip(item_offset)
            .map(|entry| entry.unwrap())
            .map(|entry| Self::map_info_info(title, entry))
            .filter(|info| info.created >= min_date_time)
            .take(item_count)
            .collect();

        if !file_info.is_empty() {
            Ok(ResultSlice::new(file_info, item_offset))
        } else {
            Err(StorageServiceError::StorageFileNotFoundError)
        }
    }
}

impl DwPublisherStorageService {
    pub fn new() -> DwPublisherStorageService {
        DwPublisherStorageService {}
    }

    pub fn file_id(filename: &str) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;

        for byte in filename.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }

        hash | 0x8000_0000_0000_0000
    }

    fn map_info_info(title: Title, entry: DirEntry) -> StorageFileInfo {
        let metadata = entry.metadata().unwrap();
        StorageFileInfo {
            id: Self::file_id(entry.file_name().to_str().unwrap()),
            filename: entry.file_name().into_string().unwrap(),
            title,
            file_size: metadata.len(),
            created: metadata
                .created()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            modified: metadata
                .modified()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            visibility: FileVisibility::VisiblePublic,
            owner_id: 0,
        }
    }
}
