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
    fn allocate(&mut self, payload_len: u64) -> io::Result<(u64, u64)> {
        let offset = self.cursor;
        let id = self.next_id;

        self.cursor += payload_len + 20;
        self.next_id += 1;

        Ok((offset, id))
    }

    fn note_written(&mut self, _id: u64, _total_len: u64) {}
}
