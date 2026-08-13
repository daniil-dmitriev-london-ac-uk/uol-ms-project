use crate::block_io::BlockIo;

use io_uring::{opcode, types, IoUring};

use std::fs::File;
use std::io;
use std::os::unix::io::AsRawFd;

pub struct UringIo {
    ring: IoUring,
}

impl UringIo {
    pub fn new(depth: u32) -> io::Result<Self> {
        Ok(UringIo { ring: IoUring::new(depth.max(1).next_power_of_two())? })
    }

    fn complete(&mut self) -> io::Result<usize> {
        self.ring.submit_and_wait(1)?;
        let result = self.ring.completion().next().ok_or(io::ErrorKind::UnexpectedEof)?.result();

        if result < 0 {
            Err(io::Error::from_raw_os_error(-result))
        } else {
            Ok(result as usize)
        }
    }
}

impl BlockIo for UringIo {
    fn read_at(&mut self, file: &File, off: u64, data: &mut [u8]) -> io::Result<()> {
        let entry = opcode::Read::new(types::Fd(file.as_raw_fd()), data.as_mut_ptr(), data.len() as u32).offset(off).build();
        unsafe { self.ring.submission().push(&entry).map_err(|_| io::ErrorKind::WouldBlock)? };
        if self.complete()? == data.len() { Ok(()) } else { Err(io::ErrorKind::UnexpectedEof.into()) }
    }

    fn write_at(&mut self, file: &File, off: u64, data: &[u8]) -> io::Result<()> {
        let entry = opcode::Write::new(types::Fd(file.as_raw_fd()), data.as_ptr(), data.len() as u32).offset(off).build();
        unsafe { self.ring.submission().push(&entry).map_err(|_| io::ErrorKind::WouldBlock)? };
        if self.complete()? == data.len() { Ok(()) } else { Err(io::ErrorKind::WriteZero.into()) }
    }

    fn sync(&mut self, file: &File) -> io::Result<()> {
        file.sync_data()
    }
}
