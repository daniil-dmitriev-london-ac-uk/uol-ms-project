//! append placement keeps records in allocation order

use crate::data::{DataFile, PagedFile};
use crate::error::Result;
use crate::format::{PAGE_SIZE_U64, SLOT_SIZE, Slot, SlotState};
use crate::heap::HeapConfig;
use crate::index::IndexFile;
use crate::io::BlockIo;
use crate::placement::{OpenStats, Placement};
use crate::recovery::walk;

use std::collections::HashMap;

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
        _registry: Option<&mut PagedFile>,
        created: bool,
    ) -> Result<(Self, OpenStats)> {
        if created {
            data.paged_file
                .ensure_alloc(config.initial_size, config.growth)?;

            return Ok((
                AppendPlacement {
                    cursor: PAGE_SIZE_U64,
                    next_id: 0,
                },
                OpenStats { restored: 0 },
            ));
        }

        // updates can extend beyond the greatest record number
        let index_size = index.paged_file.size()?;
        let mut next_id = 0u64;
        let mut max_end = PAGE_SIZE_U64;
        let mut offset = PAGE_SIZE_U64;

        while offset < index_size {
            let chunk_size = (index_size - offset).min(1 << 20);
            let (buffer, buffer_offset) = index.paged_file.read_aligned(io, offset, chunk_size)?;

            for (slot_index, chunk) in buffer[buffer_offset..buffer_offset + chunk_size as usize]
                .chunks_exact(SLOT_SIZE)
                .enumerate()
            {
                match Slot::state(chunk) {
                    SlotState::Live(slot) | SlotState::Deleted(slot) => {
                        next_id =
                            (offset - PAGE_SIZE_U64) / SLOT_SIZE as u64 + slot_index as u64 + 1;
                        max_end = max_end.max(slot.offset + slot.total_len);
                    }
                    SlotState::Empty => {}
                }
            }

            offset += chunk_size;
        }

        let mut cursor = max_end;
        let end = data.paged_file.size()?;
        let header_size = data.record_header_len();
        let mut found: Vec<(u64, u64, u16, u64)> = Vec::new();

        walk(
            io,
            &mut data.paged_file,
            false,
            data.integrity,
            cursor,
            end,
            false,
            |offset, header, payload_valid| {
                if !payload_valid {
                    return false;
                }

                found.push((header.id, offset, header.version, header.len + header_size));
                cursor = offset + header.len + header_size;

                true
            },
        )?;

        let mut restored = 0u64;
        let mut updates: HashMap<u64, (u64, u16, u64)> = HashMap::new();

        // only a contiguous tail is safe to restore
        for &(id, offset, version, record_len) in &found {
            if id == next_id {
                index.stage_slot(
                    io,
                    id,
                    Some(Slot {
                        offset: offset,
                        total_len: record_len,
                    }),
                )?;
                next_id += 1;
                restored += 1;
            } else if id < next_id {
                let entry = updates.entry(id).or_insert((offset, version, record_len));

                if version >= entry.1 {
                    *entry = (offset, version, record_len);
                }
            } else {
                break;
            }
        }

        for (id, (offset, version, record_len)) in updates {
            match index.read_slot_state(io, id)? {
                SlotState::Deleted(_) => continue,
                SlotState::Live(slot) if slot.offset == offset => continue,
                SlotState::Live(slot) => {
                    if let Ok(existing_header) = data.read_header(io, slot.offset, id) {
                        if existing_header.version >= version {
                            continue;
                        }
                    }
                }
                SlotState::Empty => {}
            }

            index.stage_slot(
                io,
                id,
                Some(Slot {
                    offset: offset,
                    total_len: record_len,
                }),
            )?;
            restored += 1;
        }

        if restored > 0 {
            index.paged_file.flush(io)?;
            io.sync(&index.paged_file.file)?;
        }

        data.paged_file.note_alloc(data.paged_file.size()?);

        Ok((AppendPlacement { cursor, next_id }, OpenStats { restored }))
    }

    fn alloc(
        &mut self,
        _io: &mut impl BlockIo,
        config: &HeapConfig,
        data: &mut DataFile,
        _registry: Option<&mut PagedFile>,
        _bucket: u64,
        total_len: u64,
    ) -> Result<(u64, u64)> {
        let offset = self.cursor;

        data.paged_file
            .ensure_alloc(offset + total_len, config.growth)?;
        self.cursor += total_len;

        let id = self.next_id;

        self.next_id += 1;

        Ok((offset, id))
    }

    fn alloc_update(
        &mut self,
        _io: &mut impl BlockIo,
        config: &HeapConfig,
        data: &mut DataFile,
        _registry: Option<&mut PagedFile>,
        _bucket: u64,
        total_len: u64,
    ) -> Result<u64> {
        let offset = self.cursor;

        data.paged_file
            .ensure_alloc(offset + total_len, config.growth)?;
        self.cursor += total_len;

        Ok(offset)
    }

    fn used_bytes(&self) -> u64 {
        self.cursor - PAGE_SIZE_U64
    }
}
