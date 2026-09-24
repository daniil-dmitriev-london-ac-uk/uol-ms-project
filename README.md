# Database Heap Table Storage Subsystem 

Build, Test, and Measurement Commands

## Requirements

- Linux with `io_uring` support (kernel 5.6+);
- Rust (`rustc` and `cargo`).

Target NVMe partition for measurements:

```bash
export NV=/media/d/cc03a91f-df77-47e7-a5b1-61a9599e4309
```



## Build

```bash
cargo build --release          # library and the bench and crash binaries
cargo clippy --all-targets     # static analysis
```



## Tests

Run unit tests, integration tests, and recovery scenarios:

```bash
cargo test                              # all tests, data is stored in target/testdata
cargo test --release                    # same tests with optimizations
HEAPSTORE_TEST_DIR=/path cargo test     # override the test data directory
```

Run individual groups:

```bash
cargo test --test unit         # crc, SplitMix64, on-disk formats
cargo test --test store        # round trips, sync/uring parity, bucket adjacency
cargo test --test recovery     # tail recovery, corruption, truncation, index loss
cargo test grant_order         # bucket allocator victim order
```



## Prepare the Host for Measurements

```bash
sudo cpupower frequency-set -g performance     # set the CPU governor to performance
df -h $NV                                      # partition must have at least 2.5 GB free
```


## Measurement Matrix

Reads and writes are confined to `$NV/bench-data`. Results are saved to `metrics/`.

```bash
./target/release/bench matrix --dir $NV --out metrics --reps 3
    # Full matrix: "layout x I/O x read/write x arbitrary/grouped" ->
    # x {1, 10, 32, 1000} records x { 1K, 5K, 18K, 64K, 512K, 2M, 7M }
./target/release/bench matrix --dir $NV --out metrics --filter bucket,uring,read
    # --filter matches a substring of the configuration tag for a selective run
./target/release/bench space   --dir $NV --out metrics   # space usage
./target/release/bench updates --dir $NV --out metrics   # update throughput
```


## Reliability Tests

```bash
./target/release/crash kill --dir $NV --out metrics --rounds 3
    # SIGKILL at a random time
./target/release/crash corrupt --dir $NV --out metrics
    # payload byte corruption, header corruption, file truncation, index deletion
./target/release/crash cost-sync --dir $NV --out metrics
    # flush policy cost: none / every32 / every8 / every1 (ops/s, p99, wear)
./target/release/crash cost-integrity --dir $NV --out metrics
    # runs with CRC enabled and disabled alternate within each of the five rounds
./target/release/crash sync-shape --dir $NV --out metrics
    # controlled experiment: the same workload touches different numbers of file regions. Shows what determines fdatasync cost: the number of dirty pages, rather than data volume
./target/release/crash crc-speed --dir $NV --out metrics
    # baseline CRC speed (GB/s, microseconds per call)
```




### Time to fully rebuild the index for 10,000 / 50,000 / 200,000 / 500,000 records
```bash
./target/release/crash recovery-time --dir $NV --out metrics
```


## Results

| File | Contents |
| --- | --- |
| `metrics/matrix.csv` | Raw rows from the Section 5 matrix |
| `metrics/space.csv` | Space usage: files, physical blocks, utilization |
| `metrics/updates.csv` | Update throughput |
| `metrics/kill.csv` | SIGKILL results: acknowledged, lost, corrupted, and recovered records |
| `metrics/corrupt.csv` | Corruption scenarios: losses and resynchronizations |
| `metrics/cost_sync.csv` | Flush cost |
| `metrics/cost_integrity.csv` | CRC cost |
| `metrics/sync_shape.csv` | Flush cost versus the number of file regions touched |
| `metrics/crc_speed.csv` | CRC speed |
| `metrics/recovery_time.csv` | Index rebuild time |
| `metrics/run.log` | Full run log |



## Run All Measurements

```bash
./target/release/bench matrix --dir $NV --out metrics --reps 3 && \
./target/release/bench space --dir $NV --out metrics && \
./target/release/bench updates --dir $NV --out metrics && \
./target/release/crash cost-sync --dir $NV --out metrics && \
./target/release/crash cost-integrity --dir $NV --out metrics && \
./target/release/crash sync-shape --dir $NV --out metrics && \
./target/release/crash crc-speed --dir $NV --out metrics && \
./target/release/crash recovery-time --dir $NV --out metrics && \
./target/release/crash kill --dir $NV --out metrics --rounds 3 && \
./target/release/crash corrupt --dir $NV --out metrics
```
