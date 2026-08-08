use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;

pub struct IoCounters {
    pub reads: u64,
    pub writes: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
}

pub struct SyncAccess {
    pub counters: IoCounters,
}

impl SyncAccess {
    pub fn new() -> Self {
        SyncAccess { counters: IoCounters { reads: 0, writes: 0, read_bytes: 0, write_bytes: 0 } }
    }

    pub fn write_at(&mut self, file: &File, mut off: u64, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            let written = file.write_at(data, off)?;

            if written == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }

            self.counters.writes += 1;
            self.counters.write_bytes += written as u64;
            off += written as u64;
            data = &data[written..];
        }

        Ok(())
    }

    pub fn read_at(&mut self, file: &File, mut off: u64, mut data: &mut [u8]) -> io::Result<()> {
        while !data.is_empty() {
            let read = file.read_at(data, off)?;

            if read == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }

            self.counters.reads += 1;
            self.counters.read_bytes += read as u64;
            off += read as u64;
            data = &mut data[read..];
        }

        Ok(())
    }
}
