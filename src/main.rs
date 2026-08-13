mod append;
mod bucket;
mod io_access;
mod io_mode;
mod placement;
mod store;
mod uring_access;

use append::AppendPlacement;
use bucket::BucketPlacement;
use io_access::SyncAccess;
use io_mode::IoMode;
use placement::Placement;
use store::Store;
use uring_access::UringAccess;

use std::io;
use std::path::Path;
use std::time::Instant;

fn run<P: Placement>(name: &str, place: P, io: IoMode) -> io::Result<()> {
    let mut store = Store::open(Path::new(name), place, io)?;

    let payload = vec![7u8; 4096];

    let started = Instant::now();

    for _ in 0..256 {
        store.insert(&payload)?;
    }

    store.flush()?;

    println!("case={name} elapsed_ms={}", started.elapsed().as_millis());

    Ok(())
}

fn main() -> io::Result<()> {
    run("sync-append", AppendPlacement::new(0, 0), IoMode::Sync(SyncAccess::new()))?;
    run("sync-bucket", BucketPlacement::new(3), IoMode::Sync(SyncAccess::new()))?;
    run("uring-append", AppendPlacement::new(0, 0), IoMode::Uring(UringAccess::new(32)?))?;
    run("uring-bucket", BucketPlacement::new(3), IoMode::Uring(UringAccess::new(32)?))?;

    Ok(())
}
