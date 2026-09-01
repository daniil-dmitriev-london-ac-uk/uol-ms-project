use super::*;

pub(super) fn read_tests(args: &Args, csv: &mut Csv) {
    for (layout, size, size_name) in ["append", "bucket"].iter().flat_map(|layout_name| {
        SIZES
            .iter()
            .map(move |&(size, size_name)| (*layout_name, size, size_name))
    }) {
        let classes: [(&str, u64, u64, Vec<u64>); 2] = [
            (
                "small",
                (POPULATION_BUDGET / (size + 64)).min(1024),
                if size >= 7 << 20 { 8 } else { 16 },
                vec![1, 10, 32],
            ),
            (
                "large",
                (LARGE_POPULATION_BUDGET / (size + 64)).min(2000),
                2,
                vec![1000],
            ),
        ];

        for (_, population, bucket_count, records) in classes {
            if population < *records.iter().max().unwrap() {
                for operation_width in &records {
                    eprintln!(
                        "skip read {layout} {size_name} operation_width={operation_width}: workload does not fit"
                    );
                }

                continue;
            }

            let need_run = records.iter().any(|operation_width| {
                ["sync", "uring"].iter().any(|io| {
                    ["arbitrary", "grouped"].iter().any(|pattern| {
                        let filter_entry =
                            format!("{layout},{io},read,{pattern},{operation_width},{size_name}");

                        filter_entry.contains(&args.filter)
                    })
                })
            });

            if !need_run {
                continue;
            }

            let dir = args.data.join("bench-data").join("read-store");

            eprintln!(
                "populate {layout} {size_name}: {population} records, {bucket_count} buckets"
            );

            let ids = populate(layout, &dir, size, population, bucket_count);

            for io in ["sync", "uring"] {
                let mut heap = BenchmarkHeap::open(
                    layout,
                    io,
                    &dir,
                    config_for(size * population, SyncPolicy::None),
                );

                for &operation_width in &records {
                    let patterns: &[&str] = if operation_width == 1 {
                        &["arbitrary"]
                    } else {
                        &["arbitrary", "grouped"]
                    };

                    for pattern in patterns {
                        let tag = [
                            layout,
                            io,
                            "read",
                            pattern,
                            &operation_width.to_string(),
                            size_name,
                        ];

                        if !tag.join(",").contains(&args.filter) {
                            continue;
                        }

                        if *pattern == "grouped" && population / bucket_count < operation_width {
                            eprintln!("skip {}: bucket is smaller than the sample", tag.join(","));

                            continue;
                        }

                        let mut repetition_results = Vec::new();

                        for repetition in 0..args.repetitions {
                            let mut rng = SplitMix64::new(0xC0FFEE + repetition as u64);
                            let mut out = Vec::new();
                            let mut outputs = Vec::new();
                            let mut selected_ids = vec![0u64; operation_width as usize];
                            let mut pool: Vec<u32> = (0..population as u32).collect();

                            heap.reset_counters();

                            let latencies = run_repetition(operation_width * size, |_| {
                                if *pattern == "grouped" {
                                    let bucket = rng.below(bucket_count);
                                    let start =
                                        rng.below(population / bucket_count - operation_width + 1);

                                    for (selection_index, selected_id) in
                                        selected_ids.iter_mut().enumerate()
                                    {
                                        *selected_id = ids[((start + selection_index as u64)
                                            * bucket_count
                                            + bucket)
                                            as usize];
                                    }
                                } else {
                                    for selection_index in 0..operation_width as usize {
                                        let random_index = selection_index as u64
                                            + rng.below(population - selection_index as u64);

                                        pool.swap(selection_index, random_index as usize);

                                        selected_ids[selection_index] =
                                            ids[pool[selection_index] as usize];
                                    }
                                }

                                let started = Instant::now();

                                if operation_width == 1 {
                                    heap.read(selected_ids[0], &mut out);
                                } else {
                                    heap.read_batch(&selected_ids, &mut outputs);
                                }

                                started.elapsed().as_nanos() as u64
                            });
                            let mut latencies = latencies;

                            repetition_results.push(RepetitionResult {
                                stats: OperationStats::from_samples(&mut latencies),
                                io_counters: heap.counters(),
                            });

                            heap.reset_counters();
                        }

                        summary_row(
                            csv,
                            &tag,
                            &repetition_results,
                            operation_width,
                            operation_width * size,
                        );
                    }
                }
            }

            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

pub(super) fn write_tests(args: &Args, csv: &mut Csv) {
    for (layout, size, size_name) in ["append", "bucket"].iter().flat_map(|layout_name| {
        SIZES
            .iter()
            .map(move |&(size, size_name)| (*layout_name, size, size_name))
    }) {
        for operation_width in [1u64, 10, 32, 1000] {
            let op_bytes = operation_width * size;

            for io in ["sync", "uring"] {
                let patterns: &[&str] = if operation_width == 1 {
                    &["arbitrary"]
                } else {
                    &["arbitrary", "grouped"]
                };

                for pattern in patterns {
                    let tag = [
                        layout,
                        io,
                        "write",
                        pattern,
                        &operation_width.to_string(),
                        size_name,
                    ];

                    if !tag.join(",").contains(&args.filter) {
                        continue;
                    }

                    let bucket_count: u64 = if size >= 7 << 20 { 8 } else { 16 };

                    if operation_width.min(2 * bucket_count) * size + op_bytes > WRITE_BUDGET {
                        eprintln!(
                            "skip {}: operation exceeds the device budget",
                            tag.join(",")
                        );

                        continue;
                    }

                    let dir = args.data.join("bench-data").join("write-store");
                    let mut written = 0u64;
                    let warmup_count = operation_width.min(2 * bucket_count);
                    let fresh = |written: &mut u64| {
                        let _ = std::fs::remove_dir_all(&dir);
                        let mut heap = BenchmarkHeap::open(
                            layout,
                            io,
                            &dir,
                            config_for(WRITE_BUDGET, SyncPolicy::EveryN(operation_width as u32)),
                        );
                        let mut payload = vec![0u8; size as usize];

                        SplitMix64::new(7).fill(&mut payload);

                        for warmup_key in 0..warmup_count {
                            heap.insert_into(warmup_key % bucket_count, &payload);
                        }

                        heap.flush();

                        *written = warmup_count * size;

                        (heap, payload)
                    };
                    let (mut heap, payload) = fresh(&mut written);
                    let mut repetition_results = Vec::new();

                    for _ in 0..args.repetitions {
                        if written + op_bytes * 4 > WRITE_BUDGET {
                            drop(heap);

                            let (new_heap, _) = fresh(&mut written);

                            heap = new_heap;
                        }

                        heap.reset_counters();

                        let budget = WRITE_BUDGET.saturating_sub(written);
                        let mut used = 0u64;
                        let mut latencies = run_repetition(op_bytes, |iteration| {
                            if used + op_bytes > budget {
                                return 1;
                            }

                            let started = Instant::now();

                            for record_index in 0..operation_width {
                                let bucket = match *pattern {
                                    "grouped" => iteration % bucket_count,
                                    _ => {
                                        (iteration * operation_width + record_index) % bucket_count
                                    }
                                };

                                heap.insert_into(bucket, &payload);
                            }

                            used += op_bytes;

                            started.elapsed().as_nanos() as u64
                        });

                        latencies.retain(|&latency| latency > 1);

                        if latencies.is_empty() {
                            latencies.push(1);
                        }
                        written += used;
                        repetition_results.push(RepetitionResult {
                            stats: OperationStats::from_samples(&mut latencies),
                            io_counters: heap.counters(),
                        });
                    }

                    summary_row(csv, &tag, &repetition_results, operation_width, op_bytes);

                    drop(heap);

                    let _ = std::fs::remove_dir_all(&dir);
                }
            }
        }
    }
}
