use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

const EXTENT_BYTES: u64 = 1 << 20;
const IDS_PER_BUCKET: u64 = 1 << 20;

#[derive(Clone, Copy)]
struct Extent {
    start: u64,
    size: u64,
    used: u64,
}

pub struct BucketStore {
    data: File,
    index: File,
    extents: HashMap<u64, Vec<Extent>>,
    counts: HashMap<u64, u64>,
    end: u64,
}

pub fn open_bucket(dir: &Path) -> io::Result<BucketStore> {
    std::fs::create_dir_all(dir)?;
    let data_path = dir.join("data.hs");
    let end = data_path.metadata().map(|m| m.len()).unwrap_or(0);

    let data = OpenOptions::new().create(true).read(true).write(true).open(data_path)?;

    let index = OpenOptions::new().create(true).read(true).write(true).open(dir.join("index.hs"))?;

    Ok(BucketStore { data, index, extents: HashMap::new(), counts: HashMap::new(), end })
}


fn allocate(store: &mut BucketStore, bucket: u64, total: u64) -> u64 {
    let extents = store.extents.entry(bucket).or_default();

    if let Some(last) = extents.last_mut() {
        if last.used + total <= last.size {
            let offset = last.start + last.used;
            last.used += total;
            return offset;
        }
    }

    let size = total.next_multiple_of(4096).max(EXTENT_BYTES);
    let offset = store.end;

    extents.push(Extent { start: offset, size, used: total });
    store.end += size;
    offset
}


pub fn bucket_record(store: &mut BucketStore, bucket: u64, payload: &[u8]) -> io::Result<u64> {
    let local = *store.counts.get(&bucket).unwrap_or(&0);
    let id = bucket * IDS_PER_BUCKET + local;
    let total = payload.len() as u64 + 24;
    let offset = allocate(store, bucket, total);

    store.data.seek(SeekFrom::Start(offset))?;
    store.data.write_all(&(payload.len() as u64).to_le_bytes())?;
    store.data.write_all(&bucket.to_le_bytes())?;
    store.data.write_all(&id.to_le_bytes())?;
    store.data.write_all(payload)?;

    store.index.seek(SeekFrom::Start(id * 16))?;
    store.index.write_all(&offset.to_le_bytes())?;
    store.index.write_all(&total.to_le_bytes())?;
    store.counts.insert(bucket, local + 1);

    Ok(id)
}


pub fn read_bucket(store: &mut BucketStore, id: u64, out: &mut Vec<u8>) -> io::Result<()> {
    store.index.seek(SeekFrom::Start(id * 16))?;
    let mut slot = [0u8; 16];
    store.index.read_exact(&mut slot)?;
    let offset = u64::from_le_bytes(slot[..8].try_into().unwrap());

    store.data.seek(SeekFrom::Start(offset))?;
    let mut header = [0u8; 24];
    store.data.read_exact(&mut header)?;
    let len = u64::from_le_bytes(header[..8].try_into().unwrap()) as usize;
    let stored_id = u64::from_le_bytes(header[16..].try_into().unwrap());

    if stored_id != id {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "record id mismatch"));
    }

    out.resize(len, 0);
    store.data.read_exact(out)
}


pub fn flush_bucket(store: &mut BucketStore) -> io::Result<()> {
    store.data.set_len(store.end)?;
    store.data.sync_data()?;

    store.index.sync_data()
}
