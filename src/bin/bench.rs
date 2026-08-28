use heapstore::heap::{Heap, HeapConfig, SyncPolicy};
use heapstore::io::IoCounters;
use heapstore::io::sync::SyncIo;
use heapstore::io::uring::UringIo;
use heapstore::measure::{Csv, OperationStats, median_f64, median_u64};
use heapstore::rng::SplitMix64;
use heapstore::{AppendPlacement, BucketPlacement};

use std::path::{Path, PathBuf};
use std::time::Instant;

const SIZES: &[(u64, &str)] = &[
    (1 << 10, "1K"),
    (5 << 10, "5K"),
    (18 << 10, "18K"),
    (64 << 10, "64K"),
    (512 << 10, "512K"),
    (2 << 20, "2M"),
    (7 << 20, "7M"),
];

const POPULATION_BUDGET: u64 = 1_900_000_000;

const LARGE_POPULATION_BUDGET: u64 = 2_100_000_000;

const WRITE_BUDGET: u64 = 2_100_000_000;

enum BenchmarkHeap {
    SyncAppend(Heap<SyncIo, AppendPlacement>),
    SyncBucket(Heap<SyncIo, BucketPlacement>),
    UringAppend(Heap<UringIo, AppendPlacement>),
    UringBucket(Heap<UringIo, BucketPlacement>),
}

macro_rules! dispatch_heap {
    ($heap_value:expr, $heap:ident, $operation:expr) => {
        match $heap_value {
            BenchmarkHeap::SyncAppend($heap) => $operation,
            BenchmarkHeap::SyncBucket($heap) => $operation,
            BenchmarkHeap::UringAppend($heap) => $operation,
            BenchmarkHeap::UringBucket($heap) => $operation,
        }
    };
}

impl BenchmarkHeap {
    fn open(layout: &str, io: &str, dir: &Path, config: HeapConfig) -> BenchmarkHeap {
        let uring = || UringIo::new(32, 512 << 10).expect("io_uring");

        match (layout, io) {
            ("append", "sync") => {
                BenchmarkHeap::SyncAppend(Heap::open(dir, SyncIo::new(), config).expect("open"))
            }
            ("bucket", "sync") => {
                BenchmarkHeap::SyncBucket(Heap::open(dir, SyncIo::new(), config).expect("open"))
            }
            ("append", "uring") => {
                BenchmarkHeap::UringAppend(Heap::open(dir, uring(), config).expect("open"))
            }
            ("bucket", "uring") => {
                BenchmarkHeap::UringBucket(Heap::open(dir, uring(), config).expect("open"))
            }
            _ => panic!("unknown combo {layout}/{io}"),
        }
    }

    fn insert_into(&mut self, bucket: u64, payload: &[u8]) -> u64 {
        dispatch_heap!(
            self,
            heap,
            heap.insert_into(bucket, payload).expect("insert")
        )
    }

    fn read(&mut self, id: u64, out: &mut Vec<u8>) {
        dispatch_heap!(self, heap, heap.read(id, out).expect("read"))
    }

    fn read_batch(&mut self, ids: &[u64], out: &mut Vec<Vec<u8>>) {
        dispatch_heap!(self, heap, heap.read_batch(ids, out).expect("read_batch"))
    }

    fn flush(&mut self) {
        dispatch_heap!(self, heap, heap.flush().expect("flush"))
    }

    fn counters(&self) -> IoCounters {
        dispatch_heap!(self, heap, heap.io_counters())
    }

    fn reset_counters(&mut self) {
        dispatch_heap!(self, heap, heap.reset_io_counters())
    }
}

fn config_for(payload_bytes: u64, policy: SyncPolicy) -> HeapConfig {
    HeapConfig {
        sync_policy: policy,
        initial_size: payload_bytes + payload_bytes / 8 + (96 << 20),
        growth: 256 << 20,
        ..HeapConfig::default()
    }
}

fn populate(layout: &str, dir: &Path, size: u64, count: u64, buckets: u64) -> Vec<u64> {
    let _ = std::fs::remove_dir_all(dir);
    let mut heap = BenchmarkHeap::open(
        layout,
        "sync",
        dir,
        config_for(size * count, SyncPolicy::None),
    );
    let mut payload = vec![0u8; size as usize];

    SplitMix64::new(42).fill(&mut payload);

    let ids: Vec<u64> = (0..count)
        .map(|key| heap.insert_into(key % buckets, &payload))
        .collect();

    heap.flush();

    ids
}

fn governor() -> String {
    std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
        .unwrap_or_default()
        .trim()
        .to_string()
}

struct RepetitionResult {
    stats: OperationStats,
    io_counters: IoCounters,
}

