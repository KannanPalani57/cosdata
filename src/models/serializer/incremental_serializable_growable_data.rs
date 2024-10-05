use super::CustomSerialize;
use crate::models::{
    buffered_io::{BufIoError, BufferManagerFactory},
    cache_loader::NodeRegistry,
    lazy_load::{
        FileIndex, IncrementalSerializableGrowableData, LazyItem, LazyItemVec, SyncPersist,
        VectorData,
    },
    types::{FileOffset, STM},
    versioning::Hash,
};
use std::collections::HashSet;
use std::{io::SeekFrom, sync::Arc};

impl CustomSerialize for IncrementalSerializableGrowableData {
    fn serialize(
        &self,
        bufmans: Arc<BufferManagerFactory>,
        version: Hash,
        cursor: u64,
    ) -> Result<u32, BufIoError> {
        let bufman = bufmans.get(&version)?;
        let start_offset = bufman.cursor_position(cursor)? as u32;

        // Store (data, version) pairs in a vector for serialization
        let mut items: Vec<_> = self
            .items
            .iter()
            .map(|item| {
                (
                    item.get_lazy_data().unwrap().get().clone().get().clone(),
                    item.get_current_version().clone(),
                )
            })
            .collect();

        let total_items = items.len();

        // Serialize number of items in the vector
        // Each item is an array of 64 u32s
        bufman.write_u32_with_cursor(cursor, total_items as u32)?;

        // Serialize individual items
        // First store version, then the array of 64 u32s
        for (item, version) in items.iter() {
            let item_start_offset = bufman.cursor_position(cursor)? as u32;
            if item.is_serialized() {
                // If the array is already serialized, move the cursor forward by 4 + (64 * 4) bytes (4 bytes for version and 64 * 4 bytes for items) and serialize the next array
                bufman.seek_with_cursor(
                    cursor,
                    SeekFrom::Start(item_start_offset as u64 + 64 * 4 + 4),
                )?;
                continue;
            }

            // Serialize the version
            bufman.write_u32_with_cursor(cursor, **version)?;

            // Serialize the array
            for i in 0..64 {
                bufman.write_u32_with_cursor(
                    cursor,
                    match item.get(i) {
                        Some(val) => val,
                        None => u32::MAX,
                    },
                )?;
            }
        }

        Ok(start_offset)
    }

    fn deserialize(
        bufmans: Arc<BufferManagerFactory>,
        file_index: FileIndex,
        cache: Arc<NodeRegistry>,
        max_loads: u16,
        skipm: &mut HashSet<u64>,
    ) -> Result<Self, BufIoError> {
        match file_index {
            FileIndex::Invalid => Ok(IncrementalSerializableGrowableData::new()),
            FileIndex::Valid {
                offset: FileOffset(offset),
                version,
            } => {
                let bufman = bufmans.get(&version)?;
                let cursor = bufman.open_cursor()?;
                bufman.seek_with_cursor(cursor, SeekFrom::Start(offset as u64))?;
                let mut items: LazyItemVec<STM<VectorData>> = LazyItemVec::new();

                // Deserialize the number of items in the vector
                let total_items = bufman.read_u32_with_cursor(cursor)? as usize;

                // Deserialize individual items
                for _ in 0..total_items {
                    let mut item = [u32::MAX; 64];
                    // Deserialize version
                    let version = bufman.read_u32_with_cursor(cursor)?;

                    // Deserialize elements
                    for i in 0..64 {
                        let val = bufman.read_u32_with_cursor(cursor)?;
                        item[i] = val;
                    }
                    items.push(LazyItem::new(
                        Hash::from(version),
                        STM::new(VectorData::from_array(item, true), 1, true),
                    ));
                }

                Ok(IncrementalSerializableGrowableData { items })
            }
        }
    }
}