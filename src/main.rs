mod append;
mod block_io;
mod bucket;
mod placement;
mod store;
mod uring_io;

use append::AppendPlacement;
use block_io::{BlockIo, SyncIo};
use bucket::BucketPlacement;
use placement::Placement;
use store::Store;

use std::io;
use std::path::Path;

fn run<I: BlockIo, P: Placement>(name: &str, io: I, place: P) -> io::Result<()> {
    let mut store = Store::open(Path::new(name), io, place)?;

    let id = store.insert(&vec![7u8; 4096])?;

    store.flush()?;

    let mut out = Vec::new();
    store.read(id, &mut out)
}

fn main() -> io::Result<()> {
    run("sync-append", SyncIo, AppendPlacement::new(0, 0))?;
    run("sync-bucket", SyncIo, BucketPlacement::new(3))?;
    run("uring-append", uring_io::UringIo::new(32)?, AppendPlacement::new(0, 0))?;
    run("uring-bucket", uring_io::UringIo::new(32)?, BucketPlacement::new(3))?;

    Ok(())
}