fn run_repetition(op_bytes: u64, mut operation: impl FnMut(u64) -> u64) -> Vec<u64> {
    let is_huge_operation = op_bytes >= 256 << 20;
    let (minimum_iterations, target_seconds, maximum_iterations, maximum_seconds) =
        if is_huge_operation {
            (3u64, 6.0, 6u64, 30.0)
        } else {
            (12u64, 0.4, 300u64, 20.0)
        };
    let started = Instant::now();
    let mut latencies = Vec::new();
    let mut iteration = 0u64;

    loop {
        latencies.push(operation(iteration));
        iteration += 1;

        let elapsed = started.elapsed().as_secs_f64();

        if (iteration >= minimum_iterations && elapsed >= target_seconds)
            || iteration >= maximum_iterations
            || elapsed >= maximum_seconds
        {
            break;
        }
    }

    latencies
}

fn summary_row(
    csv: &mut Csv,
    tag: &[&str],
    results: &[RepetitionResult],
    records: u64,
    op_bytes: u64,
) {
    let median_integer = |mapper: &dyn Fn(&RepetitionResult) -> u64| {
        median_u64(&mut results.iter().map(mapper).collect::<Vec<_>>())
    };
    let median_float = |mapper: &dyn Fn(&RepetitionResult) -> f64| {
        median_f64(&mut results.iter().map(mapper).collect::<Vec<_>>())
    };
    let iterations: u64 = results
        .iter()
        .map(|repetition| repetition.stats.iterations)
        .sum();
    let p50_ns = median_integer(&|repetition| repetition.stats.p50_ns);
    let p95_ns = median_integer(&|repetition| repetition.stats.p95_ns);
    let p99_ns = median_integer(&|repetition| repetition.stats.p99_ns);
    let operations_per_second = median_float(&|repetition| repetition.stats.ops_per_sec());
    let per_operation = |value: u64, repetition: &RepetitionResult| {
        value as f64 / repetition.stats.iterations as f64
    };
    let device_reads_per_operation =
        median_float(&|repetition| per_operation(repetition.io_counters.reads, repetition));
    let device_writes_per_operation =
        median_float(&|repetition| per_operation(repetition.io_counters.writes, repetition));
    let read_bytes_per_operation =
        median_float(&|repetition| per_operation(repetition.io_counters.read_bytes, repetition));
    let write_bytes_per_operation =
        median_float(&|repetition| per_operation(repetition.io_counters.write_bytes, repetition));
    let mut row: Vec<String> = tag.iter().map(|field| field.to_string()).collect();

    row.extend([
        iterations.to_string(),
        format!("{:.1}", p50_ns as f64 / 1000.0),
        format!("{:.1}", p95_ns as f64 / 1000.0),
        format!("{:.1}", p99_ns as f64 / 1000.0),
        format!("{operations_per_second:.1}"),
        format!("{:.1}", operations_per_second * records as f64),
        format!("{:.1}", operations_per_second * op_bytes as f64 / 1e6),
        format!("{device_reads_per_operation:.2}"),
        format!("{device_writes_per_operation:.2}"),
        format!("{:.3}", read_bytes_per_operation / op_bytes as f64),
        format!("{:.3}", write_bytes_per_operation / op_bytes as f64),
        governor(),
    ]);

    csv.row(&row).unwrap();
}

const RESULTS_HEADER: &str = "layout,io,op,pattern,records,size,iters,p50_us,p95_us,p99_us,ops_per_s,recs_per_s,mb_per_s,dev_reads_per_op,dev_writes_per_op,read_amp,write_amp,governor";

struct Args {
    data: PathBuf,
    out: PathBuf,
    filter: String,
    repetitions: usize,
}

#[path = "bench/matrix.rs"]
mod matrix;

fn parse(argv: &[String]) -> Args {
    let mut args = Args {
        data: PathBuf::from("."),
        out: PathBuf::from("metrics"),
        filter: String::new(),
        repetitions: 3,
    };
    let mut argument_index = 1;

    while argument_index < argv.len() {
        match (
            argv.get(argument_index).map(String::as_str),
            argv.get(argument_index + 1),
        ) {
            (Some("--dir"), Some(value)) => args.data = value.into(),
            (Some("--out"), Some(value)) => args.out = value.into(),
            (Some("--filter"), Some(value)) => args.filter = value.clone(),
            (Some("--reps"), Some(value)) => args.repetitions = value.parse().expect("reps"),
            (Some(flag), _) => panic!("unknown argument {flag}"),
            (None, _) => break,
        }

        argument_index += 2;
    }

    args
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    if argv.first().map(String::as_str) != Some("matrix") {
        eprintln!("command: matrix; arguments: --dir --out --filter --reps");

        std::process::exit(2);
    }

    let args = parse(&argv);

    std::fs::create_dir_all(&args.out).expect("out dir");

    let mut csv = Csv::open(&args.out.join("matrix.csv"), RESULTS_HEADER).unwrap();

    matrix::read_tests(&args, &mut csv);

    matrix::write_tests(&args, &mut csv);
    
}
