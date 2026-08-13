use crate::io_access::SyncAccess;
use crate::uring_access::UringAccess;

use std::fs::File;
use std::io;

pub enum IoMode {
    Sync(SyncAccess),
    Uring(UringAccess),
}

impl IoMode {
    pub fn write_at(&mut self, file: &File, off: u64, data: &[u8]) -> io::Result<()> {
        match self {
            IoMode::Sync(io) => io.write_at(file, off, data),
            IoMode::Uring(io) => io.write_at(file, off, data),
        }
    }

    pub fn read_at(&mut self, file: &File, off: u64, data: &mut [u8]) -> io::Result<()> {
        match self {
            IoMode::Sync(io) => io.read_at(file, off, data),
            IoMode::Uring(io) => io.read_at(file, off, data),
        }
    }
}
