use crate::io::{BlockIo, ReadReq, WriteReq};
use crate::placement::Placement;

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

pub struct Heap<I: BlockIo, P: Placement> {
    io: I,
    placement: P,
    data: File,
    index: File,
}

impl<I: BlockIo, P: Placement> Heap<I, P> {
    pub fn open(dir: &Path, io: I, placement: P) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;

        let data = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(dir.join("data.hs"))?;
        let index = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(dir.join("index.hs"))?;

        Ok(Heap {
            io,
            placement,
            data,
            index,
        })
    }

    pub fn insert(&mut self, payload: &[u8]) -> io::Result<u64> {
        let (offset, id) = self.placement.allocate(payload.len() as u64)?;
        let mut record = Vec::with_capacity(payload.len() + 16);

        record.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        record.extend_from_slice(&id.to_le_bytes());
        record.extend_from_slice(payload);

        self.io.write_vec(
            &self.data,
            &[WriteReq {
                off: offset,
                buf: &record,
            }],
        )?;

        let mut slot = [0u8; 16];

        slot[..8].copy_from_slice(&offset.to_le_bytes());
        slot[8..].copy_from_slice(&(record.len() as u64).to_le_bytes());

        self.io.write_vec(
            &self.index,
            &[WriteReq {
                off: id * 16,
                buf: &slot,
            }],
        )?;

        self.placement.note_written(id, record.len() as u64);

        Ok(id)
    }

    pub fn read(&mut self, id: u64, out: &mut Vec<u8>) -> io::Result<()> {
        let mut slot = [0u8; 16];

        self.io.read_vec(
            &self.index,
            &mut [ReadReq {
                off: id * 16,
                buf: &mut slot,
            }],
        )?;

        let offset = u64::from_le_bytes(slot[..8].try_into().unwrap());
        let total = u64::from_le_bytes(slot[8..].try_into().unwrap()) as usize;
        let mut record = vec![0u8; total];

        self.io.read_vec(
            &self.data,
            &mut [ReadReq {
                off: offset,
                buf: &mut record,
            }],
        )?;

        out.clear();
        out.extend_from_slice(&record[16..]);

        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.io.sync(&self.data)?;
        self.io.sync(&self.index)
    }

    pub fn io_counters(&self) -> crate::io::IoCounters {
        self.io.counters()
    }

    pub fn reset_io_counters(&mut self) {
        self.io.reset_counters();
    }
}
