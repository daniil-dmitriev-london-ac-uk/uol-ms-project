use super::{BlockIo, IoCounters, ReadReq, WriteReq, fdatasync_counted};
use crate::error::Result;

use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;

#[derive(Default)]
pub struct SyncIo {
    counters: IoCounters,
}

impl SyncIo {
    pub fn new() -> Self {
        SyncIo::default()
    }
}

impl BlockIo for SyncIo {
    fn read_vec(&mut self, file: &File, requests: &mut [ReadReq<'_>]) -> Result<()> {
        for request in requests {
            let mut done = 0usize;

            while done < request.buf.len() {
                let processed_bytes =
                    file.read_at(&mut request.buf[done..], request.off + done as u64)?;

                self.counters.reads += 1;
                self.counters.read_bytes += processed_bytes as u64;

                if processed_bytes == 0 {
                    request.buf[done..].fill(0);
                    break;
                }

                done += processed_bytes;
            }
        }

        Ok(())
    }

    fn write_vec(&mut self, file: &File, requests: &[WriteReq<'_>]) -> Result<()> {
        for request in requests {
            let mut done = 0usize;

            while done < request.buf.len() {
                let processed_bytes =
                    file.write_at(&request.buf[done..], request.off + done as u64)?;

                if processed_bytes == 0 {
                    return Err(io::Error::from(io::ErrorKind::WriteZero).into());
                }

                self.counters.writes += 1;
                self.counters.write_bytes += processed_bytes as u64;
                done += processed_bytes;
            }
        }

        Ok(())
    }

    fn sync(&mut self, file: &File) -> Result<()> {
        fdatasync_counted(file, &mut self.counters)
    }

    fn counters(&self) -> IoCounters {
        self.counters
    }

    fn reset_counters(&mut self) {
        self.counters = IoCounters::default();
    }
}
