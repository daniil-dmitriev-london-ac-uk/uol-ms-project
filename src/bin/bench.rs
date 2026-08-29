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

    fn update(&mut self, id: u64, payload: &[u8]) {
        dispatch_heap!(self, heap, heap.update(id, payload).expect("update"))
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

    fn stats(&self) -> heapstore::HeapStats {
        dispatch_heap!(self, heap, heap.stats())
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

fn populate(layout: &str, dir: &Path, size: u64, count: u64, bucket_count: u64) -> Vec<u64> {
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
        .map(|key| heap.insert_into(key % bucket_count, &payload))
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

        let elapsed_seconds = started.elapsed().as_secs_f64();

        if (iteration >= minimum_iterations && elapsed_seconds >= target_seconds)
            || iteration >= maximum_iterations
            || elapsed_seconds >= maximum_seconds
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
    let iterations: u64 = results.iter().map(|result| result.stats.iterations).sum();
    let p50_ns = median_integer(&|result| result.stats.p50_ns);
    let p95_ns = median_integer(&|result| result.stats.p95_ns);
    let p99_ns = median_integer(&|result| result.stats.p99_ns);
    let operations_per_second = median_float(&|result| result.stats.ops_per_sec());
    let per_operation =
        |value: u64, result: &RepetitionResult| value as f64 / result.stats.iterations as f64;
    let device_reads_per_operation =
        median_float(&|result| per_operation(result.io_counters.reads, result));
    let device_writes_per_operation =
        median_float(&|result| per_operation(result.io_counters.writes, result));
    let read_bytes_per_operation =
        median_float(&|result| per_operation(result.io_counters.read_bytes, result));
    let write_bytes_per_operation =
        median_float(&|result| per_operation(result.io_counters.write_bytes, result));
    let mut row: Vec<String> = tag.iter().map(|field| field.to_string()).collect();

    row.extend([
        format!("{iterations}"),
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

    eprintln!(
        "  {} p50={:.0}us p99={:.0}us ops/s={:.1} recs/s={:.0}",
        tag.join(","),
        p50_ns as f64 / 1000.0,
        p99_ns as f64 / 1000.0,
        operations_per_second,
        operations_per_second * records as f64
    );
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

use matrix::{read_tests, write_tests};

fn space(args: &Args) {
    let mut csv = Csv::open(
        &args.out.join("space.csv"),
        "layout,phase,records,payload_bytes,data_used,data_file,data_blocks,index_blocks,registry_blocks,utilization",
    )
    .unwrap();

    use std::os::linux::fs::MetadataExt;

    let blocks = |path: &Path| {
        path.metadata()
            .map(|metadata| metadata.st_blocks() * 512)
            .unwrap_or(0)
    };

    for layout in ["append", "bucket"] {
        let dir = args.data.join("bench-data").join("space-store");
        let size = 1 << 10;
        let count = 20_000u64;
        let ids = populate(layout, &dir, size, count, 16);
        let mut heap = BenchmarkHeap::open(
            layout,
            "sync",
            &dir,
            config_for(size * count, SyncPolicy::None),
        );
        let mut phase = |heap: &mut BenchmarkHeap, name: &str, payload_bytes: u64| {
            heap.flush();

            let stats = heap.stats();
            let (data_blocks, index_blocks, registry_blocks) = (
                blocks(&dir.join("data.hs")),
                blocks(&dir.join("index.hs")),
                blocks(&dir.join("registry.hs")),
            );

            csv.row(&[
                layout.into(),
                name.into(),
                count.to_string(),
                payload_bytes.to_string(),
                stats.data_used_bytes.to_string(),
                stats.data_file_bytes.to_string(),
                data_blocks.to_string(),
                index_blocks.to_string(),
                registry_blocks.to_string(),
                format!("{:.4}", payload_bytes as f64 / stats.data_used_bytes as f64),
            ])
            .unwrap();
        };

        phase(&mut heap, "after-populate", size * count);

        let mut payload = vec![0u8; size as usize];

        SplitMix64::new(9).fill(&mut payload);

        for &id in ids.iter().step_by(4) {
            heap.update(id, &payload);
        }

        phase(&mut heap, "after-update-quarter", size * count);

        drop(heap);

        let _ = std::fs::remove_dir_all(&dir);
    }

    println!("space.csv is ready");
}

fn updates(args: &Args) {
    let mut csv = Csv::open(&args.out.join("updates.csv"), RESULTS_HEADER).unwrap();

    for layout in ["append", "bucket"] {
        for (size, size_name) in [(1u64 << 10, "1K"), (64 << 10, "64K")] {
            let dir = args.data.join("bench-data").join("upd-store");
            let population = 4096u64;

            for io in ["sync", "uring"] {
                let tag = [layout, io, "update", "arbitrary", "1", size_name];

                if !tag.join(",").contains(&args.filter) {
                    continue;
                }

                let ids = populate(layout, &dir, size, population, 16);
                let mut heap = BenchmarkHeap::open(
                    layout,
                    io,
                    &dir,
                    config_for(size * population, SyncPolicy::None),
                );
                let mut payload = vec![0u8; size as usize];

                SplitMix64::new(11).fill(&mut payload);

                let mut repetition_results = Vec::new();

                for repetition in 0..args.repetitions {
                    let mut rng = SplitMix64::new(100 + repetition as u64);

                    heap.reset_counters();

                    let mut latencies = run_repetition(size, |_| {
                        let id = ids[rng.below(population) as usize];
                        let operation_started = Instant::now();

                        heap.update(id, &payload);

                        operation_started.elapsed().as_nanos() as u64
                    });

                    repetition_results.push(RepetitionResult {
                        stats: OperationStats::from_samples(&mut latencies),
                        io_counters: heap.counters(),
                    });
                }

                summary_row(&mut csv, &tag, &repetition_results, 1, size);
            }

            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    println!("updates.csv is ready");
}

fn parse(argv: &[String]) -> (String, Args) {
    let command = argv.first().cloned().unwrap_or_else(|| "help".into());
    let mut args = Args {
        data: PathBuf::from("."),
        out: PathBuf::from("metrics"),
        filter: String::new(),
        repetitions: 3,
    };
    let mut argument_index = 1;

    while argument_index < argv.len() {
        match (
            argv.get(argument_index).map(|value| value.as_str()),
            argv.get(argument_index + 1),
        ) {
            (Some("--dir"), Some(value)) => args.data = value.into(),
            (Some("--out"), Some(value)) => args.out = value.into(),
            (Some("--filter"), Some(value)) => args.filter = value.clone(),
            (Some("--reps"), Some(value)) => args.repetitions = value.parse().expect("reps"),
            (None, _) => break,
            (Some(flag), _) => panic!("unknown argument {flag}"),
        }

        argument_index += 2;
    }

    (command, args)
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let (command, args) = parse(&argv);

    std::fs::create_dir_all(&args.out).expect("out dir");

    match command.as_str() {
        "matrix" => {
            let mut csv = Csv::open(&args.out.join("matrix.csv"), RESULTS_HEADER).unwrap();
            let started = Instant::now();

            read_tests(&args, &mut csv);

            write_tests(&args, &mut csv);

            eprintln!(
                "matrix completed in {:.0}s",
                started.elapsed().as_secs_f64()
            );
        }
        "space" => space(&args),
        "updates" => updates(&args),
        _ => {
            eprintln!("commands: matrix | space | updates; arguments: --dir --out --filter --reps");

            std::process::exit(2);
        }
    }
}
