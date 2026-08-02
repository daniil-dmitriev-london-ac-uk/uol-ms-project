mod append;
mod bucket;
mod placement;
mod store;

use append::AppendPlacement;
use bucket::BucketPlacement;
use store::Store;

use std::io;
use std::path::Path;

fn main() -> io::Result<()> {
    let payload = vec![7u8; 4096];

    let mut append = Store::open(Path::new("append-generic"), AppendPlacement::new(0, 0))?;
    let mut bucket = Store::open(Path::new("bucket-generic"), BucketPlacement::new(3))?;
    let append_id = append.insert(&payload)?;
    let bucket_id = bucket.insert(&payload)?;

    append.flush()?;
    bucket.flush()?;

    let mut out = Vec::new();
    append.read(append_id, &mut out)?;
    bucket.read(bucket_id, &mut out)?;

    println!("generic_store_sample_bytes={}", out.len());

    Ok(())
}
