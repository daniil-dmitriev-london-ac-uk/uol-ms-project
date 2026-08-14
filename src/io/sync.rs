use super::BlockIo;

use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;

pub struct SyncIo;

impl BlockIo for SyncIo {
    fn read_at(&mut self, file: &File, mut off: u64, mut data: &mut [u8]) -> io::Result<()> {
        while !data.is_empty() {
            let processed_bytes = file.read_at(data, off)?;

            if processed_bytes == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }

            off += processed_bytes as u64;
            data = &mut data[processed_bytes..];
        }

        Ok(())
    }

    fn write_at(&mut self, file: &File, mut off: u64, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            let processed_bytes = file.write_at(data, off)?;

            if processed_bytes == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }

            off += processed_bytes as u64;
            data = &data[processed_bytes..];
        }

        Ok(())
    }

    fn sync(&mut self, file: &File) -> io::Result<()> {
        file.sync_data()
    }
}
