use crate::data::{DataFile, PagedFile};
use crate::error::{HeapError, Result};
use crate::format::Slot;
use crate::index::IndexFile;
use crate::io::BlockIo;
use crate::placement::Placement;
use crate::recovery::{RecoveryReport, rebuild};

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SyncPolicy {
    None,

    EveryN(u32),
}

#[derive(Debug, Clone)]
pub struct HeapConfig {
    pub sync_policy: SyncPolicy,

    pub integrity: bool,

    pub initial_size: u64,
    pub growth: u64,

    pub min_extent: u64,

    pub region_slots: u64,

    pub pending_limit: usize,
}

impl Default for HeapConfig {
    fn default() -> Self {
        HeapConfig {
            sync_policy: SyncPolicy::None,
            integrity: true,
            initial_size: 64 << 20,
            growth: 64 << 20,
            min_extent: 1 << 20,
            region_slots: 4096,
            pending_limit: 8 << 20,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct HeapStats {
    pub data_file_bytes: u64,
    pub data_used_bytes: u64,
    pub index_file_bytes: u64,
    pub registry_file_bytes: u64,
}

pub struct Heap<I: BlockIo, P: Placement> {
    io: I,
    placement: P,
    data: DataFile,
    index: IndexFile,
    registry: Option<PagedFile>,
    config: HeapConfig,
    dir: PathBuf,
    operations_since_sync: u32,
    armed: bool,
}

impl<I: BlockIo, P: Placement> Heap<I, P> {
    pub fn open(dir: &Path, mut io: I, config: HeapConfig) -> Result<(Self, RecoveryReport)> {
        std::fs::create_dir_all(dir)?;

        let data_path = dir.join("data.hs");
        let index_path = dir.join("index.hs");
        let registry_path = dir.join("registry.hs");
        let created = !data_path.exists();
        let mut report = RecoveryReport::default();
        let (data_paged_file, index_paged_file, registry) = if created {
            (
                PagedFile::create(&data_path, b'D', P::LAYOUT, &mut io)?,
                PagedFile::create(&index_path, b'I', P::LAYOUT, &mut io)?,
                if P::WITH_BUCKET {
                    Some(PagedFile::create(&registry_path, b'R', P::LAYOUT, &mut io)?)
                } else {
                    None
                },
            )
        } else {
            let metadata_is_broken = |path: &Path, kind: u8, io: &mut I| {
                !path.exists() || PagedFile::open(path, kind, P::LAYOUT, io).is_err()
            };

            if metadata_is_broken(&index_path, b'I', &mut io)
                || (P::WITH_BUCKET && metadata_is_broken(&registry_path, b'R', &mut io))
            {
                report.rebuilt = true;
                report.rebuild = rebuild(dir, &mut io, P::LAYOUT, config.integrity, &config)?;
            }

            (
                PagedFile::open(&data_path, b'D', P::LAYOUT, &mut io)?,
                PagedFile::open(&index_path, b'I', P::LAYOUT, &mut io)?,
                if P::WITH_BUCKET {
                    Some(PagedFile::open(&registry_path, b'R', P::LAYOUT, &mut io)?)
                } else {
                    None
                },
            )
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
        let (placement, open_stats) = match P::open(
            &mut io,
            &config,
            &mut data,
            &mut index,
            registry.as_mut(),
            created,
        ) {
            Ok(open_result) => open_result,
            Err(HeapError::Corrupt { .. }) if !created && !report.rebuilt => {
                report.rebuilt = true;
                report.rebuild = rebuild(dir, &mut io, P::LAYOUT, config.integrity, &config)?;

                let index_paged_file = PagedFile::open(&index_path, b'I', P::LAYOUT, &mut io)?;

                index = IndexFile {
                    paged_file: index_paged_file,
                };
                if P::WITH_BUCKET {
                    registry = Some(PagedFile::open(&registry_path, b'R', P::LAYOUT, &mut io)?);
                }

                P::open(
                    &mut io,
                    &config,
                    &mut data,
                    &mut index,
                    registry.as_mut(),
                    false,
                )?
            }
            Err(error) => return Err(error),
        };

        report.restored_slots = open_stats.restored;

        Ok((
            Heap {
                io,
                placement,
                data,
                index,
                registry,
                config,
                dir: dir.to_path_buf(),
                operations_since_sync: 0,
                armed: true,
            },
            report,
        ))
    }

    pub fn insert(&mut self, payload: &[u8]) -> Result<u64> {
        self.insert_into(0, payload)
    }

    pub fn insert_into(&mut self, bucket: u64, payload: &[u8]) -> Result<u64> {
        let total = self.data.record_header_len() + payload.len() as u64;
        let (off, id) = self.placement.alloc(
            &mut self.io,
            &self.config,
            &mut self.data,
            self.registry.as_mut(),
            bucket,
            total,
        )?;

        self.data
            .stage_record(&mut self.io, off, bucket, id, 1, payload)?;

        self.index.stage_slot(
            &mut self.io,
            id,
            Some(Slot {
                offset: off,
                total_len: total,
            }),
        )?;

        self.after_write()?;

        Ok(id)
    }

    pub fn read(&mut self, id: u64, out: &mut Vec<u8>) -> Result<()> {
        let slot = self
            .index
            .read_slot(&mut self.io, id)?
            .ok_or(HeapError::NotFound(id))?;

        self.data
            .read_record(&mut self.io, slot.offset, slot.total_len, id, out)?;

        Ok(())
    }

    pub fn read_batch(&mut self, ids: &[u64], out: &mut Vec<Vec<u8>>) -> Result<()> {
        let slots = self.index.read_slots(&mut self.io, ids)?;
        let mut items = Vec::with_capacity(ids.len());

        for (&id, slot) in ids.iter().zip(&slots) {
            let slot = slot.ok_or(HeapError::NotFound(id))?;

            items.push((slot.offset, slot.total_len, id));
        }

        self.data.read_many(&mut self.io, &items, out)
    }

    pub fn update(&mut self, id: u64, payload: &[u8]) -> Result<()> {
        let slot = self
            .index
            .read_slot(&mut self.io, id)?
            .ok_or(HeapError::NotFound(id))?;
        let old = self.data.read_header(&mut self.io, slot.offset, id)?;
        let total = self.data.record_header_len() + payload.len() as u64;
        let off = self.placement.alloc_update(
            &mut self.io,
            &self.config,
            &mut self.data,
            self.registry.as_mut(),
            old.bucket,
            total,
        )?;

        self.data.stage_record(
            &mut self.io,
            off,
            old.bucket,
            id,
            old.version.wrapping_add(1),
            payload,
        )?;

        self.index.stage_slot(
            &mut self.io,
            id,
            Some(Slot {
                offset: off,
                total_len: total,
            }),
        )?;

        self.after_write()
    }

    pub fn delete(&mut self, id: u64) -> Result<()> {
        let old = self
            .index
            .read_slot(&mut self.io, id)?
            .ok_or(HeapError::NotFound(id))?;

        self.index.stage_tombstone(&mut self.io, id, old)?;

        self.after_write()
    }

    pub fn flush(&mut self) -> Result<()> {
        self.commit()?;

        self.index.paged_file.flush(&mut self.io)?;
        self.io.sync(&self.index.paged_file.file)?;

        if let Some(registry_file) = &mut self.registry {
            self.io.sync(&registry_file.file)?;
        }

        Ok(())
    }

    fn commit(&mut self) -> Result<()> {
        if let Some(registry_file) = &mut self.registry {
            if registry_file.has_pending() {
                registry_file.flush(&mut self.io)?;
                self.io.sync(&registry_file.file)?;
            }
        }

        self.data.paged_file.flush(&mut self.io)?;
        self.io.sync(&self.data.paged_file.file)?;

        self.index.paged_file.flush(&mut self.io)?;

        Ok(())
    }

    fn after_write(&mut self) -> Result<()> {
        self.operations_since_sync = self.operations_since_sync.wrapping_add(1);

        if let SyncPolicy::EveryN(sync_interval) = self.config.sync_policy {
            if sync_interval > 0 && self.operations_since_sync.is_multiple_of(sync_interval) {
                return self.commit();
            }
        }

        if self.data.paged_file.pending_bytes() + self.index.paged_file.pending_bytes()
            > self.config.pending_limit
        {
            self.commit()?;
        }

        Ok(())
    }

    pub fn io_counters(&self) -> crate::io::IoCounters {
        self.io.counters()
    }

    pub fn reset_io_counters(&mut self) {
        self.io.reset_counters()
    }

    pub fn stats(&self) -> HeapStats {
        let file_size = |paged_file: &PagedFile| paged_file.size().unwrap_or(0);

        HeapStats {
            data_file_bytes: file_size(&self.data.paged_file),
            data_used_bytes: self.placement.used_bytes(),
            index_file_bytes: file_size(&self.index.paged_file),
            registry_file_bytes: self.registry.as_ref().map(&file_size).unwrap_or(0),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn simulate_crash(mut self) {
        self.armed = false;
    }
}

impl<I: BlockIo, P: Placement> Drop for Heap<I, P> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.flush();
        }
    }
}
