mod append;
mod bucket;
mod io_access;
mod placement;
mod store;
mod uring_access;

use append::AppendPlacement;
use bucket::BucketPlacement;
use io_access::SyncAccess;
use store::{Store, WriteMode};
use uring_access::UringAccess;

use std::io;
use std::path::Path;

fn main() -> io::Result<()> {
    let payload = vec![7u8; 4096];

    let mut append = Store::open(Path::new("uring-write-append"), AppendPlacement::new(0, 0), WriteMode::Uring(UringAccess::new(32)?))?;
    let mut bucket = Store::open(Path::new("uring-write-bucket"), BucketPlacement::new(3), WriteMode::Uring(UringAccess::new(32)?))?;

    append.insert(&payload)?;
    bucket.insert(&payload)?;
    append.flush()?;
    bucket.flush()?;

    let _sync = WriteMode::Sync(SyncAccess::new());

    println!("uring_write_layouts=2");

    Ok(())
}
