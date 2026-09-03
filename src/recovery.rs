use crate::crc32::crc32c;
use crate::data::PagedFile;
use crate::error::Result;
use crate::format::{RecordHeader, header_len};
use crate::io::BlockIo;

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
    _allow_resync: bool,
    mut visitor: impl FnMut(u64, &RecordHeader, bool) -> bool,
) -> Result<WalkStats> {
    let header_size = header_len(with_bucket) as u64;
    let mut stats = WalkStats {
        last_end: start_offset,
        ..WalkStats::default()
    };
    let mut offset = start_offset;

    while offset + header_size <= end_offset {
        let (header_buffer, buffer_offset) = paged_file.read_aligned(io, offset, header_size)?;
        let header = RecordHeader::decode(&header_buffer[buffer_offset..], with_bucket);

        paged_file.recycle(header_buffer);

        let Some(header) = header else {
            break;
        };
        let record_len = header_size + header.len;

        if offset + record_len > end_offset {
            break;
        }

        let (record_buffer, buffer_offset) = paged_file.read_aligned(io, offset, record_len)?;
        let payload = &record_buffer
            [buffer_offset + header_size as usize..buffer_offset + record_len as usize];
        let payload_valid = !integrity || crc32c(payload) == header.crc;

        stats.records += 1;
        stats.bad_payload += u64::from(!payload_valid);

        let continue_walk = visitor(offset, &header, payload_valid);

        paged_file.recycle(record_buffer);

        offset += record_len;
        stats.last_end = offset;

        if !continue_walk {
            break;
        }
    }

    Ok(stats)
}
