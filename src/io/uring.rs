use super::{BlockIo, IoCounters, ReadReq, WriteReq, fdatasync_counted};

use io_uring::{IoUring, opcode, types};

use std::fs::File;
use std::io;
use std::os::unix::io::AsRawFd;

pub const DEFAULT_DEPTH: u32 = 32;
pub const DEFAULT_CHUNK: usize = 512 * 1024;

struct IoSegment {
    offset: u64,
    pointer: *mut u8,
    len: usize,
}

pub struct UringIo {
    ring: IoUring,
    depth: usize,
    chunk: usize,
    counters: IoCounters,
}

impl UringIo {
    pub fn new(depth: u32, chunk: usize) -> io::Result<Self> {
        let depth = depth.max(1);

        Ok(UringIo {
            ring: IoUring::new(depth.next_power_of_two())?,
            depth: depth as usize,
            chunk,
            counters: IoCounters::default(),
        })
    }

    fn run(&mut self, file: &File, mut segments: Vec<IoSegment>, is_read: bool) -> io::Result<()> {
        let file_descriptor = types::Fd(file.as_raw_fd());
        let mut next = 0usize;
        let mut pending = 0usize;
        let mut error: Option<io::Error> = None;

        while next < segments.len() || pending > 0 {
            while pending < self.depth && next < segments.len() && error.is_none() {
                let segment = &segments[next];
                let entry = if is_read {
                    opcode::Read::new(file_descriptor, segment.pointer, segment.len as u32)
                        .offset(segment.offset)
                        .build()
                } else {
                    opcode::Write::new(
                        file_descriptor,
                        segment.pointer as *const u8,
                        segment.len as u32,
                    )
                    .offset(segment.offset)
                    .build()
                }
                .user_data(next as u64);

                unsafe { self.ring.submission().push(&entry).expect("sq full") };
                next += 1;
                pending += 1;
            }

            if pending == 0 {
                break;
            }

            loop {
                match self.ring.submit_and_wait(1) {
                    Ok(_) => break,
                    Err(interrupted_error) if interrupted_error.raw_os_error() == Some(4) => {
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }

            let completions: Vec<(u64, i32)> = self
                .ring
                .completion()
                .map(|completion| (completion.user_data(), completion.result()))
                .collect();

            for (user_data, completion_result) in completions {
                pending -= 1;

                let segment = &mut segments[user_data as usize];

                if completion_result < 0 {
                    if error.is_none() {
                        error = Some(io::Error::from_raw_os_error(-completion_result));
                    }
                    continue;
                }

                let completed_bytes = completion_result as usize;

                if is_read {
                    self.counters.reads += 1;
                    self.counters.read_bytes += completed_bytes as u64;
                } else {
                    self.counters.writes += 1;
                    self.counters.write_bytes += completed_bytes as u64;
                }

                if completed_bytes == segment.len || error.is_some() {
                    continue;
                }

                if is_read && completed_bytes == 0 {
                    unsafe { std::ptr::write_bytes(segment.pointer, 0, segment.len) };
                    continue;
                }

                if !is_read && completed_bytes == 0 {
                    error = Some(io::ErrorKind::WriteZero.into());
                    continue;
                }

                segment.offset += completed_bytes as u64;
                segment.pointer = unsafe { segment.pointer.add(completed_bytes) };
                segment.len -= completed_bytes;

                let entry = if is_read {
                    opcode::Read::new(file_descriptor, segment.pointer, segment.len as u32)
                        .offset(segment.offset)
                        .build()
                } else {
                    opcode::Write::new(
                        file_descriptor,
                        segment.pointer as *const u8,
                        segment.len as u32,
                    )
                    .offset(segment.offset)
                    .build()
                }
                .user_data(user_data);

                unsafe { self.ring.submission().push(&entry).expect("sq full") };
                pending += 1;
            }
        }

        match error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn split(&self, offset: u64, pointer: *mut u8, length: usize, segments: &mut Vec<IoSegment>) {
        let mut done = 0usize;

        while done < length {
            let segment_length = (length - done).min(self.chunk);

            segments.push(IoSegment {
                offset: offset + done as u64,
                pointer: unsafe { pointer.add(done) },
                len: segment_length,
            });
            done += segment_length;
        }
    }
}

impl BlockIo for UringIo {
    fn read_vec(&mut self, file: &File, requests: &mut [ReadReq<'_>]) -> io::Result<()> {
        let mut segments = Vec::with_capacity(requests.len());

        for request in requests {
            self.split(
                request.off,
                request.buf.as_mut_ptr(),
                request.buf.len(),
                &mut segments,
            );
        }

        self.run(file, segments, true)
    }

    fn write_vec(&mut self, file: &File, requests: &[WriteReq<'_>]) -> io::Result<()> {
        let mut segments = Vec::with_capacity(requests.len());

        for request in requests {
            self.split(
                request.off,
                request.buf.as_ptr() as *mut u8,
                request.buf.len(),
                &mut segments,
            );
        }

        self.run(file, segments, false)
    }

    fn sync(&mut self, file: &File) -> io::Result<()> {
        fdatasync_counted(file, &mut self.counters)
    }

    fn counters(&self) -> IoCounters {
        self.counters
    }

    fn reset_counters(&mut self) {
        self.counters = IoCounters::default();
    }
}
