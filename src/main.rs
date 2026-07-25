mod append;
mod bucket;

use std::io;
use std::path::Path;
use std::time::Instant;

fn run_append(records: u64, bytes: usize) -> io::Result<()> {
    let mut store = append::open_append(Path::new("append-store"))?;

    let payload = vec![7u8; bytes];

    let started = Instant::now();

    let mut ids = Vec::new();

    for _ in 0..records {
        ids.push(append::append_record(&mut store, &payload)?);
    }

    append::flush_append(&mut store)?;
    let mut out = Vec::new();
    append::read_append(&mut store, ids[ids.len() / 2], &mut out)?;

    println!("layout=append records={records} mib_s={:.2}", records as f64 * bytes as f64 / started.elapsed().as_secs_f64() / 1_048_576.0);

    Ok(())
}


fn run_bucket(records: u64, bytes: usize) -> io::Result<()> {
    let mut store = bucket::open_bucket(Path::new("bucket-store"))?;

    let payload = vec![7u8; bytes];

    let started = Instant::now();

    let mut ids = Vec::new();

    for key in 0..records {
        ids.push(bucket::bucket_record(&mut store, key % 8, &payload)?);
    }

    bucket::flush_bucket(&mut store)?;
    let mut out = Vec::new();
    bucket::read_bucket(&mut store, ids[ids.len() / 2], &mut out)?;

    println!("layout=bucket records={records} mib_s={:.2}", records as f64 * bytes as f64 / started.elapsed().as_secs_f64() / 1_048_576.0);

    Ok(())
}



fn main() -> io::Result<()> {
    let layout = std::env::args().nth(1).unwrap_or_else(|| "append".into());
    let records = 4096;
    let bytes = 4096;

    match layout.as_str() {
        "append" => run_append(records, bytes),
        "bucket" => run_bucket(records, bytes),
        _ => Err(io::Error::new(io::ErrorKind::InvalidInput, "layout must be append or bucket")),
    }
}
