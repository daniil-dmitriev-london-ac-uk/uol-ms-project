mod append;

use append::{append_record, flush_append, open_append, read_append};

use std::io;
use std::path::Path;
use std::time::Instant;

fn main() -> io::Result<()> {
    let mut store = open_append(Path::new("append-store"))?;

    let payload = vec![7u8; 4096];

    let started = Instant::now();

    let mut ids = Vec::new();

    for _ in 0..4096 {
        ids.push(append_record(&mut store, &payload)?);
    }

    flush_append(&mut store)?;

    let mut out = Vec::new();
    read_append(&mut store, ids[ids.len() / 2], &mut out)?;

    println!("records={} sample_bytes={} seconds={:.3}", ids.len(), out.len(), started.elapsed().as_secs_f64());

    Ok(())
}
