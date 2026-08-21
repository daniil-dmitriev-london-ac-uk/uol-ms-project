use crate::data::{DataFile, PagedFile};
use crate::format::{SLOT_SIZE, Slot};
use crate::index::IndexFile;
use crate::io::BlockIo;
use crate::placement::Placement;

use std::io;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct HeapConfig {
    pub integrity: bool,
    pub initial_size: u64,
    pub growth: u64,
    pub min_extent: u64,
    pub region_slots: u64,
}

impl Default for HeapConfig {
    fn default() -> Self {
        HeapConfig {
            integrity: true,
            initial_size: 64 << 20,
            growth: 64 << 20,
            min_extent: 1 << 20,
            region_slots: 4096,
        }
    }
}

pub struct Heap<I: BlockIo, P: Placement> {
    io: I,
    placement: P,
    data: DataFile,
    index: IndexFile,
    registry: Option<PagedFile>,
    config: HeapConfig,
}

impl<I: BlockIo, P: Placement> Heap<I, P> {
    pub fn open(dir: &Path, mut io: I, config: HeapConfig) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;

        let data_path = dir.join("data.hs");
        let index_path = dir.join("index.hs");
        let created = !data_path.exists();
        let data_paged_file = if created {
            PagedFile::create(&data_path, b'D', P::LAYOUT, &mut io)?
        } else {
            PagedFile::open(&data_path, b'D', P::LAYOUT, &mut io)?
        };
        let index_paged_file = if created {
            PagedFile::create(&index_path, b'I', P::LAYOUT, &mut io)?
        } else {
            PagedFile::open(&index_path, b'I', P::LAYOUT, &mut io)?
        };
        let registry_path = dir.join("registry.hs");
        let registry = if P::WITH_BUCKET {
            Some(if created {
                PagedFile::create(&registry_path, b'R', P::LAYOUT, &mut io)?
            } else {
                PagedFile::open(&registry_path, b'R', P::LAYOUT, &mut io)?
            })
        } else {
            None
        };
        let mut data = DataFile {
            paged_file: data_paged_file,
            with_bucket: P::WITH_BUCKET,
            integrity: config.integrity,
        };
        let mut index = IndexFile {
            paged_file: index_paged_file,
        };
        let mut registry = registry;
        let placement = P::open(
            &mut io,
            &config,
            &mut data,
            &mut index,
            registry.as_mut(),
            created,
        )?;

        Ok(Heap {
            io,
            placement,
            data,
            index,
            registry,
            config,
        })
    }

    pub fn insert(&mut self, payload: &[u8]) -> io::Result<u64> {
        self.insert_into(0, payload)
    }

    pub fn insert_into(&mut self, bucket: u64, payload: &[u8]) -> io::Result<u64> {
        let total = self.data.record_header_len() + payload.len() as u64;
        let (off, id) = self.placement.alloc(
            &mut self.io,
            &self.config,
            &mut self.data,
            self.registry.as_mut(),
            bucket,
            total,
        )?;

        self.index
            .paged_file
            .ensure_alloc(Slot::file_offset(id) + SLOT_SIZE as u64, 1 << 20)?;

        self.data
            .stage_record(&mut self.io, off, bucket, id, 1, payload)?;

        self.index.stage_slot(
            &mut self.io,
            id,
            Slot {
                offset: off,
                total_len: total,
            },
        )?;

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

    pub fn read_batch(&mut self, ids: &[u64], out: &mut Vec<Vec<u8>>) -> io::Result<()> {
        let slots = self.index.read_slots(&mut self.io, ids)?;
        let mut items = Vec::with_capacity(ids.len());

        for (&id, slot) in ids.iter().zip(slots) {
            let slot = slot
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "record is not indexed"))?;

            items.push((slot.offset, slot.total_len, id));
        }

        self.data.read_many(&mut self.io, &items, out)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        if let Some(registry) = &mut self.registry {
            registry.flush(&mut self.io)?;
            self.io.sync(&registry.file)?;
        }

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
        self.data.paged_file.pending_bytes()
            + self.index.paged_file.pending_bytes()
            + self
                .registry
                .as_ref()
                .map(PagedFile::pending_bytes)
                .unwrap_or(0)
    }

    pub fn used_bytes(&self) -> u64 {
        self.placement.used_bytes()
    }
}
