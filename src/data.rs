//! this module stages and reads data pages.

use crate::crc32::crc32c;
use crate::error::{HeapError, Result};
use crate::format::{
    FileHeader, PAGE, PAGE_SIZE_U64, RecordHeader, header_len, page_down, page_up,
};
use crate::io::{AlignedBuf, BlockIo, ReadReq, WriteReq, fallocate, open_direct};

use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

const MAX_STAGED_WRITES: usize = 64;

const MAX_POOLED_BUFFERS: usize = 16;
const MAX_POOLED_BUFFER_CAPACITY: usize = 8 << 20;

struct StagedWrite {
    start: u64,
    buffer: AlignedBuf,
    head: Option<Vec<u8>>,
}

impl StagedWrite {
    fn base(&self) -> u64 {
        page_down(self.start)
    }

    fn end(&self) -> u64 {
        self.base() + self.buffer.len() as u64
    }
}

pub struct PagedFile {
    pub file: File,
    tail_pages: HashMap<u64, Vec<u8>>,
    pool: Vec<AlignedBuf>,
    staged_writes: Vec<StagedWrite>,
    pending_bytes: usize,
    allocated_end: u64,
}

impl PagedFile {
    pub fn create(path: &Path, kind: u8, layout: u8, io: &mut impl BlockIo) -> Result<Self> {
        let file = open_direct(path, true)?;

        file.set_len(0)?;

        let mut paged_file = PagedFile {
            file,
            tail_pages: HashMap::new(),
            pool: Vec::new(),
            staged_writes: Vec::new(),
            pending_bytes: 0,
            allocated_end: 0,
        };
        let mut page = AlignedBuf::zeroed(PAGE);

        FileHeader { kind, layout }.encode(&mut page);

        io.write_vec(&paged_file.file, &[WriteReq { off: 0, buf: &page }])?;
        io.sync(&paged_file.file)?;

        paged_file.allocated_end = PAGE_SIZE_U64;

        Ok(paged_file)
    }

    pub fn open(path: &Path, kind: u8, layout: u8, io: &mut impl BlockIo) -> Result<Self> {
        let file = open_direct(path, false)?;
        let paged_file = PagedFile {
            file,
            tail_pages: HashMap::new(),
            pool: Vec::new(),
            staged_writes: Vec::new(),
            pending_bytes: 0,
            allocated_end: 0,
        };
        let mut page = AlignedBuf::zeroed(PAGE);

        io.read_vec(
            &paged_file.file,
            &mut [ReadReq {
                off: 0,
                buf: &mut page,
            }],
        )?;

        match FileHeader::decode(&page) {
            Some(header) if header.kind == kind && header.layout == layout => Ok(paged_file),
            _ => Err(HeapError::Corrupt {
                what: "file header",
                offset: 0,
            }),
        }
    }

    pub fn size(&self) -> Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    pub fn ensure_alloc(&mut self, upto: u64, chunk: u64) -> Result<()> {
        if upto <= self.allocated_end {
            return Ok(());
        }

        let new_end = page_up(upto.max(self.allocated_end + chunk));

        fallocate(&self.file, self.allocated_end, new_end - self.allocated_end)?;

        self.allocated_end = new_end;

        Ok(())
    }

    pub fn note_alloc(&mut self, upto: u64) {
        self.allocated_end = self.allocated_end.max(upto);
    }

    pub fn has_pending(&self) -> bool {
        !self.staged_writes.is_empty()
    }

    pub fn pending_bytes(&self) -> usize {
        self.pending_bytes
    }

    fn take_buffer(&mut self) -> AlignedBuf {
        let mut buffer = self.pool.pop().unwrap_or_else(|| AlignedBuf::zeroed(0));

        buffer.clear();

        buffer
    }

    pub fn take_sized(&mut self, len: usize) -> AlignedBuf {
        let mut buffer = self.take_buffer();

        buffer.resize_zeroed(len);

        buffer
    }

    pub fn recycle(&mut self, buf: AlignedBuf) {
        if self.pool.len() < MAX_POOLED_BUFFERS && buf.capacity() <= MAX_POOLED_BUFFER_CAPACITY {
            self.pool.push(buf);
        }
    }

