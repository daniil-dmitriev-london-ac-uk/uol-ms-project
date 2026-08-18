use crate::data::PagedFile;
use crate::format::{SLOT_SIZE, Slot};
use crate::io::BlockIo;

use std::io;

pub struct IndexFile {
    pub paged_file: PagedFile,
}

impl IndexFile {
    pub fn stage_slot(&mut self, io: &mut impl BlockIo, id: u64, slot: Slot) -> io::Result<()> {
        let mut buffer = [0u8; SLOT_SIZE];

        slot.encode(&mut buffer);
        self.paged_file.stage(io, Slot::file_offset(id), &[&buffer])
    }

    pub fn read_slot(&mut self, io: &mut impl BlockIo, id: u64) -> io::Result<Option<Slot>> {
        let (buffer, buffer_offset) =
            self.paged_file
                .read_aligned(io, Slot::file_offset(id), SLOT_SIZE as u64)?;
        let slot = Slot::decode(&buffer[buffer_offset..buffer_offset + SLOT_SIZE]);

        self.paged_file.recycle(buffer);

        Ok(slot)
    }
}
