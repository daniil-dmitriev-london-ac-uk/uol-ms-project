use crate::data::PagedFile;
use crate::error::Result;
use crate::format::{PAGE_SIZE_U64, SLOT_SIZE, Slot, TOMBSTONE_BIT, page_down};
use crate::io::{AlignedBuf, BlockIo, ReadReq};

pub struct IndexFile {
    pub paged_file: PagedFile,
}

impl IndexFile {
    pub fn stage_slot(&mut self, io: &mut impl BlockIo, id: u64, slot: Slot) -> Result<()> {
        let mut buffer = [0u8; SLOT_SIZE];

        slot.encode(&mut buffer);
        self.paged_file.stage(io, Slot::file_offset(id), &[&buffer])
    }

    pub fn stage_tombstone(
        &mut self,
        io: &mut impl BlockIo,
        id: u64,
        old_slot: Slot,
    ) -> Result<()> {
        let mut buffer = [0u8; SLOT_SIZE];

        Slot {
            offset: old_slot.offset | TOMBSTONE_BIT,
            total_len: old_slot.total_len,
        }
        .encode(&mut buffer);
        self.paged_file.stage(io, Slot::file_offset(id), &[&buffer])
    }

    pub fn read_slot(&mut self, io: &mut impl BlockIo, id: u64) -> Result<Option<Slot>> {
        let (buffer, buffer_offset) =
            self.paged_file
                .read_aligned(io, Slot::file_offset(id), SLOT_SIZE as u64)?;
        let slot = Slot::decode(&buffer[buffer_offset..buffer_offset + SLOT_SIZE]);

        self.paged_file.recycle(buffer);

        Ok(slot)
    }

    pub fn read_slots(&mut self, io: &mut impl BlockIo, ids: &[u64]) -> Result<Vec<Option<Slot>>> {
        self.paged_file.flush(io)?;

        let mut pages: Vec<u64> = ids
            .iter()
            .map(|&id| page_down(Slot::file_offset(id)))
            .collect();

        pages.sort_unstable();
        pages.dedup();

        let mut groups: Vec<(u64, u64)> = Vec::new();

        for &page in &pages {
            match groups.last_mut() {
                Some(group) if page == group.1 => group.1 += PAGE_SIZE_U64,
                _ => groups.push((page, page + PAGE_SIZE_U64)),
            }
        }

        let mut buffers: Vec<AlignedBuf> = groups
            .iter()
            .map(|group| self.paged_file.take_sized((group.1 - group.0) as usize))
            .collect();

        {
            let mut requests: Vec<ReadReq<'_>> = groups
                .iter()
                .zip(buffers.iter_mut())
                .map(|(group, buffer)| ReadReq {
                    off: group.0,
                    buf: buffer,
                })
                .collect();

            io.read_vec(&self.paged_file.file, &mut requests)?;
        }

        let mut slots = Vec::with_capacity(ids.len());

        for &id in ids {
            let slot_offset = Slot::file_offset(id);
            let group_index = groups.partition_point(|group| group.1 <= slot_offset);
            let buffer_offset = (slot_offset - groups[group_index].0) as usize;

            slots.push(Slot::decode(
                &buffers[group_index][buffer_offset..buffer_offset + SLOT_SIZE],
            ));
        }

        for buffer in buffers {
            self.paged_file.recycle(buffer);
        }

        Ok(slots)
    }
}
