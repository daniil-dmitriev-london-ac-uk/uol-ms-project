use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Instant;

struct Config {
    path: PathBuf,
    batches: u64,
    records_per_batch: usize,
    record_bytes: usize,
}

fn config() -> Config {
    let args: Vec<String> = std::env::args().skip(1).collect();
    Config {
        path: args.first().map(PathBuf::from).unwrap_or_else(|| "heap.data".into()),
        batches: args.get(1).and_then(|v| v.parse().ok()).unwrap_or(256),
        records_per_batch: args.get(2).and_then(|v| v.parse().ok()).unwrap_or(16),
        record_bytes: args.get(3).and_then(|v| v.parse().ok()).unwrap_or(4096),
    }
}


fn nextRandom(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut value = *state;
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}


fn randomData(state: &mut u64, data: &mut Vec<u8>, records: usize, record_bytes: usize) {
    data.clear();
    data.resize(records * record_bytes, 0);

    for chunk in data.chunks_mut(8) {
        let value = nextRandom(state).to_le_bytes();
        chunk.copy_from_slice(&value[..chunk.len()]);
    }
}


fn writeBatch(file: &mut std::fs::File, batch: &[u8]) -> io::Result<()> {
    file.write_all(batch)
}



fn main() -> io::Result<()> {
    let cfg = config();

    let mut state = 0x1234_5678_9ABC_DEF0;

    let mut batch = Vec::with_capacity( cfg.records_per_batch * cfg.record_bytes );

    let mut file = OpenOptions::new().create(true).truncate(true).write(true).open(&cfg.path)?;

    let started = Instant::now();
    let mut generated = 0u64;

    for _ in 0..cfg.batches {
        randomData(&mut state, &mut batch, cfg.records_per_batch, cfg.record_bytes);
        writeBatch(&mut file, &batch)?;
        generated += batch.len() as u64;
    }

    file.sync_data()?;

    let seconds = started.elapsed().as_secs_f64();

    println!("path={} bytes={generated} seconds={seconds:.3} mib_s={:.2}", cfg.path.display(), generated as f64 / seconds / 1_048_576.0);

    Ok(())
}
