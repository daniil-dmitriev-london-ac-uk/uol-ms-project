mod append;
mod bucket;
mod data;
mod heap;
mod io;
mod placement;

use append::AppendPlacement;
use bucket::BucketPlacement;
use heap::Heap;
use io::sync::SyncIo;

use std::path::Path;

fn main() -> std::io::Result<()> {
    let payload = vec![7u8; 4096];
    let mut sync_append = Heap::open(
        Path::new("sync-append"),
        SyncIo::new(),
        AppendPlacement::new(0, 0),
    )?;
    let id = sync_append.insert(&payload)?;

    sync_append.flush()?;

    let mut out = Vec::new();

    sync_append.read(id, &mut out)?;

    let mut uring_bucket = Heap::open(
        Path::new("uring-bucket"),
        io::uring::UringIo::new(io::uring::DEFAULT_DEPTH, io::uring::DEFAULT_CHUNK)?,
        BucketPlacement::new(3),
    )?;
    let id = uring_bucket.insert(&payload)?;

    uring_bucket.flush()?;

    uring_bucket.reset_io_counters();

    uring_bucket.read(id, &mut out)?;

    let counters = uring_bucket.io_counters();

    println!(
        "flexible_heap_sample_bytes={} reads={} pending={}",
        out.len(),
        counters.reads,
        uring_bucket.pending_bytes()
    );

    Ok(())
}
