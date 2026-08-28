use super::*;

pub(super) fn read_tests(args: &Args, csv: &mut Csv) {
    for (layout, size, size_name) in ["append", "bucket"]
        .iter()
        .flat_map(|layout| SIZES.iter().map(move |&(size, name)| (*layout, size, name)))
    {
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

        for (_, population, buckets, widths) in classes {
            if population < *widths.iter().max().unwrap() {
                continue;
            }

            let selected = widths.iter().any(|width| {
                ["sync", "uring"].iter().any(|io| {
                    ["arbitrary", "grouped"].iter().any(|pattern| {
                        format!("{layout},{io},read,{pattern},{width},{size_name}")
                            .contains(&args.filter)
                    })
                })
            });

            if !selected {
                continue;
            }

            let dir = args.data.join("bench-data").join("read-store");
            let ids = populate(layout, &dir, size, population, buckets);

            for io in ["sync", "uring"] {
                let mut heap = BenchmarkHeap::open(
                    layout,
                    io,
                    &dir,
                    config_for(size * population, SyncPolicy::None),
                );

                for &width in &widths {
                    let patterns: &[&str] = if width == 1 {
                        &["arbitrary"]
                    } else {
                        &["arbitrary", "grouped"]
                    };

                    for pattern in patterns {
                        let tag = [layout, io, "read", pattern, &width.to_string(), size_name];

                        if !tag.join(",").contains(&args.filter) {
                            continue;
                        }

                        if *pattern == "grouped" && population / buckets < width {
                            continue;
                        }

                        let mut repetition_results = Vec::new();

                        for repetition in 0..args.repetitions {
                            let mut rng = SplitMix64::new(0xC0FFEE + repetition as u64);
                            let mut out = Vec::new();
                            let mut outs = Vec::new();
                            let mut picked = vec![0u64; width as usize];
                            let mut pool: Vec<u32> = (0..population as u32).collect();

                            heap.reset_counters();

                            let mut latencies = run_repetition(width * size, |_| {
                                if *pattern == "grouped" {
                                    let bucket = rng.below(buckets);
                                    let start = rng.below(population / buckets - width + 1);

                                    for (index, selected) in picked.iter_mut().enumerate() {
                                        *selected = ids
                                            [((start + index as u64) * buckets + bucket) as usize];
                                    }
                                } else {
                                    for index in 0..width as usize {
                                        let random =
                                            index as u64 + rng.below(population - index as u64);

                                        pool.swap(index, random as usize);

                                        picked[index] = ids[pool[index] as usize];
                                    }
                                }

                                let started = Instant::now();

                                if width == 1 {
                                    heap.read(picked[0], &mut out);
                                } else {
                                    heap.read_batch(&picked, &mut outs);
                                }

                                started.elapsed().as_nanos() as u64
                            });

                            repetition_results.push(RepetitionResult {
                                stats: OperationStats::from_samples(&mut latencies),
                                io_counters: heap.counters(),
                            });
                        }

                        summary_row(csv, &tag, &repetition_results, width, width * size);
                    }
                }
            }

            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}



pub(super) fn write_tests(args: &Args, csv: &mut Csv) {
    for (layout, size, size_name) in ["append", "bucket"].iter().flat_map(|layout| SIZES.iter().map(move |&(size, name)| (*layout, size, name)))
    {
        for width in [1u64, 10, 32, 1000] {
            let op_bytes = width * size;

            for io in ["sync", "uring"] {
                let patterns: &[&str] = if width == 1 { &["arbitrary"] } else { &["arbitrary", "grouped"] };

                for pattern in patterns {
                    let tag = [layout, io, "write", pattern, &width.to_string(), size_name];

                    if !tag.join(",").contains(&args.filter) {
                        continue;
                    }

                    let buckets = if size >= 7 << 20 { 8 } else { 16 };

                    if width.min(2 * buckets) * size + op_bytes > WRITE_BUDGET {
                        continue;
                    }

                    let dir = args.data.join("bench-data").join("write-store");
                    let warm = width.min(2 * buckets);
                    
                    let fresh = |written: &mut u64| {
                        let _ = std::fs::remove_dir_all(&dir);
                        
                        let mut heap = BenchmarkHeap::open(layout, io, &dir, config_for(WRITE_BUDGET, SyncPolicy::EveryN(width as u32)));

                        let mut payload = vec![0u8; size as usize];

                        SplitMix64::new(7).fill(&mut payload);

                        for index in 0..warm {
                            heap.insert_into(index % buckets, &payload);
                        }

                        heap.flush();

                        *written = warm * size;

                        (heap, payload)
                    };


                    let mut written = 0u64;
                    let (mut heap, payload) = fresh(&mut written);
                    let mut repetition_results = Vec::new();

                    for _ in 0..args.repetitions {
                        if written + op_bytes * 4 > WRITE_BUDGET {
                            drop(heap);

                            let (next, _) = fresh(&mut written);

                            heap = next;
                        }

                        heap.reset_counters();

                        let budget = WRITE_BUDGET.saturating_sub(written);
                        let mut used = 0u64;
                        let mut latencies = run_repetition(op_bytes, |iteration| {
                            if used + op_bytes > budget {
                                return 1;
                            }

                            let started = Instant::now();

                            for index in 0..width {
                                let bucket = match *pattern {
                                    "grouped" => iteration % buckets,
                                    _ => (iteration * width + index) % buckets,
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
                            io_counters: heap.counters()
                        });
                    }

                    summary_row(csv, &tag, &repetition_results, width, op_bytes);

                    drop(heap);

                    let _ = std::fs::remove_dir_all(&dir);




                    
                }
            }
        }
    }


}


