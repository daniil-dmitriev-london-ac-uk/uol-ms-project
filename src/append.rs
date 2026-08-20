use crate::data::DataFile;
use crate::format::{PAGE_SIZE_U64, SLOT_SIZE, Slot};
use crate::heap::HeapConfig;
use crate::index::IndexFile;
use crate::io::BlockIo;
use crate::placement::Placement;

use std::io;

pub struct AppendPlacement {
    pub cursor: u64,
    pub next_id: u64,
}

impl Placement for AppendPlacement {
    const WITH_BUCKET: bool = false;
    const LAYOUT: u8 = crate::format::LAYOUT_APPEND;

    fn open(
        io: &mut impl BlockIo,
        config: &HeapConfig,
        data: &mut DataFile,
        index: &mut IndexFile,
        created: bool,
    ) -> io::Result<Self> {
        if created {
            data.paged_file
                .ensure_alloc(config.initial_size, config.growth)?;

            return Ok(AppendPlacement {
                cursor: PAGE_SIZE_U64,
                next_id: 0,
            });
        }

        let size = index.paged_file.size()?;
        let mut next_id = 0u64;
        let mut cursor = PAGE_SIZE_U64;
        let mut offset = PAGE_SIZE_U64;

        while offset + SLOT_SIZE as u64 <= size {
            let chunk_size = (size - offset).min(1 << 20);
            let (buffer, buffer_offset) = index.paged_file.read_aligned(io, offset, chunk_size)?;

            for (slot_index, chunk) in buffer[buffer_offset..buffer_offset + chunk_size as usize]
                .chunks_exact(SLOT_SIZE)
                .enumerate()
            {
                if let Some(slot) = Slot::decode(chunk) {
                    next_id = (offset - PAGE_SIZE_U64) / SLOT_SIZE as u64 + slot_index as u64 + 1;
                    cursor = cursor.max(slot.offset + slot.total_len);
                }
            }

            index.paged_file.recycle(buffer);

            offset += chunk_size;
        }

        data.paged_file.note_alloc(data.paged_file.size()?);

        Ok(AppendPlacement { cursor, next_id })
    }

    fn alloc(
        &mut self,
        config: &HeapConfig,
        data: &mut DataFile,
        _bucket: u64,
        total_len: u64,
    ) -> io::Result<(u64, u64)> {
        let offset = self.cursor;

        data.paged_file
            .ensure_alloc(offset + total_len, config.growth)?;
        self.cursor += total_len;

        let id = self.next_id;

        self.next_id += 1;

        Ok((offset, id))
    }

    fn used_bytes(&self) -> u64 {
        self.cursor - PAGE_SIZE_U64
    }
}
