mod append;
mod bucket;

use std::io;
use std::path::Path;

const USE_BUCKET: bool = true;

fn main() -> io::Result<()> {
    let payload = vec![7u8; 4096];

    if USE_BUCKET {
        let mut store = bucket::open_bucket(Path::new("bucket-store"))?;

        let mut ids = Vec::new();

        for key in 0..4096u64 {
            ids.push(bucket::bucket_record(&mut store, key % 8, &payload)?);
        }

        bucket::flush_bucket(&mut store)?;
        let mut out = Vec::new();
        bucket::read_bucket(&mut store, ids[100], &mut out)?;

        println!("layout=bucket records={} sample_bytes={}", ids.len(), out.len());
    } else {
        let mut store = append::open_append(Path::new("append-store"))?;

        let mut ids = Vec::new();

        for _ in 0..4096 {
            ids.push(append::append_record(&mut store, &payload)?);
        }

        append::flush_append(&mut store)?;
        let mut out = Vec::new();
        append::read_append(&mut store, ids[100], &mut out)?;

        println!("layout=append records={} sample_bytes={}", ids.len(), out.len());
    }

    Ok(())
}
