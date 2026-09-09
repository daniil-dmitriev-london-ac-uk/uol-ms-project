use heapstore::heap::{Heap, HeapConfig};
use heapstore::io::BlockIo;
use heapstore::io::sync::SyncIo;
use heapstore::io::uring::UringIo;
use heapstore::placement::Placement;
use heapstore::rng::payload_for;

use std::path::PathBuf;




pub const SEED: u64 = 0x5EED_2026;

pub fn test_dir(name: &str) -> PathBuf {
    let base = std::env::var("HEAPSTORE_TEST_DIR").unwrap_or_else(|_| "target/testdata".into());
    let dir = PathBuf::from(base).join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    dir
}

pub fn small_config() -> HeapConfig {
    HeapConfig {
        initial_size: 16 << 20,
        growth: 8 << 20,
        min_extent: 256 << 10,
        region_slots: 4096,
        ..HeapConfig::default()
    }
}

#[allow(dead_code)]
pub fn sync_io() -> SyncIo {
    SyncIo::new()
}


#[allow(dead_code)]
pub fn uring_io() -> UringIo {
    UringIo::new(32, 512 << 10).expect("io_uring is unavailable")
}


pub fn make_payload(key: u64, len: usize) -> Vec<u8> {
    let mut payload = Vec::new();

    payload_for(SEED, key, len, &mut payload);

    payload
}



pub fn assert_record<I: BlockIo, P: Placement>(heap: &mut Heap<I, P>, id: u64, key: u64, len: usize) {
    let mut out = Vec::new();

    heap.read(id, &mut out).unwrap_or_else(|error| panic!("read {id}: {error}"));

    assert_eq!(out, make_payload(key, len), "payload mismatch for id {id}");
}






