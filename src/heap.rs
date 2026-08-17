use crate::crc32::crc32c;
use crate::data::PagedFile;
use crate::io::BlockIo;
use crate::placement::Placement;

use std::io;
use std::path::Path;

pub struct Heap<I: BlockIo, P: Placement> {
    io: I,
    placement: P,
    data: PagedFile,
    index: PagedFile,
}

impl<I: BlockIo, P: Placement> Heap<I, P> {
    pub fn open(dir: &Path, io: I, placement: P) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;

        let data_path = dir.join("data.hs");
        let index_path = dir.join("index.hs");
        let data = if data_path.exists() {
            PagedFile::open(&data_path)?
        } else {
            PagedFile::create(&data_path)?
        };
        let index = if index_path.exists() {
            PagedFile::open(&index_path)?
        } else {
            PagedFile::create(&index_path)?
        };

        Ok(Heap {
            io,
            placement,
            data,
            index,
        })
    }

    pub fn insert(&mut self, payload: &[u8]) -> io::Result<u64> {
        let (offset, id) = self.placement.allocate(payload.len() as u64)?;

        self.data
            .ensure_alloc(offset + payload.len() as u64 + 20, 64 << 20)?;
        self.index.ensure_alloc(id * 16 + 16, 1 << 20)?;

        let mut header = [0u8; 20];

        header[..8].copy_from_slice(&(payload.len() as u64).to_le_bytes());
        header[8..16].copy_from_slice(&id.to_le_bytes());
        header[16..20].copy_from_slice(&crc32c(payload).to_le_bytes());
        self.data.stage(&mut self.io, offset, &[&header, payload])?;

        let mut slot = [0u8; 16];

        slot[..8].copy_from_slice(&offset.to_le_bytes());
        slot[8..].copy_from_slice(&(payload.len() as u64 + 20).to_le_bytes());
        self.index.stage(&mut self.io, id * 16, &[&slot])?;

        self.placement.note_written(id, payload.len() as u64 + 20);

        Ok(id)
    }

    pub fn read(&mut self, id: u64, out: &mut Vec<u8>) -> io::Result<()> {
        let (slot_buf, at) = self.index.read_aligned(&mut self.io, id * 16, 16)?;
        let slot = &slot_buf[at..at + 16];
        let offset = u64::from_le_bytes(slot[..8].try_into().unwrap());
        let total = u64::from_le_bytes(slot[8..].try_into().unwrap());

        self.index.recycle(slot_buf);

        let (record, at) = self.data.read_aligned(&mut self.io, offset, total)?;

        out.clear();

        let payload = &record[at + 20..at + total as usize];
        let stored_crc = u32::from_le_bytes(record[at + 16..at + 20].try_into().unwrap());

        if crc32c(payload) != stored_crc {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "payload checksum mismatch",
            ));
        }

        out.extend_from_slice(payload);

        self.data.recycle(record);

        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.data.flush(&mut self.io)?;
        self.io.sync(&self.data.file)?;

        self.index.flush(&mut self.io)?;
        self.io.sync(&self.index.file)
    }

    pub fn io_counters(&self) -> crate::io::IoCounters {
        self.io.counters()
    }

    pub fn reset_io_counters(&mut self) {
        self.io.reset_counters();
    }

    pub fn pending_bytes(&self) -> usize {
        self.data.pending_bytes() + self.index.pending_bytes()
    }
}
