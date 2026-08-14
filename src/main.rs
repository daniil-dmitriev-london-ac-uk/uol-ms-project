mod append;
mod block_io;
mod bucket;
mod heap;
mod placement;
mod uring_io;

use append::AppendPlacement;
use block_io::SyncIo;
use bucket::BucketPlacement;
use heap::Heap;

use std::io;
use std::path::Path;

fn main() -> io::Result<()> {
    let payload = vec![7u8; 4096];

    let mut sync_append = Heap::open(Path::new("sync-append"), SyncIo, AppendPlacement::new(0, 0))?;
    let id = sync_append.insert(&payload)?;

    sync_append.flush()?;

    let mut out = Vec::new();
    sync_append.read(id, &mut out)?;

    let mut uring_bucket = Heap::open(Path::new("uring-bucket"), uring_io::UringIo::new(32)?, BucketPlacement::new(3))?;
    let id = uring_bucket.insert(&payload)?;

    uring_bucket.flush()?;
    uring_bucket.read(id, &mut out)?;

    println!("flexible_heap_sample_bytes={}", out.len());

    Ok(())
}
