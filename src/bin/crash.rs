use heapstore::error::HeapError;
use heapstore::format::{LAYOUT_APPEND, LAYOUT_BUCKET, Slot};
use heapstore::heap::{Heap, HeapConfig, SyncPolicy};
use heapstore::io::sync::SyncIo;
use heapstore::measure::{Csv, OperationStats, median_f64};
use heapstore::rng::{SplitMix64, payload_for};
use heapstore::{AppendPlacement, BucketPlacement, RecoveryReport};

use std::io::{BufRead, Write};
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
    fn open(layout: &str, dir: &Path, config: HeapConfig) -> (LayoutHeap, RecoveryReport) {
        match layout {
            "append" => {
                let (heap, report) = Heap::open(dir, SyncIo::new(), config).expect("open");

                (LayoutHeap::Append(heap), report)
            }
            _ => {
                let (heap, report) = Heap::open(dir, SyncIo::new(), config).expect("open");

                (LayoutHeap::Bucket(heap), report)
            }
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

fn child(dir: &Path, layout: &str, policy: u32) -> ! {
    let (mut heap, _) =
        LayoutHeap::open(layout, dir, heap_config(SyncPolicy::EveryN(policy), true));
    let mut log = std::fs::File::create(dir.join("confirmed.log")).unwrap();
    let mut pending = String::new();
    let mut key = 0u64;

    loop {
        let id = heap.insert_into(key % BUCKET_COUNT, &make_payload(key, 1024));

        pending.push_str(&format!("{key} {id}\n"));
        key += 1;

        if key.is_multiple_of(policy as u64) {
            pending.push_str(&format!("C {key}\n"));

            log.write_all(pending.as_bytes()).unwrap();
            log.sync_data().unwrap();

            pending.clear();
        }
    }
}

fn kill_rounds(args: &Args, layout: &str, policy: u32, rounds: u32, csv: &mut Csv) {
    for round in 0..rounds {
        let dir = args.data.join("bench-data").join("crash-store");
        let _ = std::fs::remove_dir_all(&dir);

        std::fs::create_dir_all(&dir).unwrap();

        let executable = std::env::current_exe().unwrap();
        let mut child_process = std::process::Command::new(executable)
            .args([
                "child",
                "--dir",
                dir.to_str().unwrap(),
                "--layout",
                layout,
                "--policy",
                &policy.to_string(),
            ])
            .spawn()
            .expect("spawn child");
        let mut rng = SplitMix64::new(SEED ^ (round as u64) << 8 ^ policy as u64);

        std::thread::sleep(std::time::Duration::from_millis(300 + rng.below(900)));

        unsafe { libc::kill(child_process.id() as i32, libc::SIGKILL) };

        let _ = child_process.wait();
        let mut confirmed: Vec<(u64, u64)> = Vec::new();
        let mut tail: Vec<(u64, u64)> = Vec::new();

        if let Ok(file) = std::fs::File::open(dir.join("confirmed.log")) {
            for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
                let fields: Vec<&str> = line.split_whitespace().collect();

                match fields.as_slice() {
                    ["C", _] => confirmed.append(&mut tail),
                    [key, id] => {
                        if let (Ok(key), Ok(id)) = (key.parse(), id.parse()) {
                            tail.push((key, id));
                        }
                    }
                    _ => {}
                }
            }
        }

        let (mut heap, recovery_report) =
            LayoutHeap::open(layout, &dir, heap_config(SyncPolicy::EveryN(policy), true));
        let (mut lost, mut corrupted, mut survivors) = (0u64, 0u64, 0u64);
        let mut out = Vec::new();

        for &(key, id) in &confirmed {
            match heap.read(id, &mut out) {
                Ok(()) if out == make_payload(key, 1024) => {}

                Ok(()) => corrupted += 1,
                Err(_) => lost += 1,
            }
        }

        for &(key, id) in &tail {
            if heap.read(id, &mut out).is_ok() && out == make_payload(key, 1024) {
                survivors += 1;
            }
        }

        csv.row(&[
            layout.into(),
            format!("every{policy}"),
            round.to_string(),
            confirmed.len().to_string(),
            lost.to_string(),
            corrupted.to_string(),
            tail.len().to_string(),
            survivors.to_string(),
            recovery_report.restored_slots.to_string(),
            recovery_report.rebuilt.to_string(),
        ])
        .unwrap();

        println!(
            "kill {layout} every{policy} round {round}: confirmed {}, lost {lost}, corrupted {corrupted}, tail {} survived {survivors}, restored {}",
            confirmed.len(),
            tail.len(),
            recovery_report.restored_slots
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn raw_slot(dir: &Path, id: u64) -> Slot {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(dir.join("index.hs")).unwrap();

    file.seek(SeekFrom::Start(Slot::file_offset(id))).unwrap();

    let mut bytes = [0u8; 16];

    file.read_exact(&mut bytes).unwrap();

    Slot::decode(&bytes).expect("slot")
}

fn flip(dir: &Path, offset: u64) {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.join("data.hs"))
        .unwrap();

    file.seek(SeekFrom::Start(offset)).unwrap();

    let mut bytes = [0u8; 1];

    file.read_exact(&mut bytes).unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(&[bytes[0] ^ 0xFF]).unwrap();
    file.sync_all().unwrap();
}

fn corrupt(args: &Args, csv: &mut Csv) {
    let record_count = 5000u64;

    for layout in ["append", "bucket"] {
        for scenario in ["payload-flip", "header-flip", "truncate", "drop-index"] {
            let dir = args.data.join("bench-data").join("corrupt-store");
            let _ = std::fs::remove_dir_all(&dir);
            let mut ids = Vec::new();

            {
                let (mut heap, _) =
                    LayoutHeap::open(layout, &dir, heap_config(SyncPolicy::None, true));

                for key in 0..record_count {
                    ids.push(heap.insert_into(key % BUCKET_COUNT, &make_payload(key, 1024)));
                }

                heap.flush();
            }

            let header_size = if layout == "bucket" { 38 } else { 30 };

            match scenario {
                "payload-flip" => flip(&dir, raw_slot(&dir, ids[2500]).offset + header_size + 500),
                "header-flip" => flip(&dir, raw_slot(&dir, ids[2500]).offset),
                "truncate" => {
                    let slot = raw_slot(&dir, *ids.last().unwrap());
                    let file = std::fs::OpenOptions::new()
                        .write(true)
                        .open(dir.join("data.hs"))
                        .unwrap();

                    file.set_len(slot.offset + slot.total_len / 2).unwrap();
                }
                _ => {
                    std::fs::remove_file(dir.join("index.hs")).unwrap();

                    if layout == "bucket" {
                        let _ = std::fs::remove_file(dir.join("registry.hs"));
                    }
                }
            }

            let started = Instant::now();
            let stats = if scenario != "drop-index" {
                let mut io = SyncIo::new();
                let layout_id = if layout == "bucket" {
                    LAYOUT_BUCKET
                } else {
                    LAYOUT_APPEND
                };

                heapstore::rebuild(
                    &dir,
                    &mut io,
                    layout_id,
                    true,
                    &heap_config(SyncPolicy::None, true),
                )
                .unwrap()
            } else {
                let (_heap, recovery_report) =
                    LayoutHeap::open(layout, &dir, heap_config(SyncPolicy::None, true));

                recovery_report.rebuild
            };
            let (mut heap, _) = LayoutHeap::open(layout, &dir, heap_config(SyncPolicy::None, true));
            let elapsed_ms = started.elapsed().as_millis();
            let (mut lost, mut corrupted) = (0u64, 0u64);
            let mut out = Vec::new();

            for (key, &id) in ids.iter().enumerate() {
                match heap.read(id, &mut out) {
                    Ok(()) if out == make_payload(key as u64, 1024) => {}

                    Ok(()) => corrupted += 1,
                    Err(_) => lost += 1,
                }
            }

            csv.row(&[
                layout.into(),
                scenario.into(),
                record_count.to_string(),
                lost.to_string(),
                corrupted.to_string(),
                stats.resyncs.to_string(),
                stats.records.to_string(),
                elapsed_ms.to_string(),
            ])
            .unwrap();

            println!(
                "corrupt {layout} {scenario}: lost {lost}, corrupted {corrupted}, resyncs {}, scanned {} records",
                stats.resyncs, stats.records
            );

            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

#[path = "crash/costs.rs"]
mod costs;

use costs::{cost_integrity, cost_sync, crc_speed, recovery_time, sync_shape};

struct Args {
    data: PathBuf,
    out: PathBuf,
    layout: String,
    policy: u32,
    rounds: u32,
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let command = argv.first().cloned().unwrap_or_else(|| "help".into());
    let mut args = Args {
        data: PathBuf::from("."),
        out: PathBuf::from("metrics"),
        layout: String::new(),
        policy: 1,
        rounds: 3,
    };
    let mut argument_index = 1;

    while argument_index < argv.len() {
        match (argv[argument_index].as_str(), argv.get(argument_index + 1)) {
            ("--dir", Some(value)) => args.data = value.into(),
            ("--out", Some(value)) => args.out = value.into(),
            ("--layout", Some(value)) => args.layout = value.clone(),
            ("--policy", Some(value)) => args.policy = value.parse().expect("policy"),
            ("--rounds", Some(value)) => args.rounds = value.parse().expect("rounds"),
            (flag, _) => panic!("unknown argument {flag}"),
        }

        argument_index += 2;
    }

    if command == "child" {
        child(&args.data, &args.layout, args.policy);
    }

    std::fs::create_dir_all(&args.out).expect("out dir");

    match command.as_str() {
        "kill" => {
            let mut csv = Csv::open(
                &args.out.join("kill.csv"),
                "layout,policy,round,confirmed,lost,corrupted_served,unconfirmed_tail,tail_survived,restored_slots,rebuilt",
            )
            .unwrap();
            let layouts: Vec<&str> = if args.layout.is_empty() {
                vec!["append", "bucket"]
            } else {
                vec![args.layout.as_str()]
            };

            for layout in layouts {
                for policy in [1u32, 32] {
                    kill_rounds(&args, layout, policy, args.rounds, &mut csv);
                }
            }
        }
        "corrupt" => {
            let mut csv = Csv::open(
                &args.out.join("corrupt.csv"),
                "layout,scenario,records,lost,corrupted_served,resyncs,rebuilt_records,wall_ms",
            )
            .unwrap();

            corrupt(&args, &mut csv);
        }
        "cost-sync" => {
            let mut csv = Csv::open(
                &args.out.join("cost_sync.csv"),
                "layout,policy,iters,p50_us,p99_us,ops_per_s,write_amp,syncs",
            )
            .unwrap();

            cost_sync(&args, &mut csv);
        }
        "cost-integrity" => {
            let mut csv = Csv::open(
                &args.out.join("cost_integrity.csv"),
                "layout,size,crc,insert_med,insert_min,insert_max,read_med,read_min,read_max",
            )
            .unwrap();

            cost_integrity(&args, &mut csv);
        }
        "sync-shape" => {
            let mut csv = Csv::open(
                &args.out.join("sync_shape.csv"),
                "regions,ops_per_s,write_amp,syncs,ms_per_sync",
            )
            .unwrap();

            sync_shape(&args, &mut csv);
        }
        "crc-speed" => {
            let mut csv = Csv::open(
                &args.out.join("crc_speed.csv"),
                "bytes,gb_per_s,us_per_call,checksum",
            )
            .unwrap();

            crc_speed(&args, &mut csv);
        }
        "recovery-time" => {
            let mut csv = Csv::open(
                &args.out.join("recovery_time.csv"),
                "layout,records,bytes,clean_open_ms,rebuild_ms,mb_per_s,rebuilt_records",
            )
            .unwrap();

            recovery_time(&args, &mut csv);
        }
        _ => {
            eprintln!(
                "commands: kill | corrupt | cost-sync | cost-integrity | sync-shape | crc-speed | recovery-time"
            );

            std::process::exit(2);
        }
    }
}
