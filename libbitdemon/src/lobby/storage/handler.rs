use crate::domain::result_slice::ResultSlice;
use crate::lobby::LobbyHandler;
use crate::lobby::response::task_reply::TaskReply;
use crate::lobby::storage::result::FileDataResult;
use crate::lobby::storage::service::{
    FileVisibility, StorageFileInfo, StorageServiceError, ThreadSafePublisherStorageService,
    ThreadSafeUserStorageService,
};
use crate::messaging::BdErrorCode;
use crate::messaging::bd_message::BdMessage;
use crate::messaging::bd_reader::BdReader;
use crate::messaging::bd_response::{BdResponse, ResponseCreator};
use crate::networking::bd_session::BdSession;
use log::{trace, warn};
use num_traits::FromPrimitive;
use std::error::Error;
use std::sync::Arc;

pub struct StorageHandler {
    storage_service: Arc<ThreadSafeUserStorageService>,
    publisher_storage_service: Arc<ThreadSafePublisherStorageService>,
}

#[derive(Debug, Eq, PartialEq, Hash, Copy, Clone, FromPrimitive, ToPrimitive)]
#[repr(u8)]
enum StorageTaskId {
    CreateFile = 1,
    UpdateFileById = 2,
    GetFileById = 5,
    ListFilesByOwner = 7,
    ListPublisherFiles = 8,
}

impl LobbyHandler for StorageHandler {
    fn handle_message(
        &self,
        session: &mut BdSession,
        mut message: BdMessage,
    ) -> Result<BdResponse, Box<dyn Error>> {
        let task_id_value = message.reader.read_u8()?;

        let unrecovered = message.reader.read_u8()?;
        if unrecovered != 0 {
            trace!("Storage request leading byte is {unrecovered}, not the usual 0");
        }

        let maybe_task_id = StorageTaskId::from_u8(task_id_value);
        if maybe_task_id.is_none() {
            warn!("Client called unknown storage task {task_id_value}");
            return TaskReply::with_only_error_code(BdErrorCode::ServiceNotAvailable, task_id_value)
                .to_response();
        }

        match maybe_task_id.unwrap() {
            StorageTaskId::CreateFile => self.create_file(session, &mut message.reader),
            StorageTaskId::UpdateFileById => self.update_file_by_id(session, &mut message.reader),
            StorageTaskId::GetFileById => self.get_file_by_id(session, &mut message.reader),
            StorageTaskId::ListFilesByOwner => {
                self.list_files_by_owner(session, &mut message.reader)
            }
            StorageTaskId::ListPublisherFiles => {
                self.list_publisher_files(session, &mut message.reader)
            }
        }
    }
}

impl StorageHandler {
    pub fn new(
        storage_service: Arc<ThreadSafeUserStorageService>,
        publisher_storage_service: Arc<ThreadSafePublisherStorageService>,
    ) -> StorageHandler {
        StorageHandler {
            storage_service,
            publisher_storage_service,
        }
    }

    fn create_file(
        &self,
        session: &mut BdSession,
        reader: &mut BdReader,
    ) -> Result<BdResponse, Box<dyn Error>> {
        let first_flag = reader.read_bool()?;
        let filename = reader.read_str()?;
        let second_flag = reader.read_bool()?;
        let file_data = reader.read_blob()?;

        trace!(
            "Create file {filename}, {} bytes, flags {first_flag}/{second_flag}",
            file_data.len()
        );

        let owner_id = session.authentication().unwrap().user_id;

        let result = self.storage_service.create_storage_file(
            session,
            owner_id,
            filename,
            FileVisibility::VisiblePrivate,
            file_data,
        );

        match result {
            Ok(info) => Ok(TaskReply::with_single_result(
                StorageTaskId::CreateFile,
                Box::from(info),
            )
            .to_response()?),
            Err(error) => Ok(TaskReply::with_only_error_code(
                error.into(),
                StorageTaskId::CreateFile,
            )
            .to_response()?),
        }
    }

