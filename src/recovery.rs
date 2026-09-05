use crate::crc32::crc32c;
use crate::data::PagedFile;
use crate::error::Result;
use crate::format::{
    LAYOUT_BUCKET, PAGE_SIZE_U64, RECORD_MAGIC, REGISTRY_ROW_SIZE, RecordHeader, RegistryRow, Slot,
    header_len,
};
use crate::heap::HeapConfig;
use crate::io::BlockIo;

use std::collections::HashMap;
use std::path::Path;

const RECOVERY_SCAN_CHUNK_SIZE: u64 = 1 << 20;

#[derive(Debug, Default, Clone, Copy)]
pub struct WalkStats {
    pub records: u64,
    pub resyncs: u64,
    pub bad_payload: u64,
    pub last_end: u64,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RecoveryReport {
    pub rebuilt: bool,
    pub restored_slots: u64,
    pub rebuild: WalkStats,
}

#[allow(clippy::too_many_arguments)]
pub fn walk(
    io: &mut impl BlockIo,
    paged_file: &mut PagedFile,
    with_bucket: bool,
    integrity: bool,
    start_offset: u64,
    end_offset: u64,
    allow_resync: bool,
    mut visitor: impl FnMut(u64, &RecordHeader, bool) -> bool,
) -> Result<WalkStats> {
    let header_size = header_len(with_bucket);
    let mut stats = WalkStats {
        last_end: start_offset,
        ..Default::default()
    };
    let mut buffer: Vec<u8> = Vec::new();
    let mut buffer_start = start_offset;
    let mut position = start_offset;

    paged_file.flush(io)?;

    macro_rules! ensure_available {
        ($needed_bytes:expr) => {{
            let needed_bytes: u64 = $needed_bytes;
            let mut available_bytes = buffer_start + buffer.len() as u64 - position;
            let mut enough_data = true;

            while available_bytes < needed_bytes {
                let read_offset = buffer_start + buffer.len() as u64;

                if read_offset >= end_offset {
                    enough_data = false;
                    break;
                }

                let read_len = RECOVERY_SCAN_CHUNK_SIZE.min(end_offset - read_offset);
                let (chunk, chunk_offset) = paged_file.read_aligned(io, read_offset, read_len)?;

                buffer.extend_from_slice(&chunk[chunk_offset..chunk_offset + read_len as usize]);
                available_bytes += read_len;
            }

            enough_data
        }};
    }

    loop {
        if position - buffer_start > 8 * RECOVERY_SCAN_CHUNK_SIZE {
            buffer.drain(..(position - buffer_start) as usize);
            buffer_start = position;
        }

        if position + header_size as u64 > end_offset || !ensure_available!(header_size as u64) {
            break;
        }

        let buffer_offset = (position - buffer_start) as usize;
        let header = RecordHeader::decode(&buffer[buffer_offset..], with_bucket);
        let header = match header {
            Some(header) if position + header_size as u64 + header.len <= end_offset => header,
            _ => {
                if !allow_resync {
                    break;
                }

                let record_magic = RECORD_MAGIC.to_le_bytes();
                let mut search_offset = position + 1;

                'resync: loop {
                    if search_offset - buffer_start > 8 * RECOVERY_SCAN_CHUNK_SIZE {
                        let drain_len = (search_offset - buffer_start - 3) as usize;

                        buffer.drain(..drain_len);
                        buffer_start += drain_len as u64;
                    }

                    if !ensure_available!(search_offset - position + 4) {
                        return Ok(stats);
                    }

                    let search_index = (search_offset - buffer_start) as usize;

                    match buffer[search_index..]
                        .windows(4)
                        .position(|window| window == record_magic)
                    {
                        Some(relative_offset) => {
                            search_offset += relative_offset as u64;
                            break 'resync;
                        }
                        None => search_offset = buffer_start + buffer.len() as u64 - 3,
                    }
                }

                stats.resyncs += 1;
                position = search_offset;
                continue;
            }
        };

        if !ensure_available!(header_size as u64 + header.len) {
            break;
        }

        let buffer_offset = (position - buffer_start) as usize;
        let payload_end = buffer_offset + header_size + header.len as usize;
        let payload = &buffer[buffer_offset + header_size..payload_end];
        let payload_valid = !integrity || crc32c(payload) == header.crc;

        stats.records += 1;
        if !payload_valid {
            stats.bad_payload += 1;
        }

        let continue_walk = visitor(position, &header, payload_valid);

        position += header_size as u64 + header.len;
        stats.last_end = position;

        if !continue_walk {
            break;
        }
    }

