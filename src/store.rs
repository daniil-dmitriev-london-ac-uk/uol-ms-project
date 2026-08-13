use crate::block_io::BlockIo;
use crate::placement::Placement;

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

pub struct Store<I: BlockIo, P: Placement> {
    io: I,
    place: P,
    data: File,
    index: File,
}

impl<I: BlockIo, P: Placement> Store<I, P> {
    pub fn open(dir: &Path, io: I, place: P) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let data = OpenOptions::new().create(true).read(true).write(true).open(dir.join("data.hs"))?;
        let index = OpenOptions::new().create(true).read(true).write(true).open(dir.join("index.hs"))?;

        Ok(Store { io, place, data, index })
    }

    pub fn insert(&mut self, payload: &[u8]) -> io::Result<u64> {
        let (offset, id) = self.place.allocate(payload.len() as u64)?;
        let mut record = Vec::with_capacity(payload.len() + 16);

        record.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        record.extend_from_slice(&id.to_le_bytes());
        record.extend_from_slice(payload);
        self.io.write_at(&self.data, offset, &record)?;
        let mut slot = [0u8; 16];
        slot[..8].copy_from_slice(&offset.to_le_bytes());
        slot[8..].copy_from_slice(&(record.len() as u64).to_le_bytes());
        self.io.write_at(&self.index, id * 16, &slot)?;
        self.place.note_written(id, record.len() as u64);

        Ok(id)
    }

    pub fn read(&mut self, id: u64, out: &mut Vec<u8>) -> io::Result<()> {
        let mut slot = [0u8; 16];
        self.io.read_at(&self.index, id * 16, &mut slot)?;
        let offset = u64::from_le_bytes(slot[..8].try_into().unwrap());
        let total = u64::from_le_bytes(slot[8..].try_into().unwrap()) as usize;
        let mut record = vec![0u8; total];

        self.io.read_at(&self.data, offset, &mut record)?;
        out.clear();
        out.extend_from_slice(&record[16..]);

        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.io.sync(&self.data)?;
        self.io.sync(&self.index)
    }
}
