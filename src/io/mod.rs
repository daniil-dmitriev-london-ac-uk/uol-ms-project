pub mod sync;
pub mod uring;

use std::fs::File;
use std::io;
use std::ops::{Deref, DerefMut};

pub const PAGE: usize = 4096;

pub struct AlignedBuf {
    storage: Vec<u8>,
    start: usize,
    len: usize,
}

impl AlignedBuf {
    pub fn zeroed(len: usize) -> Self {
        let storage = vec![0u8; len + PAGE];
        let start = storage.as_ptr().align_offset(PAGE);

        AlignedBuf {
            storage,
            start,
            len,
        }
    }

    pub fn reserve(&mut self, need: usize) {
        if need <= self.capacity() {
            return;
        }

        let capacity = need.next_multiple_of(PAGE).max(self.capacity() * 2);
        let mut next = AlignedBuf::zeroed(capacity);

        next.len = self.len;
        next[..self.len].copy_from_slice(&self[..]);
        *self = next;
    }

    pub fn extend_from_slice(&mut self, data: &[u8]) {
        self.reserve(self.len + data.len());

        let append_offset = self.len;

        self.len += data.len();
        self[append_offset..].copy_from_slice(data);
    }

    pub fn resize_zeroed(&mut self, len: usize) {
        self.reserve(len);

        if len > self.len {
            let old = self.len;

            self.len = len;
            self[old..].fill(0);
        } else {
            self.len = len;
        }
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn capacity(&self) -> usize {
        self.storage.len().saturating_sub(self.start)
    }
}

impl Deref for AlignedBuf {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.storage[self.start..self.start + self.len]
    }
}

impl DerefMut for AlignedBuf {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.storage[self.start..self.start + self.len]
    }
}

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
