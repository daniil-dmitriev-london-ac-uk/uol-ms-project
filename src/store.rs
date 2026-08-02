use crate::placement::Placement;

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub struct Store<P: Placement> {
    data: File,
    index: File,
    place: P,
}

impl<P: Placement> Store<P> {
    pub fn open(dir: &Path, place: P) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let data = OpenOptions::new().create(true).read(true).write(true).open(dir.join("data.hs"))?;
        let index = OpenOptions::new().create(true).read(true).write(true).open(dir.join("index.hs"))?;

        Ok(Store { data, index, place })
    }

    pub fn insert(&mut self, payload: &[u8]) -> io::Result<u64> {
        let (offset, id) = self.place.allocate(payload.len() as u64)?;
        self.data.seek(SeekFrom::Start(offset))?;
        self.data.write_all(&(payload.len() as u64).to_le_bytes())?;
        self.data.write_all(&id.to_le_bytes())?;
        self.data.write_all(payload)?;

        self.index.seek(SeekFrom::Start(id * 16))?;
        self.index.write_all(&offset.to_le_bytes())?;
        self.index.write_all(&(payload.len() as u64 + 16).to_le_bytes())?;
        self.place.note_written(id, payload.len() as u64 + 16);

        Ok(id)
    }

    pub fn read(&mut self, id: u64, out: &mut Vec<u8>) -> io::Result<()> {
        self.index.seek(SeekFrom::Start(id * 16))?;
        let mut slot = [0u8; 16];
        self.index.read_exact(&mut slot)?;
        let offset = u64::from_le_bytes(slot[..8].try_into().unwrap());

        self.data.seek(SeekFrom::Start(offset))?;
        let mut header = [0u8; 16];
        self.data.read_exact(&mut header)?;
        let len = u64::from_le_bytes(header[..8].try_into().unwrap()) as usize;
        out.resize(len, 0);
        self.data.read_exact(out)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.data.sync_data()?;
        self.index.sync_data()
    }
}
