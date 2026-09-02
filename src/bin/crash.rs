use heapstore::error::HeapError;
use heapstore::heap::{Heap, HeapConfig, SyncPolicy};
use heapstore::io::sync::SyncIo;
use heapstore::measure::{Csv, OperationStats, median_f64};
use heapstore::rng::{SplitMix64, payload_for};
use heapstore::{AppendPlacement, BucketPlacement};

use std::path::{Path, PathBuf};
use std::time::Instant;

const SEED: u64 = 0xDEAD_2026;
const BUCKET_COUNT: u64 = 8;

fn heap_config(policy: SyncPolicy, integrity: bool) -> HeapConfig {
    HeapConfig {
        sync_policy: policy,
        integrity,
        initial_size: 768 << 20,
        growth: 128 << 20,
        ..HeapConfig::default()
    }
}

fn make_payload(key: u64, len: usize) -> Vec<u8> {
    let mut payload = Vec::new();

    payload_for(SEED, key, len, &mut payload);

    payload
}

enum LayoutHeap {
    Append(Heap<SyncIo, AppendPlacement>),
    Bucket(Heap<SyncIo, BucketPlacement>),
}

macro_rules! dispatch_heap {
    ($self:expr, $heap:ident, $expr:expr) => {
        match $self {
            LayoutHeap::Append($heap) => $expr,
            LayoutHeap::Bucket($heap) => $expr,
        }
    };
}

impl LayoutHeap {
    fn open(layout: &str, dir: &Path, config: HeapConfig) -> (LayoutHeap, ()) {
        match layout {
            "append" => (
                LayoutHeap::Append(Heap::open(dir, SyncIo::new(), config).expect("open")),
                (),
            ),
            _ => (
                LayoutHeap::Bucket(Heap::open(dir, SyncIo::new(), config).expect("open")),
                (),
            ),
        }
    }

    fn insert_into(&mut self, bucket: u64, payload: &[u8]) -> u64 {
        dispatch_heap!(
            self,
            heap,
            heap.insert_into(bucket, payload).expect("insert")
        )
    }

    fn read(&mut self, id: u64, out: &mut Vec<u8>) -> Result<(), HeapError> {
        dispatch_heap!(self, heap, heap.read(id, out))
    }

    fn flush(&mut self) {
        dispatch_heap!(self, heap, heap.flush().expect("flush"))
    }

    fn counters(&self) -> heapstore::io::IoCounters {
        dispatch_heap!(self, heap, heap.io_counters())
    }

    fn reset_counters(&mut self) {
        dispatch_heap!(self, heap, heap.reset_io_counters())
    }
}

struct Args {
    data: PathBuf,
    out: PathBuf,
}

#[path = "crash/costs.rs"]
mod costs;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let command = argv.first().map(String::as_str).unwrap_or("help");
    let mut args = Args {
        data: PathBuf::from("."),
        out: PathBuf::from("metrics"),
    };
    let mut argument_index = 1;

    while argument_index < argv.len() {
        match (
            argv.get(argument_index).map(String::as_str),
            argv.get(argument_index + 1),
        ) {
            (Some("--dir"), Some(value)) => args.data = value.into(),
            (Some("--out"), Some(value)) => args.out = value.into(),
            (Some(flag), _) => panic!("unknown argument {flag}"),
            (None, _) => break,
        }

        argument_index += 2;
    }

    std::fs::create_dir_all(&args.out).expect("out dir");

    match command {
        "cost-sync" => {
            let mut csv = Csv::open(
                &args.out.join("cost-sync.csv"),
                "layout,policy,iters,p50_us,p99_us,ops_per_s,write_amp,syncs",
            )
            .unwrap();

            costs::cost_sync(&args, &mut csv);
        }
        "cost-integrity" => {
            let mut csv = Csv::open(
                &args.out.join("cost-integrity.csv"),
                "layout,size,integrity,insert_median,insert_low,insert_high,read_median,read_low,read_high",
            )
            .unwrap();

            costs::cost_integrity(&args, &mut csv);
        }
        "sync-shape" => {
            let mut csv = Csv::open(
                &args.out.join("sync-shape.csv"),
                "regions,ops_per_s,write_amp,syncs,ms_per_sync",
            )
            .unwrap();

            costs::sync_shape(&args, &mut csv);
        }
        "crc-speed" => {
            let mut csv = Csv::open(
                &args.out.join("crc-speed.csv"),
                "size,gb_per_s,us_per_call,checksum",
            )
            .unwrap();

            costs::crc_speed(&args, &mut csv);
        }
        _ => {
            eprintln!("commands: cost-sync | cost-integrity | sync-shape | crc-speed");

            std::process::exit(2);
        }
    }
}
