use crate::data::{DataFile, PagedFile};
use crate::format::{
    PAGE_SIZE_U64, REGISTRY_ROW_SIZE, RegistryRow, SLOT_SIZE, Slot, page_down, page_up,
};
use crate::heap::HeapConfig;
use crate::index::IndexFile;
use crate::io::BlockIo;
use crate::placement::Placement;

use std::collections::HashMap;
use std::io;

#[derive(Clone, Copy)]
struct Extent {
    start: u64,
    size: u64,
    used: u64,
}

impl Extent {
    fn end(&self) -> u64 {
        self.start + self.size
    }
}

struct BucketState {
    id: u64,
    data: Vec<Extent>,
    slots: Vec<Extent>,
}

pub struct BucketPlacement {
    states: Vec<BucketState>,
    bucket_indices: HashMap<u64, usize>,
    extent_grant_count: u64,
    file_size: u64,
    next_region: u64,
    registry_end: u64,
}

fn required_extent_size(config: &HeapConfig, total_len: u64) -> u64 {
    page_up(total_len).max(config.min_extent)
}

impl BucketPlacement {
    fn push_row(
        &mut self,
        io: &mut impl BlockIo,
        registry: &mut PagedFile,
        row: RegistryRow,
    ) -> io::Result<()> {
        let mut buffer = [0u8; REGISTRY_ROW_SIZE];

        row.encode(&mut buffer);
        registry.stage(io, self.registry_end, &[&buffer])?;
        self.registry_end += REGISTRY_ROW_SIZE as u64;

        Ok(())
    }

    fn grant_extent(
        &mut self,
        io: &mut impl BlockIo,
        config: &HeapConfig,
        data: &mut DataFile,
        registry: &mut PagedFile,
        bucket_index: usize,
        need: u64,
    ) -> io::Result<()> {
        let extent = loop {
            let grant = self.extent_grant_count;

            self.extent_grant_count += 1;

            if grant == 0 {
                break Extent {
                    start: PAGE_SIZE_U64,
                    size: self.file_size - PAGE_SIZE_U64,
                    used: 0,
                };
            }

            let level = 1u64 << (63 - grant.leading_zeros());
            let victim_index = (grant - level) as usize;

            if victim_index >= self.states.len() {
                let grow = need.max(config.growth);

                data.paged_file.ensure_alloc(self.file_size + grow, grow)?;

                let extent = Extent {
                    start: self.file_size,
                    size: grow,
                    used: 0,
                };

                self.file_size += grow;
                break extent;
            }

            let victim = self.states[victim_index]
                .data
                .last_mut()
                .expect("bucket without extent");
            let free = victim.size.saturating_sub(page_up(victim.used));

            if free >= need {
                let cut = page_down(free / 2).max(need);

                victim.size -= cut;
                break Extent {
                    start: victim.start + victim.size,
                    size: cut,
                    used: 0,
                };
            }
        };
        let bucket = self.states[bucket_index].id;

        self.push_row(
            io,
            registry,
            RegistryRow {
                kind: b'D',
                bucket,
                start: extent.start,
                size: extent.size,
            },
        )?;
        self.states[bucket_index].data.push(extent);

        Ok(())
    }

    fn grant_region(
        &mut self,
        io: &mut impl BlockIo,
        config: &HeapConfig,
        registry: &mut PagedFile,
        bucket_index: usize,
    ) -> io::Result<()> {
        let region = Extent {
            start: self.next_region,
            size: config.region_slots,
            used: 0,
        };

        self.next_region += config.region_slots;

        let bucket = self.states[bucket_index].id;

        self.push_row(
            io,
            registry,
            RegistryRow {
                kind: b'S',
                bucket,
                start: region.start,
                size: region.size,
            },
        )?;
        self.states[bucket_index].slots.push(region);

        Ok(())
    }

    fn bucket_index(
        &mut self,
        io: &mut impl BlockIo,
        config: &HeapConfig,
        data: &mut DataFile,
        registry: &mut PagedFile,
        bucket: u64,
        need: u64,
    ) -> io::Result<usize> {
        if let Some(&index) = self.bucket_indices.get(&bucket) {
            return Ok(index);
        }

        let index = self.states.len();

        self.states.push(BucketState {
            id: bucket,
            data: Vec::new(),
            slots: Vec::new(),
        });
        self.bucket_indices.insert(bucket, index);

        self.grant_extent(io, config, data, registry, index, need)?;
        self.grant_region(io, config, registry, index)?;

        Ok(index)
    }

