//! this module defines the durable byte format.

use crate::crc32::crc32c;

pub const PAGE: usize = 4096;
pub const PAGE_SIZE_U64: u64 = PAGE as u64;

pub const RECORD_MAGIC: u32 = 0x4452_4352;
pub const REGISTRY_MAGIC: u32 = 0x5847_4552;

pub const LAYOUT_APPEND: u8 = 0;
pub const LAYOUT_BUCKET: u8 = 1;

pub const SLOT_SIZE: usize = 16;
pub const REGISTRY_ROW_SIZE: usize = 64;

pub fn page_down(off: u64) -> u64 {
    off & !(PAGE_SIZE_U64 - 1)
}

pub fn page_up(off: u64) -> u64 {
    (off + PAGE_SIZE_U64 - 1) & !(PAGE_SIZE_U64 - 1)
}

pub struct FileHeader {
    pub kind: u8,
    pub layout: u8,
}

impl FileHeader {
    pub fn encode(&self, page: &mut [u8]) {
        page[..PAGE].fill(0);
        page[..4].copy_from_slice(b"HSF1");
        page[4] = self.kind;
        page[5] = self.layout;

        let crc = crc32c(&page[..8]);

        page[8..12].copy_from_slice(&crc.to_le_bytes());
    }

    pub fn decode(page: &[u8]) -> Option<FileHeader> {
        if &page[..4] != b"HSF1" {
            return None;
        }

        let crc = u32::from_le_bytes(page[8..12].try_into().unwrap());

        if crc != crc32c(&page[..8]) {
            return None;
        }

        Some(FileHeader {
            kind: page[4],
            layout: page[5],
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecordHeader {
    pub len: u64,
    pub bucket: u64,
    pub id: u64,
    pub version: u16,
    pub crc: u32,
}

pub const fn header_len(with_bucket: bool) -> usize {
    if with_bucket { 38 } else { 30 }
}

impl RecordHeader {
    pub fn encode(&self, with_bucket: bool, out: &mut [u8]) {
        out[..4].copy_from_slice(&RECORD_MAGIC.to_le_bytes());
        out[4..12].copy_from_slice(&self.len.to_le_bytes());

        let mut field_offset = 12;

        if with_bucket {
            out[12..20].copy_from_slice(&self.bucket.to_le_bytes());
            field_offset = 20;
        }
        out[field_offset..field_offset + 8].copy_from_slice(&self.id.to_le_bytes());
        out[field_offset + 8..field_offset + 10].copy_from_slice(&self.version.to_le_bytes());
        out[field_offset + 10..field_offset + 14].copy_from_slice(&self.crc.to_le_bytes());

        // header integrity is checked separately from payload integrity.
        let header_crc = crc32c(&out[..field_offset + 14]);

        out[field_offset + 14..field_offset + 18].copy_from_slice(&header_crc.to_le_bytes());
    }

    pub fn decode(buf: &[u8], with_bucket: bool) -> Option<RecordHeader> {
        let header_size = header_len(with_bucket);

        if buf.len() < header_size
            || u32::from_le_bytes(buf[..4].try_into().unwrap()) != RECORD_MAGIC
        {
            return None;
        }

        let stored = u32::from_le_bytes(buf[header_size - 4..header_size].try_into().unwrap());

        if stored != crc32c(&buf[..header_size - 4]) {
            return None;
        }

        let len = u64::from_le_bytes(buf[4..12].try_into().unwrap());
        let (bucket, field_offset) = if with_bucket {
            (u64::from_le_bytes(buf[12..20].try_into().unwrap()), 20)
        } else {
            (0, 12)
        };

        Some(RecordHeader {
            len,
            bucket,
            id: u64::from_le_bytes(buf[field_offset..field_offset + 8].try_into().unwrap()),
            version: u16::from_le_bytes(
                buf[field_offset + 8..field_offset + 10].try_into().unwrap(),
            ),
            crc: u32::from_le_bytes(
                buf[field_offset + 10..field_offset + 14]
                    .try_into()
                    .unwrap(),
            ),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    pub offset: u64,
    pub total_len: u64,
}

pub const TOMBSTONE_BIT: u64 = 1 << 63;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SlotState {
    Empty,

    Deleted(Slot),
    Live(Slot),
}

impl Slot {
    pub fn encode(&self, out: &mut [u8]) {
        out[..8].copy_from_slice(&self.offset.to_le_bytes());
        out[8..16].copy_from_slice(&self.total_len.to_le_bytes());
    }

    pub fn decode(buf: &[u8]) -> Option<Slot> {
        match Slot::state(buf) {
            SlotState::Live(slot) => Some(slot),
            _ => None,
        }
    }

    pub fn state(buf: &[u8]) -> SlotState {
        let offset = u64::from_le_bytes(buf[..8].try_into().unwrap());
        let total_len = u64::from_le_bytes(buf[8..16].try_into().unwrap());

        // tombstones prevent deleted records from returning.
        if offset == 0 {
            SlotState::Empty
        } else if offset & TOMBSTONE_BIT != 0 {
            SlotState::Deleted(Slot {
                offset: offset & !TOMBSTONE_BIT,
                total_len,
            })
        } else {
            SlotState::Live(Slot { offset, total_len })
        }
    }

    pub fn file_offset(id: u64) -> u64 {
        PAGE_SIZE_U64 + id * SLOT_SIZE as u64
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegistryRow {
    pub kind: u8,
    pub bucket: u64,
    pub start: u64,
    pub size: u64,
}

impl RegistryRow {
    pub fn encode(&self, out: &mut [u8]) {
        out[..REGISTRY_ROW_SIZE].fill(0);
        out[..4].copy_from_slice(&REGISTRY_MAGIC.to_le_bytes());
        out[4] = self.kind;
        out[8..16].copy_from_slice(&self.bucket.to_le_bytes());
        out[16..24].copy_from_slice(&self.start.to_le_bytes());
        out[24..32].copy_from_slice(&self.size.to_le_bytes());

        let crc = crc32c(&out[..32]);

        out[32..36].copy_from_slice(&crc.to_le_bytes());
    }

    pub fn decode(buf: &[u8]) -> Option<RegistryRow> {
        if u32::from_le_bytes(buf[..4].try_into().unwrap()) != REGISTRY_MAGIC {
            return None;
        }

        if u32::from_le_bytes(buf[32..36].try_into().unwrap()) != crc32c(&buf[..32]) {
            return None;
        }

        Some(RegistryRow {
            kind: buf[4],
            bucket: u64::from_le_bytes(buf[8..16].try_into().unwrap()),
            start: u64::from_le_bytes(buf[16..24].try_into().unwrap()),
            size: u64::from_le_bytes(buf[24..32].try_into().unwrap()),
        })
    }
}
