use std::io;

pub trait Placement {
    const WITH_BUCKET: bool;
    const LAYOUT: u8;

    fn allocate(&mut self, payload_len: u64) -> io::Result<(u64, u64)>;
    fn note_written(&mut self, id: u64, total_len: u64);
    fn bucket(&self) -> u64;
}