    Ok(stats)
}

pub fn rebuild(
    dir: &Path,
    io: &mut impl BlockIo,
    layout: u8,
    integrity: bool,
    config: &HeapConfig,
) -> Result<WalkStats> {
    let with_bucket = layout == LAYOUT_BUCKET;
    let mut data_file = PagedFile::open(&dir.join("data.hs"), b'D', layout, io)?;
    let data_size = data_file.size()?;
    let header_size = header_len(with_bucket) as u64;
    let mut latest_records: HashMap<u64, (u64, u64, u16, u64)> = HashMap::new();
    let mut bucket_runs: Vec<(u64, u64, u64)> = Vec::new();
    let stats = walk(
        io,
        &mut data_file,
        with_bucket,
        integrity,
        PAGE_SIZE_U64,
        data_size,
        true,
        |offset, header, payload_valid| {
            if !payload_valid {
                return true;
            }

            let record_len = header_size + header.len;

            match bucket_runs.last_mut() {
                Some(run) if run.0 == header.bucket && run.2 == offset => {
                    run.2 = offset + record_len
                }
                _ => bucket_runs.push((header.bucket, offset, offset + record_len)),
            }

            let entry = latest_records.entry(header.id).or_insert((
                offset,
                record_len,
                header.version,
                header.bucket,
            ));

            if header.version >= entry.2 {
                *entry = (offset, record_len, header.version, header.bucket);
            }

            true
        },
    )?;
    let mut index_file = crate::index::IndexFile {
        paged_file: PagedFile::create(&dir.join("index.hs"), b'I', layout, io)?,
    };
    let mut ids: Vec<u64> = latest_records.keys().copied().collect();

    ids.sort_unstable();

    for id in ids {
        let (offset, record_len, _, _) = latest_records[&id];

        index_file.stage_slot(
            io,
            id,
            Some(Slot {
                offset,
                total_len: record_len,
            }),
        )?;

        if index_file.paged_file.pending_bytes() > 4 << 20 {
            index_file.paged_file.flush(io)?;
        }
    }

    index_file.paged_file.flush(io)?;
    io.sync(&index_file.paged_file.file)?;

    if with_bucket {
        let mut registry_file = PagedFile::create(&dir.join("registry.hs"), b'R', layout, io)?;
        let mut registry_end = PAGE_SIZE_U64;
        let mut row_buffer = [0u8; REGISTRY_ROW_SIZE];

        for &(bucket, start, run_end) in &bucket_runs {
            RegistryRow {
                kind: b'D',
                bucket,
                start: crate::format::page_down(start),
                size: crate::format::page_up(run_end) - crate::format::page_down(start),
            }
            .encode(&mut row_buffer);
            registry_file.stage(io, registry_end, &[&row_buffer])?;
            registry_end += REGISTRY_ROW_SIZE as u64;
        }

        let mut region_buckets: HashMap<u64, u64> = HashMap::new();

        for (&id, &(_, _, _, bucket)) in latest_records.iter() {
            region_buckets
                .entry(id / config.region_slots * config.region_slots)
                .or_insert(bucket);
        }

        let mut regions: Vec<(u64, u64)> = region_buckets.into_iter().collect();

        regions.sort_unstable();

        for &(region_start, bucket) in &regions {
            RegistryRow {
                kind: b'S',
                bucket,
                start: region_start,
                size: config.region_slots,
            }
            .encode(&mut row_buffer);
            registry_file.stage(io, registry_end, &[&row_buffer])?;
            registry_end += REGISTRY_ROW_SIZE as u64;
        }

        registry_file.flush(io)?;
        io.sync(&registry_file.file)?;
    }

    Ok(stats)
}
