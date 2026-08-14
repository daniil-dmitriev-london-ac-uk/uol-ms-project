pub mod sync;
pub mod uring;

use std::fs::File;
use std::io;

pub trait BlockIo {
    fn read_at(&mut self, file: &File, off: u64, data: &mut [u8]) -> io::Result<()>;
    fn write_at(&mut self, file: &File, off: u64, data: &[u8]) -> io::Result<()>;
    fn sync(&mut self, file: &File) -> io::Result<()>;
}
