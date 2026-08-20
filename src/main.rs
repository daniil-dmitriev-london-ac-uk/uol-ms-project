mod append;
mod bucket;
mod crc32;
mod data;
mod format;
mod heap;
mod index;
mod io;
mod placement;

use append::AppendPlacement;
use bucket::BucketPlacement;
use heap::{Heap, HeapConfig};
use io::sync::SyncIo;

use std::path::Path;

fn main() -> std::io::Result<()> {
    let payload = vec![7u8; 4096];
    let mut sync_append = Heap::<SyncIo, AppendPlacement>::open(
        Path::new("sync-append"),
        SyncIo::new(),
        HeapConfig::default(),
    )?;
    let id = sync_append.insert(&payload)?;

    sync_append.flush()?;

    let mut out = Vec::new();

    sync_append.read(id, &mut out)?;

    let mut uring_bucket = Heap::<io::uring::UringIo, BucketPlacement>::open(
        Path::new("uring-bucket"),
        io::uring::UringIo::new(io::uring::DEFAULT_DEPTH, io::uring::DEFAULT_CHUNK)?,
        HeapConfig::default(),
    )?;
    let id = uring_bucket.insert_into(3, &payload)?;

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
    println!("used_bytes={}", uring_bucket.used_bytes());

    Ok(())
}
