use crate::data::{DataFile, PagedFile};
use crate::error::{HeapError, Result};
use crate::format::{
    PAGE_SIZE_U64, REGISTRY_ROW_SIZE, RegistryRow, SLOT_SIZE, Slot, SlotState, page_down, page_up,
};
use crate::heap::HeapConfig;
use crate::index::IndexFile;
use crate::io::BlockIo;
use crate::placement::{OpenStats, Placement};
use crate::recovery::walk;

use std::collections::HashMap;

#[derive(Debug, Clone, Copy)]
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
    ) -> Result<()> {
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
    ) -> Result<()> {
        let extent = loop {
            let grant_number = self.extent_grant_count;

            self.extent_grant_count += 1;

            if grant_number == 0 {
                break Extent {
                    start: PAGE_SIZE_U64,
                    size: self.file_size - PAGE_SIZE_U64,
                    used: 0,
                };
            }

            let section_base = 1u64 << (63 - grant_number.leading_zeros());
            let victim_index = (grant_number - section_base) as usize;

            if victim_index >= self.states.len() {
                let grow = need.max(config.growth);

                data.paged_file.ensure_alloc(self.file_size + grow, grow)?;

                let new_extent = Extent {
                    start: self.file_size,
                    size: grow,
                    used: 0,
                };

                self.file_size += grow;
                break new_extent;
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
    ) -> Result<()> {
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
    ) -> Result<usize> {
        if let Some(&bucket_index) = self.bucket_indices.get(&bucket) {
            return Ok(bucket_index);
        }

        let bucket_index = self.states.len();

        self.states.push(BucketState {
            id: bucket,
            data: Vec::new(),
            slots: Vec::new(),
        });
        self.bucket_indices.insert(bucket, bucket_index);

        self.grant_extent(io, config, data, registry, bucket_index, need)?;
        self.grant_region(io, config, registry, bucket_index)?;

        Ok(bucket_index)
    }

    fn place_bytes(
        &mut self,
        io: &mut impl BlockIo,
        config: &HeapConfig,
        data: &mut DataFile,
        registry: &mut PagedFile,
        bucket_index: usize,
        total_len: u64,
    ) -> Result<u64> {
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
    ) -> Result<(Self, OpenStats)> {
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

            return Ok((placement, OpenStats { restored: 0 }));
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

                    return Err(HeapError::Corrupt {
                        what: "registry row",
                        offset: placement.registry_end,
                    });
                };
                let bucket_index = match placement.bucket_indices.get(&row.bucket) {
                    Some(&bucket_index) => bucket_index,
                    None => {
                        placement.states.push(BucketState {
                            id: row.bucket,
                            data: Vec::new(),
                            slots: Vec::new(),
                        });
                        placement
                            .bucket_indices
                            .insert(row.bucket, placement.states.len() - 1);

                        placement.states.len() - 1
                    }
                };

                placement.extent_grant_count += 1;
                placement.registry_end += REGISTRY_ROW_SIZE as u64;

                match row.kind {
                    b'D' => {
                        'shrink: for bucket_state in &mut placement.states {
                            for extent in &mut bucket_state.data {
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
                        return Err(HeapError::Corrupt {
                            what: "registry row kind",
                            offset: placement.registry_end,
                        });
                    }
                }
            }
        }

        placement.file_size = placement.file_size.max(data.paged_file.size()?);
        data.paged_file.note_alloc(placement.file_size);

        let mut restored = 0u64;
        let header_size = data.record_header_len();

        for bucket_index in 0..placement.states.len() {
            let mut known: HashMap<u64, SlotState> = HashMap::new();
            let region_count = placement.states[bucket_index].slots.len();

            for region_index in 0..region_count {
                let region = placement.states[bucket_index].slots[region_index];
                let (buffer, buffer_offset) = index.paged_file.read_aligned(
                    io,
                    Slot::file_offset(region.start),
                    region.size * SLOT_SIZE as u64,
                )?;
                let mut last_used = 0u64;

                for slot_index in 0..region.size {
                    let slot_bytes =
                        &buffer[buffer_offset + (slot_index * SLOT_SIZE as u64) as usize..];
                    let slot_state = Slot::state(&slot_bytes[..SLOT_SIZE]);

                    if slot_state != SlotState::Empty {
                        known.insert(region.start + slot_index, slot_state);
                        last_used = slot_index + 1;
                    }
                }

                placement.states[bucket_index].slots[region_index].used =
                    if region_index + 1 == region_count {
                        last_used
                    } else {
                        region.size
                    };
            }

            let extent_count = placement.states[bucket_index].data.len();

            for extent_index in 0..extent_count.saturating_sub(1) {
                let size = placement.states[bucket_index].data[extent_index].size;

                placement.states[bucket_index].data[extent_index].used = size;
            }

            if let Some(last_extent) = placement.states[bucket_index].data.last().copied() {
                let mut found: Vec<(u64, u64, u16, u64)> = Vec::new();
                let mut used = 0u64;

                walk(
                    io,
                    &mut data.paged_file,
                    true,
                    data.integrity,
                    last_extent.start,
                    last_extent.end(),
                    false,
                    |offset, header, payload_valid| {
                        if !payload_valid {
                            return false;
                        }

                        used = offset + header.len + header_size - last_extent.start;
                        found.push((header.id, offset, header.version, header.len + header_size));

                        true
                    },
                )?;

                for slot_state in known.values() {
                    if let SlotState::Live(slot) | SlotState::Deleted(slot) = slot_state {
                        if slot.offset >= last_extent.start
                            && slot.offset + slot.total_len <= last_extent.end()
                        {
                            used = used.max(slot.offset + slot.total_len - last_extent.start);
                        }
                    }
                }

                placement.states[bucket_index].data.last_mut().unwrap().used = used;

                let mut max_id: Option<u64> = known.keys().copied().max();
                let mut best: HashMap<u64, (u64, u16, u64)> = HashMap::new();

                for (id, offset, version, record_len) in found {
                    max_id = Some(max_id.map_or(id, |current_max| current_max.max(id)));

                    let entry = best.entry(id).or_insert((offset, version, record_len));

                    if version >= entry.1 {
                        *entry = (offset, version, record_len);
                    }
                }

                for (id, (offset, version, record_len)) in best {
                    match known.get(&id) {
                        Some(SlotState::Deleted(_)) => continue,
                        Some(SlotState::Live(slot)) if slot.offset == offset => continue,
                        Some(SlotState::Live(slot)) => {
                            if let Ok(existing_header) = data.read_header(io, slot.offset, id) {
                                if existing_header.version >= version {
                                    continue;
                                }
                            }
                        }
                        _ => {}
                    }

                    index.stage_slot(
                        io,
                        id,
                        Some(Slot {
                            offset: offset,
                            total_len: record_len,
                        }),
                    )?;
                    restored += 1;
                }

                let region = placement.states[bucket_index].slots.last_mut().unwrap();

                if let Some(max_seen_id) = max_id {
                    if max_seen_id >= region.start {
                        region.used = region.used.max(max_seen_id - region.start + 1);
                    }
                }
            }
        }

        if restored > 0 {
            index.paged_file.flush(io)?;
            io.sync(&index.paged_file.file)?;
        }

        Ok((placement, OpenStats { restored }))
    }

    fn alloc(
        &mut self,
        io: &mut impl BlockIo,
        config: &HeapConfig,
        data: &mut DataFile,
        registry: Option<&mut PagedFile>,
        bucket: u64,
        total_len: u64,
    ) -> Result<(u64, u64)> {
        let registry = registry.expect("bucket layout requires registry");
        let bucket_index = self.bucket_index(
            io,
            config,
            data,
            registry,
            bucket,
            required_extent_size(config, total_len),
        )?;
        let offset = self.place_bytes(io, config, data, registry, bucket_index, total_len)?;
        let region = self.states[bucket_index]
            .slots
            .last()
            .copied()
            .expect("bucket without region");
        let id = if region.used < region.size {
            self.states[bucket_index].slots.last_mut().unwrap().used += 1;

            region.start + region.used
        } else {
            self.grant_region(io, config, registry, bucket_index)?;

            let region = self.states[bucket_index].slots.last_mut().unwrap();

            region.used = 1;

            region.start
        };

        Ok((offset, id))
    }

    fn alloc_update(
        &mut self,
        io: &mut impl BlockIo,
        config: &HeapConfig,
        data: &mut DataFile,
        registry: Option<&mut PagedFile>,
        bucket: u64,
        total_len: u64,
    ) -> Result<u64> {
        let registry = registry.expect("bucket layout requires registry");
        let bucket_index = *self
            .bucket_indices
            .get(&bucket)
            .ok_or(HeapError::InvalidArg("update refers to unknown bucket"))?;

        self.place_bytes(io, config, data, registry, bucket_index, total_len)
    }

    fn used_bytes(&self) -> u64 {
        self.states
            .iter()
            .flat_map(|bucket_state| bucket_state.data.iter())
            .map(|extent| extent.used)
            .sum()
    }
}
