use super::*;

pub(super) fn cost_sync(args: &Args, csv: &mut Csv) {
    for layout in ["append", "bucket"] {
        for (policy, policy_name) in [
            (SyncPolicy::None, "none"),
            (SyncPolicy::EveryN(32), "every32"),
            (SyncPolicy::EveryN(8), "every8"),
            (SyncPolicy::EveryN(1), "every1"),
        ] {
            let dir = args.data.join("bench-data").join("cost-store");
            let _ = std::fs::remove_dir_all(&dir);
            let (mut heap, _) = LayoutHeap::open(layout, &dir, heap_config(policy, true));
            let payload = make_payload(1, 1024);

            for warmup_key in 0..64 {
                heap.insert_into(warmup_key % BUCKET_COUNT, &payload);
            }

            heap.flush();

            heap.reset_counters();

            let benchmark_started = Instant::now();
            let mut latencies = Vec::new();
            let mut key = 64u64;

            while benchmark_started.elapsed().as_secs_f64() < 6.0 && latencies.len() < 30_000 {
                let started = Instant::now();

                heap.insert_into(key % BUCKET_COUNT, &payload);

                latencies.push(started.elapsed().as_nanos() as u64);
                key += 1;
            }

            heap.flush();

            let elapsed_seconds = benchmark_started.elapsed().as_secs_f64();
            let counters = heap.counters();
            let stats = OperationStats::from_samples(&mut latencies);
            let operations_per_second = stats.iterations as f64 / elapsed_seconds;
            let write_amplification =
                counters.write_bytes as f64 / (stats.iterations * 1024) as f64;

            csv.row(&[
                layout.into(),
                policy_name.into(),
                stats.iterations.to_string(),
                format!("{:.1}", stats.p50_ns as f64 / 1000.0),
                format!("{:.1}", stats.p99_ns as f64 / 1000.0),
                format!("{operations_per_second:.1}"),
                format!("{:.2}", write_amplification),
                counters.syncs.to_string(),
            ])
            .unwrap();

            println!(
                "cost-sync {layout} {policy_name}: ops/s={:.0} p99={:.0}us WA={:.2}",
                operations_per_second,
                stats.p99_ns as f64 / 1000.0,
                write_amplification
            );

            drop(heap);

            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

pub(super) fn cost_integrity(args: &Args, csv: &mut Csv) {
    const ROUNDS: usize = 5;

    for layout in ["append", "bucket"] {
        for (size, size_name) in [(1024usize, "1K"), (65536, "64K")] {
            let record_count = if size > 4096 { 3000u64 } else { 20_000 };
            let mut write_rates: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
            let mut read_rates: [Vec<f64>; 2] = [Vec::new(), Vec::new()];

            for _round in 0..ROUNDS {
                for (integrity_index, integrity) in [true, false].into_iter().enumerate() {
                    let dir = args.data.join("bench-data").join("integ-store");
                    let _ = std::fs::remove_dir_all(&dir);
                    let (mut heap, _) =
                        LayoutHeap::open(layout, &dir, heap_config(SyncPolicy::None, integrity));
                    let payload = make_payload(2, size);
                    let started = Instant::now();
                    let ids: Vec<u64> = (0..record_count)
                        .map(|key| heap.insert_into(key % BUCKET_COUNT, &payload))
                        .collect();

                    heap.flush();

                    write_rates[integrity_index]
                        .push(record_count as f64 / started.elapsed().as_secs_f64());

                    let mut rng = SplitMix64::new(5);
                    let mut out = Vec::new();
                    let reads = record_count.min(8000);
                    let started = Instant::now();

                    for _ in 0..reads {
                        heap.read(ids[rng.below(record_count) as usize], &mut out)
                            .unwrap();
                    }

                    read_rates[integrity_index]
                        .push(reads as f64 / started.elapsed().as_secs_f64());

                    drop(heap);

                    let _ = std::fs::remove_dir_all(&dir);
                }
            }

            for (integrity_index, integrity) in [true, false].into_iter().enumerate() {
                let (mut sorted_write_rates, mut sorted_read_rates) = (
                    write_rates[integrity_index].clone(),
                    read_rates[integrity_index].clone(),
                );
                let (median_write_rate, median_read_rate) = (
                    median_f64(&mut sorted_write_rates),
                    median_f64(&mut sorted_read_rates),
                );

                csv.row(&[
                    layout.into(),
                    size_name.into(),
                    integrity.to_string(),
                    format!("{median_write_rate:.1}"),
                    format!("{:.1}", sorted_write_rates[0]),
                    format!("{:.1}", sorted_write_rates[ROUNDS - 1]),
                    format!("{median_read_rate:.1}"),
                    format!("{:.1}", sorted_read_rates[0]),
                    format!("{:.1}", sorted_read_rates[ROUNDS - 1]),
                ])
                .unwrap();

                println!(
                    "cost-integrity {layout} {size_name} crc={integrity}: insert/s={median_write_rate:.0} ({:.0}..{:.0}) read/s={median_read_rate:.0} ({:.0}..{:.0})",
                    sorted_write_rates[0],
                    sorted_write_rates[ROUNDS - 1],
                    sorted_read_rates[0],
                    sorted_read_rates[ROUNDS - 1]
                );
            }
        }
    }
}

pub(super) fn sync_shape(args: &Args, csv: &mut Csv) {
    for regions in [1u64, 2, 4, 8, 16] {
        let dir = args.data.join("bench-data").join("shape-store");
        let _ = std::fs::remove_dir_all(&dir);
        let (mut heap, _) =
            LayoutHeap::open("bucket", &dir, heap_config(SyncPolicy::EveryN(8), true));
        let payload = make_payload(1, 1024);

        for warmup_key in 0..64 {
            heap.insert_into(warmup_key % regions, &payload);
        }

        heap.flush();

        heap.reset_counters();

        let started = Instant::now();

        for key in 64..8064u64 {
            heap.insert_into(key % regions, &payload);
        }

        heap.flush();

        let elapsed_seconds = started.elapsed().as_secs_f64();
        let counters = heap.counters();
        let sync_count = counters.syncs.max(1);

        csv.row(&[
            regions.to_string(),
            format!("{:.1}", 8000.0 / elapsed_seconds),
            format!("{:.2}", counters.write_bytes as f64 / (8000 * 1024) as f64),
            counters.syncs.to_string(),
            format!("{:.2}", elapsed_seconds * 1000.0 / sync_count as f64),
        ])
        .unwrap();

        println!(
            "sync-shape {regions} regions: {:.0} ops/s, amplification {:.2}, {} syncs at {:.2} ms",
            8000.0 / elapsed_seconds,
            counters.write_bytes as f64 / (8000 * 1024) as f64,
            counters.syncs,
            elapsed_seconds * 1000.0 / sync_count as f64
        );

        drop(heap);

        let _ = std::fs::remove_dir_all(&dir);
    }
}

pub(super) fn crc_speed(args: &Args, csv: &mut Csv) {
    for size in [1024usize, 65536] {
        let payload = make_payload(4, size);
        let iterations = 2_000_000_000usize / size;
        let started = Instant::now();
        let mut checksum = 0u32;

        for _ in 0..iterations {
            checksum ^= heapstore::crc32::crc32c(&payload);
        }

        let elapsed_seconds = started.elapsed().as_secs_f64();
        let microseconds_per_call = elapsed_seconds / iterations as f64 * 1e6;
        let gigabytes_per_second = (size * iterations) as f64 / elapsed_seconds / 1e9;

        csv.row(&[
            size.to_string(),
            format!("{gigabytes_per_second:.2}"),
            format!("{microseconds_per_call:.2}"),
            format!("{checksum}"),
        ])
        .unwrap();

        println!(
            "crc-speed {size} bytes: {gigabytes_per_second:.2} gb/s, {microseconds_per_call:.2} us per call"
        );
    }

    let _ = args;
}

pub(super) fn recovery_time(args: &Args, csv: &mut Csv) {
    for layout in ["append", "bucket"] {
        for count in [10_000u64, 50_000, 200_000, 500_000] {
            let dir = args.data.join("bench-data").join("rec-store");
            let _ = std::fs::remove_dir_all(&dir);

            {
                let (mut heap, _) =
                    LayoutHeap::open(layout, &dir, heap_config(SyncPolicy::None, true));
                let payload = make_payload(3, 1024);

                for key in 0..count {
                    heap.insert_into(key % BUCKET_COUNT, &payload);
                }

                heap.flush();
            }

            let started = Instant::now();
            let (heap, _) = LayoutHeap::open(layout, &dir, heap_config(SyncPolicy::None, true));
            let clean_open_ms = started.elapsed().as_millis();

            drop(heap);

            std::fs::remove_file(dir.join("index.hs")).unwrap();

            if layout == "bucket" {
                let _ = std::fs::remove_file(dir.join("registry.hs"));
            }

            let started = Instant::now();
            let (_heap, recovery_report) =
                LayoutHeap::open(layout, &dir, heap_config(SyncPolicy::None, true));
            let rebuild_ms = started.elapsed().as_millis();
            let bytes = count * (1024 + if layout == "bucket" { 38 } else { 30 });

            csv.row(&[
                layout.into(),
                count.to_string(),
                bytes.to_string(),
                clean_open_ms.to_string(),
                rebuild_ms.to_string(),
                format!("{:.1}", bytes as f64 / 1e6 / (rebuild_ms as f64 / 1000.0)),
                recovery_report.rebuild.records.to_string(),
            ])
            .unwrap();

            println!(
                "recovery {layout} n={count}: clean open {clean_open_ms}ms, rebuild {rebuild_ms}ms"
            );

            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
