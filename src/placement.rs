use std::io;

pub trait Placement {
    fn allocate(&mut self, payload_len: u64) -> io::Result<(u64, u64)>;
    fn note_written(&mut self, id: u64, total_len: u64);
}
