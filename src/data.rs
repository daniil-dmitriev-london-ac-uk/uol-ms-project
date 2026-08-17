use crate::format::{FileHeader, PAGE, PAGE_SIZE_U64, page_down, page_up};
use crate::io::{AlignedBuf, BlockIo, ReadReq, WriteReq, fallocate, open_direct};

use std::collections::HashMap;
use std::fs::File;
use std::io;
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
    pub fn create(path: &Path, kind: u8, layout: u8, io: &mut impl BlockIo) -> io::Result<Self> {
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

    pub fn open(path: &Path, kind: u8, layout: u8, io: &mut impl BlockIo) -> io::Result<Self> {
        let file = open_direct(path, false)?;
        let allocated_end = file.metadata()?.len();
        let paged_file = PagedFile {
            file,
            tail_pages: HashMap::new(),
            pool: Vec::new(),
            staged_writes: Vec::new(),
            pending_bytes: 0,
            allocated_end,
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
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid file header",
            )),
        }
    }

    pub fn ensure_alloc(&mut self, upto: u64, chunk: u64) -> io::Result<()> {
        if upto <= self.allocated_end {
            return Ok(());
        }

        let new_end = page_up(upto.max(self.allocated_end + chunk));

        fallocate(&self.file, self.allocated_end, new_end - self.allocated_end)?;

        self.allocated_end = new_end;

        Ok(())
    }

    fn take_buffer(&mut self) -> AlignedBuf {
        let mut buf = self.pool.pop().unwrap_or_else(|| AlignedBuf::zeroed(0));

        buf.clear();

        buf
    }

    pub fn recycle(&mut self, buf: AlignedBuf) {
        if self.pool.len() < MAX_POOLED_BUFFERS && buf.capacity() <= MAX_POOLED_BUFFER_CAPACITY {
            self.pool.push(buf);
        }
    }

    pub fn pending_bytes(&self) -> usize {
        self.pending_bytes
    }

    pub fn stage(&mut self, io: &mut impl BlockIo, off: u64, parts: &[&[u8]]) -> io::Result<()> {
        let len: usize = parts.iter().map(|part| part.len()).sum();

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
            let page = match self.tail_pages.get(&head_page) {
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

    pub fn flush(&mut self, io: &mut impl BlockIo) -> io::Result<()> {
        if self.staged_writes.is_empty() {
            return Ok(());
        }

        let mut staged_writes = std::mem::take(&mut self.staged_writes);

        for staged_write in &mut staged_writes {
            let end = staged_write.end();

            staged_write
                .buffer
                .resize_zeroed((page_up(end) - staged_write.base()) as usize);

            if end % PAGE_SIZE_U64 != 0 {
                let last = page_down(end);
                let within = (end - last) as usize;
                let suffix = if last == staged_write.base() && staged_write.head.is_some() {
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
    ) -> io::Result<(AlignedBuf, usize)> {
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
