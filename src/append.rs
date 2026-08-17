use crate::placement::Placement;

use std::io;

pub struct AppendPlacement {
    cursor: u64,
    next_id: u64,
}

impl AppendPlacement {
    pub fn new(cursor: u64, next_id: u64) -> Self {
        AppendPlacement { cursor, next_id }
    }
}

impl Placement for AppendPlacement {
    const WITH_BUCKET: bool = false;
    const LAYOUT: u8 = crate::format::LAYOUT_APPEND;

    fn allocate(&mut self, payload_len: u64) -> io::Result<(u64, u64)> {
        let offset = self.cursor;
        let id = self.next_id;

        self.cursor += payload_len + crate::format::header_len(false) as u64;
        self.next_id += 1;

        Ok((offset, id))
    }

    fn note_written(&mut self, _id: u64, _total_len: u64) {}

    fn bucket(&self) -> u64 {
        0
    }
}
