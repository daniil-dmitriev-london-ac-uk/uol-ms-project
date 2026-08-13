use crate::io_access::SyncAccess;
use crate::placement::Placement;
use crate::uring_access::UringAccess;

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

pub enum WriteMode {
    Sync(SyncAccess),
    Uring(UringAccess),
}

impl WriteMode {
    fn write_at(&mut self, file: &File, off: u64, data: &[u8]) -> io::Result<()> {
        match self {
            WriteMode::Sync(io) => io.write_at(file, off, data),
            WriteMode::Uring(io) => io.write_at(file, off, data),
        }
    }
}

pub struct Store<P: Placement> {
    data: File,
    index: File,
    place: P,
    writer: WriteMode,
    reader: SyncAccess,
}

impl<P: Placement> Store<P> {
    pub fn open(dir: &Path, place: P, writer: WriteMode) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let data = OpenOptions::new().create(true).read(true).write(true).open(dir.join("data.hs"))?;
        let index = OpenOptions::new().create(true).read(true).write(true).open(dir.join("index.hs"))?;

        Ok(Store { data, index, place, writer, reader: SyncAccess::new() })
    }

    pub fn insert(&mut self, payload: &[u8]) -> io::Result<u64> {
        let (offset, id) = self.place.allocate(payload.len() as u64)?;
        let mut record = Vec::with_capacity(payload.len() + 16);

        record.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        record.extend_from_slice(&id.to_le_bytes());
        record.extend_from_slice(payload);
        self.writer.write_at(&self.data, offset, &record)?;
        let mut slot = [0u8; 16];
        slot[..8].copy_from_slice(&offset.to_le_bytes());
        slot[8..].copy_from_slice(&(record.len() as u64).to_le_bytes());
        self.writer.write_at(&self.index, id * 16, &slot)?;
        self.place.note_written(id, record.len() as u64);

        Ok(id)
    }

    pub fn read(&mut self, id: u64, out: &mut Vec<u8>) -> io::Result<()> {
        let mut slot = [0u8; 16];
        self.reader.read_at(&self.index, id * 16, &mut slot)?;
        let offset = u64::from_le_bytes(slot[..8].try_into().unwrap());
        let total = u64::from_le_bytes(slot[8..].try_into().unwrap()) as usize;
        let mut record = vec![0u8; total];

        self.reader.read_at(&self.data, offset, &mut record)?;
        out.clear();
        out.extend_from_slice(&record[16..]);

        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.data.sync_data()?;
        self.index.sync_data()
    }
}
