use crate::data::{DataFile, PagedFile};
use crate::heap::HeapConfig;
use crate::index::IndexFile;
use crate::io::BlockIo;

use std::io;

pub trait Placement: Sized {
    const WITH_BUCKET: bool;
    const LAYOUT: u8;

    fn open(
        io: &mut impl BlockIo,
        config: &HeapConfig,
        data: &mut DataFile,
        index: &mut IndexFile,
        registry: Option<&mut PagedFile>,
        created: bool,
    ) -> io::Result<Self>;

    fn alloc(
        &mut self,
        io: &mut impl BlockIo,
        config: &HeapConfig,
        data: &mut DataFile,
        registry: Option<&mut PagedFile>,
        bucket: u64,
        total_len: u64,
    ) -> io::Result<(u64, u64)>;

    fn used_bytes(&self) -> u64;
}
