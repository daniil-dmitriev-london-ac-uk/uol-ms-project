mod append;
mod bucket;
mod placement;

use placement::Placement;

use std::io;
use std::path::Path;

fn main() -> io::Result<()> {
    let mut allocator = append::AppendPlacement::new(0, 0);
    let _ = allocator.allocate(1024)?;
    let mut bucket_allocator = bucket::BucketPlacement::new(7);
    let _ = bucket_allocator.allocate(1024)?;
    let layout = std::env::args().nth(1).unwrap_or_else(|| "append".into());

    let payload = vec![7u8; 4096];

    if layout == "append" {
        let mut store = append::open_append(Path::new( "append-store" ))?;
        let id = append::append_record(&mut store, &payload)?;

        append::flush_append(&mut store)?;
        let mut out = Vec::new();
        append::read_append(&mut store, id, &mut out)?;

        println!("layout=append sample_bytes={}", out.len());
    } else {
        let mut store = bucket::open_bucket(Path::new("bucket-store"))?;
        let id = bucket::bucket_record(&mut store, 3, &payload)?;

        bucket::flush_bucket(&mut store)?;
        let mut out = Vec::new();
        bucket::read_bucket(&mut store, id, &mut out)?;

        println!("layout=bucket sample_bytes={}", out.len());
    }

    Ok(())
}
