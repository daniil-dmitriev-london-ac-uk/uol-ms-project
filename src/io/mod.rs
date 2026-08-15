pub mod sync;
pub mod uring;

use std::fs::File;
use std::io;

#[derive(Debug, Default, Clone, Copy)]
pub struct IoCounters {
    pub reads: u64,
    pub writes: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub syncs: u64,
}

pub struct ReadReq<'a> {
    pub off: u64,
    pub buf: &'a mut [u8],
}

pub struct WriteReq<'a> {
    pub off: u64,
    pub buf: &'a [u8],
}

pub trait BlockIo {
    fn read_vec(&mut self, file: &File, requests: &mut [ReadReq<'_>]) -> io::Result<()>;
    fn write_vec(&mut self, file: &File, requests: &[WriteReq<'_>]) -> io::Result<()>;
    fn sync(&mut self, file: &File) -> io::Result<()>;
    fn counters(&self) -> IoCounters;
    fn reset_counters(&mut self);
}

pub fn sync_counted(file: &File, counters: &mut IoCounters) -> io::Result<()> {
    file.sync_data()?;

    counters.syncs += 1;

    Ok(())
}
