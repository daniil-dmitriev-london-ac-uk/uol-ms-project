use heapstore::AppendPlacement;
use heapstore::heap::{Heap, HeapConfig};
use heapstore::io::sync::SyncIo;
use heapstore::measure::OperationStats;
use heapstore::rng::SplitMix64;
use std::path::Path;
use std::time::Instant;




fn main() {

    let mut heap = Heap::<SyncIo, AppendPlacement>::open(
        Path::new("bench-data"),
        SyncIo::new(),
        HeapConfig::default(),
    )
    .expect("open");
    
    
    let mut payload = vec![0u8; 4096];
    SplitMix64::new(42).fill(&mut payload);
    let mut samples = Vec::new();


    for _ in 0..1024 {
        let started = Instant::now();

        heap.insert(&payload).expect("insert");
        samples.push(started.elapsed().as_nanos() as u64);
    }


    heap.flush().expect("flush");


    let stats = OperationStats::from_samples(&mut samples);


    println!(
        "iters={} p50_ns={} p99_ns={} ops_s={:.1}",
        stats.iterations,
        stats.p50_ns,
        stats.p99_ns,
        stats.ops_per_sec()
    );





























}
