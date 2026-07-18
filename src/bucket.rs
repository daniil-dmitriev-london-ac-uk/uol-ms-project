use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

const BUCKET_BYTES: u64 = 64 << 20;
const IDS_PER_BUCKET: u64 = 1 << 20;

pub struct BucketStore {
    data: File,
    index: File,
    used: HashMap<u64, u64>,
    counts: HashMap<u64, u64>,
}

pub fn open_bucket(dir: &Path) -> io::Result<BucketStore> {
    std::fs::create_dir_all(dir)?;
    let data = OpenOptions::new().create(true).read(true).write(true).open(dir.join("data.hs"))?;
    let index = OpenOptions::new().create(true).read(true).write(true).open(dir.join("index.hs"))?;

    Ok(BucketStore { data, index, used: HashMap::new(), counts: HashMap::new() })
}


pub fn bucket_record(store: &mut BucketStore, bucket: u64, payload: &[u8]) -> io::Result<u64> {
    let local = *store.counts.get(&bucket).unwrap_or(&0);
    let id = bucket * IDS_PER_BUCKET + local;
    let used = *store.used.get(&bucket).unwrap_or(&0);
    let offset = bucket * BUCKET_BYTES + used;
    let total = payload.len() as u64 + 24;

    if used + total > BUCKET_BYTES {
        return Err(io::Error::new(io::ErrorKind::OutOfMemory, "bucket lane is full"));
    }

    store.data.seek(SeekFrom::Start(offset))?;
    store.data.write_all(&(payload.len() as u64).to_le_bytes())?;
    store.data.write_all(&bucket.to_le_bytes())?;
    store.data.write_all(&id.to_le_bytes())?;
    store.data.write_all(payload)?;

    store.index.seek(SeekFrom::Start(id * 16))?;
    store.index.write_all(&offset.to_le_bytes())?;
    store.index.write_all(&total.to_le_bytes())?;
    store.used.insert(bucket, used + total);
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
    store.data.sync_data()?;
    store.index.sync_data()
}