    fn update_file_by_id(
        &self,
        session: &mut BdSession,
        reader: &mut BdReader,
    ) -> Result<BdResponse, Box<dyn Error>> {
        let file_id = reader.read_u64()?;
        let file_data = reader.read_blob()?;

        trace!("Update file {file_id}, {} bytes", file_data.len());

        let owner_id = session.authentication().unwrap().user_id;

        let result =
            self.storage_service
                .update_storage_file_data(session, owner_id, file_id, file_data);

        match result {
            Ok(_) => Ok(TaskReply::with_only_error_code(
                BdErrorCode::NoError,
                StorageTaskId::UpdateFileById,
            )
            .to_response()?),
            Err(error) => Ok(TaskReply::with_only_error_code(
                error.into(),
                StorageTaskId::UpdateFileById,
            )
            .to_response()?),
        }
    }

    fn get_file_by_id(
        &self,
        session: &mut BdSession,
        reader: &mut BdReader,
    ) -> Result<BdResponse, Box<dyn Error>> {
        let file_id = reader.read_u64()?;

        trace!("Get file {file_id}");

        let result = self
            .storage_service
            .get_storage_file_data_by_id(
                session,
                session.authentication().unwrap().user_id,
                file_id,
            )
            .or_else(|_| {
                self.publisher_storage_service
                    .get_publisher_file_data_by_id(session, file_id)
            });

        match result {
            Ok(file) => Ok(TaskReply::with_single_result(
                StorageTaskId::GetFileById,
                Box::from(FileDataResult { file }),
            )
            .to_response()?),
            Err(error) => Ok(TaskReply::with_only_error_code(
                error.into(),
                StorageTaskId::GetFileById,
            )
            .to_response()?),
        }
    }

    fn list_files_by_owner(
        &self,
        session: &mut BdSession,
        reader: &mut BdReader,
    ) -> Result<BdResponse, Box<dyn Error>> {
        let owner_id = reader.read_u64()?;
        let min_date_time = reader.read_u32()?;
        let max_num_results = reader.read_u16()?;

        trace!("List files of {owner_id}, since {min_date_time}, up to {max_num_results}");

        let result = if reader.next_is_str().unwrap_or(false) {
            let filter = reader.read_str()?;
            self.storage_service.filter_storage_files(
                session,
                owner_id,
                min_date_time as i64,
                0,
                max_num_results as usize,
                filter,
            )
        } else {
            self.storage_service.list_storage_files(
                session,
                owner_id,
                min_date_time as i64,
                0,
                max_num_results as usize,
            )
        };

        self.answer_for_file_info_slice(StorageTaskId::ListFilesByOwner, result)
    }

    fn list_publisher_files(
        &self,
        session: &mut BdSession,
        reader: &mut BdReader,
    ) -> Result<BdResponse, Box<dyn Error>> {
        let min_date_time = reader.read_u32()?;
        let max_num_results = reader.read_u16()?;

        trace!("List publisher files since {min_date_time}, up to {max_num_results}");

        let result = if reader.next_is_str().unwrap_or(false) {
            let filter = reader.read_str()?;
            self.publisher_storage_service.filter_publisher_files(
                session,
                min_date_time as i64,
                0,
                max_num_results as usize,
                filter,
            )
        } else {
            self.publisher_storage_service.list_publisher_files(
                session,
                min_date_time as i64,
                0,
                max_num_results as usize,
            )
        };

        self.answer_for_file_info_slice(StorageTaskId::ListPublisherFiles, result)
    }

    fn answer_for_file_info_slice(
        &self,
        task_id: StorageTaskId,
        result: Result<ResultSlice<StorageFileInfo>, StorageServiceError>,
    ) -> Result<BdResponse, Box<dyn Error>> {
        match result {
            Ok(info) => {
                Ok(TaskReply::with_result_slice(task_id, info.serializable()).to_response()?)
            }
            Err(error) => Ok(TaskReply::with_only_error_code(error.into(), task_id).to_response()?),
        }
    }
}

impl From<StorageServiceError> for BdErrorCode {
    fn from(value: StorageServiceError) -> Self {
        match value {
            StorageServiceError::PermissionDeniedError => BdErrorCode::PermissionDenied,
            StorageServiceError::FilenameTooLongError => BdErrorCode::FilenameMaxLengthExceeded,
            StorageServiceError::StorageFileTooLargeError => BdErrorCode::FileSizeLimitExceeded,
            StorageServiceError::StorageFileNotFoundError => BdErrorCode::NoFile,
        }
    }
}
