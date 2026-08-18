use crate::data::{DataFile, PagedFile};
use crate::format::{SLOT_SIZE, Slot};
use crate::index::IndexFile;
use crate::io::BlockIo;
use crate::placement::Placement;

use std::io;
use std::path::Path;

pub struct Heap<I: BlockIo, P: Placement> {
    io: I,
    placement: P,
    data: DataFile,
    index: IndexFile,
}

impl<I: BlockIo, P: Placement> Heap<I, P> {
    pub fn open(dir: &Path, mut io: I, placement: P) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;

        let data_path = dir.join("data.hs");
        let index_path = dir.join("index.hs");
        let data_paged_file = if data_path.exists() {
            PagedFile::open(&data_path, b'D', P::LAYOUT, &mut io)?
        } else {
            PagedFile::create(&data_path, b'D', P::LAYOUT, &mut io)?
        };
        let data = DataFile {
            paged_file: data_paged_file,
            with_bucket: P::WITH_BUCKET,
            integrity: true,
        };
        let index_paged_file = if index_path.exists() {
            PagedFile::open(&index_path, b'I', P::LAYOUT, &mut io)?
        } else {
            PagedFile::create(&index_path, b'I', P::LAYOUT, &mut io)?
        };
        let index = IndexFile {
            paged_file: index_paged_file,
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
        let total = self.data.record_header_len() + payload.len() as u64;

        self.data
            .paged_file
            .ensure_alloc(offset + total, 64 << 20)?;
        self.index
            .paged_file
            .ensure_alloc(Slot::file_offset(id) + SLOT_SIZE as u64, 1 << 20)?;

        self.data.stage_record(
            &mut self.io,
            offset,
            self.placement.bucket(),
            id,
            1,
            payload,
        )?;

        self.index.stage_slot(
            &mut self.io,
            id,
            Slot {
                offset,
                total_len: total,
            },
        )?;

        self.placement.note_written(id, total);

        Ok(id)
    }

    pub fn read(&mut self, id: u64, out: &mut Vec<u8>) -> io::Result<()> {
        let slot = self
            .index
            .read_slot(&mut self.io, id)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "record is not indexed"))?;

        self.data
            .read_record(&mut self.io, slot.offset, slot.total_len, id, out)?;

        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.data.paged_file.flush(&mut self.io)?;
        self.io.sync(&self.data.paged_file.file)?;

        self.index.paged_file.flush(&mut self.io)?;
        self.io.sync(&self.index.paged_file.file)
    }

    pub fn io_counters(&self) -> crate::io::IoCounters {
        self.io.counters()
    }

    pub fn reset_io_counters(&mut self) {
        self.io.reset_counters();
    }

    pub fn pending_bytes(&self) -> usize {
        self.data.paged_file.pending_bytes() + self.index.paged_file.pending_bytes()
    }
}