    fn place_bytes(
        &mut self,
        io: &mut impl BlockIo,
        config: &HeapConfig,
        data: &mut DataFile,
        registry: &mut PagedFile,
        bucket_index: usize,
        total_len: u64,
    ) -> io::Result<u64> {
        let extent = self.states[bucket_index]
            .data
            .last()
            .copied()
            .expect("bucket without extent");

        if extent.used + total_len <= extent.size {
            let offset = extent.start + extent.used;

            self.states[bucket_index].data.last_mut().unwrap().used += total_len;

            return Ok(offset);
        }

        self.grant_extent(
            io,
            config,
            data,
            registry,
            bucket_index,
            required_extent_size(config, total_len),
        )?;

        let extent = self.states[bucket_index].data.last_mut().unwrap();

        extent.used = total_len;

        Ok(extent.start)
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
        registry: Option<&mut PagedFile>,
        created: bool,
    ) -> io::Result<Self> {
        let registry = registry.expect("bucket layout requires registry");
        let mut placement = BucketPlacement {
            states: Vec::new(),
            bucket_indices: HashMap::new(),
            extent_grant_count: 0,
            file_size: config.initial_size,
            next_region: 0,
            registry_end: PAGE_SIZE_U64,
        };

        if created {
            data.paged_file
                .ensure_alloc(config.initial_size, config.growth)?;

            return Ok(placement);
        }

        let registry_size = registry.size()?;

        if registry_size > PAGE_SIZE_U64 {
            let (buffer, buffer_offset) =
                registry.read_aligned(io, PAGE_SIZE_U64, registry_size - PAGE_SIZE_U64)?;

            for chunk in buffer[buffer_offset..].chunks_exact(REGISTRY_ROW_SIZE) {
                let Some(row) = RegistryRow::decode(chunk) else {
                    if chunk.iter().all(|&byte| byte == 0) {
                        break;
                    }

                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid registry row",
                    ));
                };
                let bucket_index = match placement.bucket_indices.get(&row.bucket) {
                    Some(&index) => index,
                    None => {
                        placement.states.push(BucketState {
                            id: row.bucket,
                            data: Vec::new(),
                            slots: Vec::new(),
                        });

                        let index = placement.states.len() - 1;

                        placement.bucket_indices.insert(row.bucket, index);

                        index
                    }
                };

                placement.extent_grant_count += 1;
                placement.registry_end += REGISTRY_ROW_SIZE as u64;

                match row.kind {
                    b'D' => {
                        'shrink: for state in &mut placement.states {
                            for extent in &mut state.data {
                                if row.start >= extent.start && row.start + row.size <= extent.end()
                                {
                                    extent.size = row.start - extent.start;
                                    break 'shrink;
                                }
                            }
                        }

                        placement.file_size = placement.file_size.max(row.start + row.size);
                        placement.states[bucket_index].data.push(Extent {
                            start: row.start,
                            size: row.size,
                            used: 0,
                        });
                    }
                    b'S' => {
                        placement.next_region = placement.next_region.max(row.start + row.size);
                        placement.states[bucket_index].slots.push(Extent {
                            start: row.start,
                            size: row.size,
                            used: 0,
                        });
                    }
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "invalid registry row kind",
                        ));
                    }
                }
            }

            registry.recycle(buffer);
        }

        placement.file_size = placement.file_size.max(data.paged_file.size()?);
        data.paged_file.note_alloc(placement.file_size);

        let index_size = index.paged_file.size()?;
        let mut id = 0u64;

        while Slot::file_offset(id) + SLOT_SIZE as u64 <= index_size {
            if let Some(slot) = index.read_slot(io, id)? {
                let header = data.read_header(io, slot.offset, id)?;

                if let Some(&bucket_index) = placement.bucket_indices.get(&header.bucket) {
                    if let Some(extent) = placement.states[bucket_index]
                        .data
                        .iter_mut()
                        .find(|extent| extent.start <= slot.offset && slot.offset < extent.end())
                    {
                        extent.used = extent.used.max(slot.offset + slot.total_len - extent.start);
                    }
                    if let Some(region) = placement.states[bucket_index]
                        .slots
                        .iter_mut()
                        .find(|region| region.start <= id && id < region.end())
                    {
                        region.used = region.used.max(id - region.start + 1);
                    }
                }
            }

            id += 1;
        }

        Ok(placement)
    }

    fn alloc(
        &mut self,
        io: &mut impl BlockIo,
        config: &HeapConfig,
        data: &mut DataFile,
        registry: Option<&mut PagedFile>,
        bucket: u64,
        total_len: u64,
    ) -> io::Result<(u64, u64)> {
        let registry = registry.expect("bucket layout requires registry");
        let bucket_index = self.bucket_index(
            io,
            config,
            data,
            registry,
            bucket,
            required_extent_size(config, total_len),
        )?;
        let needs_region = self.states[bucket_index]
            .slots
            .last()
            .map(|region| region.used >= region.size)
            .unwrap_or(true);

        if needs_region {
            self.grant_region(io, config, registry, bucket_index)?;
        }

        let region = self.states[bucket_index].slots.last_mut().unwrap();
        let id = region.start + region.used;

        region.used += 1;

        let offset = self.place_bytes(io, config, data, registry, bucket_index, total_len)?;

        Ok((offset, id))
    }

    fn used_bytes(&self) -> u64 {
        self.states
            .iter()
            .flat_map(|state| &state.data)
            .map(|extent| extent.used)
            .sum()
    }
}
