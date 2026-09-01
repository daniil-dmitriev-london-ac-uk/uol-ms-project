use heapstore::measure::Csv;
use heapstore::rng::payload_for;

use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    if argv.first().map(String::as_str) != Some("crc-speed") {
        eprintln!("command: crc-speed; optional argument: --out DIR");

        std::process::exit(2);
    }

    let mut out = PathBuf::from("metrics");

    if argv.get(1).map(String::as_str) == Some("--out") {
        out = argv.get(2).expect("output directory").into();
    }

    let mut csv = Csv::open(
        &out.join("crc-speed.csv"),
        "size,gb_per_s,us_per_call,checksum",
    )
    .expect("csv");

    for size in [1024usize, 65536] {
        let mut data = Vec::new();

        payload_for(0xDEAD_2026, 4, size, &mut data);

        let iterations = 2_000_000_000usize / size;
        let started = Instant::now();
        let mut checksum = 0u32;

        for _ in 0..iterations {
            checksum ^= heapstore::crc32::crc32c(&data);
        }

        let elapsed = started.elapsed().as_secs_f64();
        let per_call_us = elapsed / iterations as f64 * 1e6;
        let gb_per_s = (size * iterations) as f64 / elapsed / 1e9;

        csv.row(&[
            size.to_string(),
            format!("{gb_per_s:.2}"),
            format!("{per_call_us:.2}"),
            checksum.to_string(),
        ])
        .expect("row");
    }
}
