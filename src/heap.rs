use crate::crc32::crc32c;
use crate::data::PagedFile;
use crate::format::{RecordHeader, SLOT_SIZE, Slot, header_len};
use crate::io::BlockIo;
use crate::placement::Placement;

use std::io;
use std::path::Path;

pub struct Heap<I: BlockIo, P: Placement> {
    io: I,
    placement: P,
    data: PagedFile,
    index: PagedFile,
}

impl<I: BlockIo, P: Placement> Heap<I, P> {
    pub fn open(dir: &Path, mut io: I, placement: P) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;

        let data_path = dir.join("data.hs");
        let index_path = dir.join("index.hs");
        let data = if data_path.exists() {
            PagedFile::open(&data_path, b'D', P::LAYOUT, &mut io)?
        } else {
            PagedFile::create(&data_path, b'D', P::LAYOUT, &mut io)?
        };
        let index = if index_path.exists() {
            PagedFile::open(&index_path, b'I', P::LAYOUT, &mut io)?
        } else {
            PagedFile::create(&index_path, b'I', P::LAYOUT, &mut io)?
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
        let total = header_len(P::WITH_BUCKET) as u64 + payload.len() as u64;

        self.data.ensure_alloc(offset + total, 64 << 20)?;
        self.index
            .ensure_alloc(Slot::file_offset(id) + SLOT_SIZE as u64, 1 << 20)?;

        let mut header = [0u8; 38];

        RecordHeader {
            len: payload.len() as u64,
            bucket: self.placement.bucket(),
            id,
            version: 1,
            crc: crc32c(payload),
        }
        .encode(P::WITH_BUCKET, &mut header);

        let header = &header[..header_len(P::WITH_BUCKET)];

        self.data.stage(&mut self.io, offset, &[header, payload])?;

        let mut slot = [0u8; SLOT_SIZE];

        Slot {
            offset,
            total_len: total,
        }
        .encode(&mut slot);
        self.index
            .stage(&mut self.io, Slot::file_offset(id), &[&slot])?;

        self.placement.note_written(id, total);

        Ok(id)
    }

    pub fn read(&mut self, id: u64, out: &mut Vec<u8>) -> io::Result<()> {
        let (slot_buf, at) =
            self.index
                .read_aligned(&mut self.io, Slot::file_offset(id), SLOT_SIZE as u64)?;
        let slot = Slot::decode(&slot_buf[at..at + SLOT_SIZE])
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "record is not indexed"))?;

        self.index.recycle(slot_buf);

        let (record, at) = self
            .data
            .read_aligned(&mut self.io, slot.offset, slot.total_len)?;
        let header_bytes = header_len(P::WITH_BUCKET);
        let header = RecordHeader::decode(&record[at..at + header_bytes], P::WITH_BUCKET)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "record header checksum mismatch",
                )
            })?;
        let payload = &record[at + header_bytes..at + slot.total_len as usize];

        if header.id != id || header.len != payload.len() as u64 || crc32c(payload) != header.crc {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "payload checksum mismatch",
            ));
        }

        out.clear();
        out.extend_from_slice(payload);

        self.data.recycle(record);

        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.data.flush(&mut self.io)?;
        self.io.sync(&self.data.file)?;

        self.index.flush(&mut self.io)?;
        self.io.sync(&self.index.file)
    }

    pub fn io_counters(&self) -> crate::io::IoCounters {
        self.io.counters()
    }

    pub fn reset_io_counters(&mut self) {
        self.io.reset_counters();
    }

    pub fn pending_bytes(&self) -> usize {
        self.data.pending_bytes() + self.index.pending_bytes()
    }
}
