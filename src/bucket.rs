use crate::data::DataFile;
use crate::format::{PAGE_SIZE_U64, SLOT_SIZE, Slot, page_up};
use crate::heap::HeapConfig;
use crate::index::IndexFile;
use crate::io::BlockIo;
use crate::placement::Placement;

use std::collections::HashMap;
use std::io;

struct Extent {
    start: u64,
    size: u64,
    used: u64,
}

struct BucketState {
    extents: Vec<Extent>,
    region_start: u64,
    region_used: u64,
}

pub struct BucketPlacement {
    buckets: HashMap<u64, BucketState>,
    next_data: u64,
    next_region: u64,
}

fn extent_size(config: &HeapConfig, total_len: u64) -> u64 {
    page_up(total_len.max(config.min_extent))
}

impl BucketPlacement {
    fn place_bytes(
        &mut self,
        config: &HeapConfig,
        data: &mut DataFile,
        bucket: u64,
        total_len: u64,
    ) -> io::Result<u64> {
        let state = self.buckets.entry(bucket).or_insert(BucketState {
            extents: Vec::new(),
            region_start: 0,
            region_used: config.region_slots,
        });

        if let Some(extent) = state.extents.last_mut() {
            if extent.used + total_len <= extent.size {
                let offset = extent.start + extent.used;

                extent.used += total_len;

                return Ok(offset);
            }
        }

        let size = extent_size(config, total_len);
        let start = self.next_data;

        self.next_data += size;
        data.paged_file
            .ensure_alloc(self.next_data, config.growth)?;
        state.extents.push(Extent {
            start,
            size,
            used: total_len,
        });

        Ok(start)
    }
}

impl Placement for BucketPlacement {
    const WITH_BUCKET: bool = true;
    const LAYOUT: u8 = crate::format::LAYOUT_BUCKET;

    fn open(
        io: &mut impl BlockIo,
        config: &HeapConfig,
        data: &mut DataFile,
        index: &mut IndexFile,
        created: bool,
    ) -> io::Result<Self> {
        if created {
            data.paged_file
                .ensure_alloc(config.initial_size, config.growth)?;

            return Ok(BucketPlacement {
                buckets: HashMap::new(),
                next_data: PAGE_SIZE_U64,
                next_region: 0,
            });
        }

        let size = index.paged_file.size()?;
        let mut placement = BucketPlacement {
            buckets: HashMap::new(),
            next_data: PAGE_SIZE_U64,
            next_region: 0,
        };
        let mut id = 0u64;

        while Slot::file_offset(id) + SLOT_SIZE as u64 <= size {
            if let Some(slot) = index.read_slot(io, id)? {
                let header = data.read_header(io, slot.offset, id)?;
                let region_start = id - id % config.region_slots;
                let used = id - region_start + 1;
                let state = placement
                    .buckets
                    .entry(header.bucket)
                    .or_insert(BucketState {
                        extents: Vec::new(),
                        region_start,
                        region_used: used,
                    });

                if region_start >= state.region_start {
                    state.region_start = region_start;
                    state.region_used = state.region_used.max(used);
                }
                placement.next_region = placement
                    .next_region
                    .max(region_start + config.region_slots);
                placement.next_data = placement.next_data.max(slot.offset + slot.total_len);
            }

            id += 1;
        }

        data.paged_file.note_alloc(data.paged_file.size()?);

        Ok(placement)
    }

    fn alloc(
        &mut self,
        config: &HeapConfig,
        data: &mut DataFile,
        bucket: u64,
        total_len: u64,
    ) -> io::Result<(u64, u64)> {
        let needs_region = self
            .buckets
            .get(&bucket)
            .map(|state| state.region_used >= config.region_slots)
            .unwrap_or(true);

        if needs_region {
            let start = self.next_region;

            self.next_region += config.region_slots;

            let state = self.buckets.entry(bucket).or_insert(BucketState {
                extents: Vec::new(),
                region_start: start,
                region_used: 0,
            });

            state.region_start = start;
            state.region_used = 0;
        }

        let state = self.buckets.get_mut(&bucket).unwrap();
        let id = state.region_start + state.region_used;

        state.region_used += 1;

        let offset = self.place_bytes(config, data, bucket, total_len)?;

        Ok((offset, id))
    }

    fn used_bytes(&self) -> u64 {
        self.next_data - PAGE_SIZE_U64
    }
}