    pub fn stage(&mut self, io: &mut impl BlockIo, off: u64, parts: &[&[u8]]) -> Result<()> {
        let len: usize = parts.iter().map(|part| part.len()).sum();

        // reuse open staged_writes to batch interleaved buckets.
        if let Some(staged_write_index) = self
            .staged_writes
            .iter()
            .position(|staged_write| staged_write.end() == off)
        {
            let new_end = page_up(off + len as u64);
            let base = self.staged_writes[staged_write_index].base();
            let overlaps_other_write =
                self.staged_writes
                    .iter()
                    .enumerate()
                    .any(|(other_index, staged_write)| {
                        other_index != staged_write_index
                            && staged_write.base() < new_end
                            && base < page_up(staged_write.end())
                    });

            if !overlaps_other_write {
                for part in parts {
                    self.staged_writes[staged_write_index]
                        .buffer
                        .extend_from_slice(part);
                }

                self.pending_bytes += len;

                return Ok(());
            }
        }

        // bounded staged_writes keep lookup and memory costs predictable.
        if self.staged_writes.len() >= MAX_STAGED_WRITES {
            self.flush(io)?;
        }

        let head_page = page_down(off);
        let new_end = page_up(off + len as u64);

        if self.staged_writes.iter().any(|staged_write| {
            staged_write.base() < new_end && head_page < page_up(staged_write.end())
        }) {
            self.flush(io)?;
        }

        let mut buf = self.take_buffer();
        let pad = (off - head_page) as usize;
        let mut head = None;

        if pad > 0 {
            let page: Vec<u8> = match self.tail_pages.get(&head_page) {
                Some(page) => page.clone(),
                None => {
                    let mut page = AlignedBuf::zeroed(PAGE);

                    io.read_vec(
                        &self.file,
                        &mut [ReadReq {
                            off: head_page,
                            buf: &mut page,
                        }],
                    )?;

                    page.to_vec()
                }
            };

            buf.extend_from_slice(&page[..pad]);
            head = Some(page);
        }

        for part in parts {
            buf.extend_from_slice(part);
        }

        self.pending_bytes += len;
        self.staged_writes.push(StagedWrite {
            start: off,
            buffer: buf,
            head,
        });

        Ok(())
    }

    pub fn flush(&mut self, io: &mut impl BlockIo) -> Result<()> {
        if self.staged_writes.is_empty() {
            return Ok(());
        }

        let mut staged_writes = std::mem::take(&mut self.staged_writes);

        for staged_write in &mut staged_writes {
            let end = staged_write.end();

            staged_write
                .buffer
                .resize_zeroed((page_up(end) - staged_write.base()) as usize);

            // preserve unrelated bytes in a partial page.
            if end % PAGE_SIZE_U64 != 0 {
                let last = page_down(end);
                let within = (end - last) as usize;
                let suffix: Option<Vec<u8>> =
                    if last == staged_write.base() && staged_write.head.is_some() {
                        staged_write.head.take()
                    } else if let Some(page) = self.tail_pages.get(&last) {
                        Some(page.clone())
                    } else {
                        let mut page = AlignedBuf::zeroed(PAGE);

                        io.read_vec(
                            &self.file,
                            &mut [ReadReq {
                                off: last,
                                buf: &mut page,
                            }],
                        )?;

                        Some(page.to_vec())
                    };

                if let Some(page) = suffix {
                    let page_buffer_offset = (last - staged_write.base()) as usize;

                    staged_write.buffer[page_buffer_offset + within..page_buffer_offset + PAGE]
                        .copy_from_slice(&page[within..]);
                }
            }

            for page in (staged_write.base()..page_up(end)).step_by(PAGE) {
                self.tail_pages.remove(&page);
            }

            if end % PAGE_SIZE_U64 != 0 {
                let last = page_down(end);
                let page_buffer_offset = (last - staged_write.base()) as usize;

                self.tail_pages.insert(
                    last,
                    staged_write.buffer[page_buffer_offset..page_buffer_offset + PAGE].to_vec(),
                );
            }
        }

        let requests: Vec<WriteReq<'_>> = staged_writes
            .iter()
            .map(|staged_write| WriteReq {
                off: staged_write.base(),
                buf: &staged_write.buffer,
            })
            .collect();

        io.write_vec(&self.file, &requests)?;

        self.pending_bytes = 0;

        for staged_write in staged_writes {
            self.recycle(staged_write.buffer);
        }

        Ok(())
    }

    pub fn read_aligned(
        &mut self,
        io: &mut impl BlockIo,
        off: u64,
        len: u64,
    ) -> Result<(AlignedBuf, usize)> {
        self.flush(io)?;

        let base = page_down(off);
        let mut buf = self.take_buffer();

        buf.resize_zeroed((page_up(off + len) - base) as usize);

        io.read_vec(
            &self.file,
            &mut [ReadReq {
                off: base,
                buf: &mut buf,
            }],
        )?;

        Ok((buf, (off - base) as usize))
    }
}

pub struct DataFile {
    pub paged_file: PagedFile,
    pub with_bucket: bool,
    pub integrity: bool,
}

impl DataFile {
    pub fn record_header_len(&self) -> u64 {
        header_len(self.with_bucket) as u64
    }

