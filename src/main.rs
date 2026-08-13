mod append;
mod bucket;
mod io_access;
mod placement;
mod store;
mod uring_access;

use append::AppendPlacement;
use bucket::BucketPlacement;
use store::{IoMode, Store};
use uring_access::UringAccess;

use std::io;
use std::path::Path;

fn main() -> io::Result<()> {
    let payload = vec![7u8; 4096];

    let mut append = Store::open(Path::new("uring-append"), AppendPlacement::new(0, 0), IoMode::Uring(UringAccess::new(32)?))?;
    let mut bucket = Store::open(Path::new("uring-bucket"), BucketPlacement::new(3), IoMode::Uring(UringAccess::new(32)?))?;
    let append_id = append.insert(&payload)?;
    let bucket_id = bucket.insert(&payload)?;

    append.flush()?;
    bucket.flush()?;

    let mut out = Vec::new();
    append.read(append_id, &mut out)?;
    bucket.read(bucket_id, &mut out)?;

    println!("uring_read_layouts=2 sample_bytes={}", out.len());

    Ok(())
}
