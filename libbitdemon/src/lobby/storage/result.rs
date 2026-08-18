use crate::lobby::storage::service::{FileVisibility, StorageFile, StorageFileInfo};
use crate::messaging::bd_serialization::BdSerialize;
use crate::messaging::bd_writer::BdWriter;
use std::error::Error;

impl BdSerialize for StorageFileInfo {
    fn serialize(&self, writer: &mut BdWriter) -> Result<(), Box<dyn Error>> {
        writer.write_u32(self.file_size as u32)?;

        writer.write_u64(self.id)?;
        writer.write_u32((self.created % (u32::MAX as i64)) as u32)?;
        writer.write_u32((self.modified % (u32::MAX as i64)) as u32)?;
        writer.write_bool(self.visibility == FileVisibility::VisiblePrivate)?;
        writer.write_bool(false)?;
        writer.write_u64(self.owner_id)?;
        writer.write_str(self.filename.as_str())?;

        Ok(())
    }
}

pub struct FileDataResult {
    pub file: StorageFile,
}

impl BdSerialize for FileDataResult {
    fn serialize(&self, writer: &mut BdWriter) -> Result<(), Box<dyn Error>> {
        self.file.info.serialize(writer)?;
        writer.write_blob(self.file.data.as_slice())
    }
}