    pub fn stage_record(
        &mut self,
        io: &mut impl BlockIo,
        off: u64,
        bucket: u64,
        id: u64,
        version: u16,
        payload: &[u8],
    ) -> Result<()> {
        let crc = if self.integrity { crc32c(payload) } else { 0 };
        let header = RecordHeader {
            len: payload.len() as u64,
            bucket,
            id,
            version,
            crc,
        };
        let mut header_buffer = [0u8; 38];
        let header_size = header_len(self.with_bucket);

        header.encode(self.with_bucket, &mut header_buffer[..header_size]);
        self.paged_file
            .stage(io, off, &[&header_buffer[..header_size], payload])
    }

    fn parse(
        &self,
        buffer: &[u8],
        buffer_offset: usize,
        total_len: u64,
        expect_id: u64,
        offset: u64,
        out: &mut Vec<u8>,
    ) -> Result<RecordHeader> {
        let header_size = header_len(self.with_bucket);
        let header = RecordHeader::decode(&buffer[buffer_offset..], self.with_bucket).ok_or(
            HeapError::Corrupt {
                what: "record header",
                offset,
            },
        )?;

        if header.id != expect_id || header.len + header_size as u64 != total_len {
            return Err(HeapError::Corrupt {
                what: "record identity",
                offset,
            });
        }

        let payload_end = buffer_offset + header_size + header.len as usize;
        let payload = &buffer[buffer_offset + header_size..payload_end];

        if self.integrity && crc32c(payload) != header.crc {
            return Err(HeapError::Corrupt {
                what: "record payload",
                offset,
            });
        }

        out.clear();
        out.extend_from_slice(payload);

        Ok(header)
    }

    pub fn read_record(
        &mut self,
        io: &mut impl BlockIo,
        off: u64,
        total_len: u64,
        expect_id: u64,
        out: &mut Vec<u8>,
    ) -> Result<RecordHeader> {
        let (buffer, buffer_offset) = self.paged_file.read_aligned(io, off, total_len)?;
        let parse_result = self.parse(&buffer, buffer_offset, total_len, expect_id, off, out);

        self.paged_file.recycle(buffer);

        parse_result
    }

    pub fn read_header(
        &mut self,
        io: &mut impl BlockIo,
        off: u64,
        expect_id: u64,
    ) -> Result<RecordHeader> {
        let (buffer, buffer_offset) =
            self.paged_file
                .read_aligned(io, off, self.record_header_len())?;
        let header = RecordHeader::decode(&buffer[buffer_offset..], self.with_bucket);

        self.paged_file.recycle(buffer);

        let header = header.ok_or(HeapError::Corrupt {
            what: "record header",
            offset: off,
        })?;

        if header.id != expect_id {
            return Err(HeapError::Corrupt {
                what: "record identity",
                offset: off,
            });
        }

        Ok(header)
    }

    pub fn read_many(
        &mut self,
        io: &mut impl BlockIo,
        items: &[(u64, u64, u64)],
        out: &mut Vec<Vec<u8>>,
    ) -> Result<()> {
        self.paged_file.flush(io)?;

        let mut order: Vec<usize> = (0..items.len()).collect();

        order.sort_by_key(|&item_index| items[item_index].0);

        let mut groups: Vec<(u64, u64, Vec<usize>)> = Vec::new();

        for &item_index in &order {
            let (offset, length, _) = items[item_index];
            let (group_start, group_end) = (page_down(offset), page_up(offset + length));

            match groups.last_mut() {
                Some(group) if group_start <= group.1 => {
                    group.1 = group.1.max(group_end);
                    group.2.push(item_index);
                }
                _ => groups.push((group_start, group_end, vec![item_index])),
            }
        }

        let mut buffers: Vec<AlignedBuf> = groups
            .iter()
            .map(|group| self.paged_file.take_sized((group.1 - group.0) as usize))
            .collect();

        {
            let mut requests: Vec<ReadReq<'_>> = groups
                .iter()
                .zip(buffers.iter_mut())
                .map(|(group, buffer)| ReadReq {
                    off: group.0,
                    buf: buffer,
                })
                .collect();

            io.read_vec(&self.paged_file.file, &mut requests)?;
        }

        out.clear();
        out.resize(items.len(), Vec::new());

        for (group, buffer) in groups.iter().zip(buffers.iter()) {
            for &item_index in &group.2 {
                let (offset, length, id) = items[item_index];
                let mut payload = Vec::new();

                self.parse(
                    buffer,
                    (offset - group.0) as usize,
                    length,
                    id,
                    offset,
                    &mut payload,
                )?;

                out[item_index] = payload;
            }
        }

        for buffer in buffers {
            self.paged_file.recycle(buffer);
        }

        Ok(())
    }
}
