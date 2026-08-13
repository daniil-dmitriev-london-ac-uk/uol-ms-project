use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;

pub trait BlockIo {
    fn read_at(&mut self, file: &File, off: u64, data: &mut [u8]) -> io::Result<()>;
    fn write_at(&mut self, file: &File, off: u64, data: &[u8]) -> io::Result<()>;
    fn sync(&mut self, file: &File) -> io::Result<()>;
}

pub struct SyncIo;

impl BlockIo for SyncIo {
    fn read_at(&mut self, file: &File, mut off: u64, mut data: &mut [u8]) -> io::Result<()> {
        while !data.is_empty() {
            let n = file.read_at(data, off)?;

            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }

            off += n as u64;
            data = &mut data[n..];
        }

        Ok(())
    }

    fn write_at(&mut self, file: &File, mut off: u64, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            let n = file.write_at(data, off)?;

            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }

            off += n as u64;
            data = &data[n..];
        }

        Ok(())
    }

    fn sync(&mut self, file: &File) -> io::Result<()> {
        file.sync_data()
    }
}
