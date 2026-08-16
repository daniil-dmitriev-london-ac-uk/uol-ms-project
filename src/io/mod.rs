pub mod sync;
pub mod uring;

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::fs::File;
use std::io;
use std::ops::{Deref, DerefMut};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub const PAGE: usize = 4096;

pub struct AlignedBuf {
    ptr: *mut u8,
    len: usize,
    capacity: usize,
}

unsafe impl Send for AlignedBuf {}

impl AlignedBuf {
    pub fn zeroed(len: usize) -> Self {
        let mut buf = AlignedBuf {
            ptr: std::ptr::null_mut(),
            len: 0,
            capacity: 0,
        };

        buf.reserve(len);
        buf.len = len;

        buf
    }

    pub fn reserve(&mut self, need: usize) {
        if need <= self.capacity {
            return;
        }

        let capacity = need.next_multiple_of(PAGE).max(self.capacity * 2);
        let layout = Layout::from_size_align(capacity, PAGE).unwrap();
        let ptr = unsafe { alloc_zeroed(layout) };

        assert!(!ptr.is_null(), "aligned alloc failed");

        if self.len > 0 {
            unsafe { std::ptr::copy_nonoverlapping(self.ptr, ptr, self.len) };
        }
        self.free();
        self.ptr = ptr;
        self.capacity = capacity;
    }

    fn free(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                dealloc(
                    self.ptr,
                    Layout::from_size_align(self.capacity, PAGE).unwrap(),
                )
            };
        }
    }

    pub fn extend_from_slice(&mut self, data: &[u8]) {
        self.reserve(self.len + data.len());
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), self.ptr.add(self.len), data.len()) };
        self.len += data.len();
    }

    pub fn resize_zeroed(&mut self, len: usize) {
        self.reserve(len);
        if len > self.len {
            unsafe { std::ptr::write_bytes(self.ptr.add(self.len), 0, len - self.len) };
        }
        self.len = len;
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Drop for AlignedBuf {
    fn drop(&mut self) {
        self.free();
    }
}

impl Deref for AlignedBuf {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        if self.ptr.is_null() {
            return &[];
        }

        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl DerefMut for AlignedBuf {
    fn deref_mut(&mut self) -> &mut [u8] {
        if self.ptr.is_null() {
            return &mut [];
        }

        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct IoCounters {
    pub reads: u64,
    pub writes: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub syncs: u64,
}

pub struct ReadReq<'a> {
    pub off: u64,
    pub buf: &'a mut [u8],
}

pub struct WriteReq<'a> {
    pub off: u64,
    pub buf: &'a [u8],
}

pub trait BlockIo {
    fn read_vec(&mut self, file: &File, requests: &mut [ReadReq<'_>]) -> io::Result<()>;
    fn write_vec(&mut self, file: &File, requests: &[WriteReq<'_>]) -> io::Result<()>;
    fn sync(&mut self, file: &File) -> io::Result<()>;
    fn counters(&self) -> IoCounters;
    fn reset_counters(&mut self);
}

pub fn open_direct(path: &Path, create: bool) -> io::Result<File> {
    let mut options = std::fs::OpenOptions::new();

    options.read(true).write(true).custom_flags(libc::O_DIRECT);

    if create {
        options.create(true);
    }

    options.open(path)
}

pub fn fdatasync_counted(file: &File, counters: &mut IoCounters) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;

    let result_code = unsafe { libc::fdatasync(file.as_raw_fd()) };

    if result_code != 0 {
        return Err(io::Error::last_os_error());
    }

    counters.syncs += 1;

    Ok(())
}

pub fn fallocate(file: &File, offset: u64, len: u64) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;

    let result_code = unsafe { libc::fallocate(file.as_raw_fd(), 0, offset as i64, len as i64) };

    if result_code != 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}
