use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

const EXTENT_BYTES: u64 = 1 << 20;
const REGION_SLOTS: u64 = 4096;

#[derive(Clone, Copy)]
struct Extent {
    start: u64,
    size: u64,
    used: u64,
}

pub struct BucketStore {
    data: File,
    index: File,
    registry: File,
    extents: HashMap<u64, Vec<Extent>>,
    regions: HashMap<u64, (u64, u64)>,
    next_region: u64,
    end: u64,
}

pub fn open_bucket(dir: &Path) -> io::Result<BucketStore> {
    std::fs::create_dir_all(dir)?;
    let data_path = dir.join("data.hs");
    let end = data_path.metadata().map(|m| m.len()).unwrap_or(0);

    let data = OpenOptions::new().create(true).read(true).write(true).open(data_path)?;

    let index = OpenOptions::new().create(true).read(true).write(true).open(dir.join("index.hs"))?;
    let registry = OpenOptions::new().create(true).append(true).read(true).open(dir.join("registry.hs"))?;

    Ok(BucketStore { data, index, registry, extents: HashMap::new(), regions: HashMap::new(), next_region: 0, end })
}


fn allocate(store: &mut BucketStore, bucket: u64, total: u64) -> io::Result<u64> {
    let extents = store.extents.entry(bucket).or_default();

    if let Some(last) = extents.last_mut() {
        if last.used + total <= last.size {
            let offset = last.start + last.used;
            last.used += total;
            return Ok(offset);
        }
    }

    let size = total.next_multiple_of(4096).max(EXTENT_BYTES);
    let offset = store.end;

    extents.push(Extent { start: offset, size, used: total });
    store.end += size;
    store.registry.write_all(&[b'D'])?;
    store.registry.write_all(&bucket.to_le_bytes())?;
    store.registry.write_all(&offset.to_le_bytes())?;
    store.registry.write_all(&size.to_le_bytes())?;

    Ok(offset)
}


pub fn bucket_record(store: &mut BucketStore, bucket: u64, payload: &[u8]) -> io::Result<u64> {
    if !store.regions.contains_key(&bucket) {
        let start = store.next_region;
        store.next_region += REGION_SLOTS;
        let mut row = Vec::with_capacity(25);
        row.push(b'S');
        row.extend_from_slice(&bucket.to_le_bytes());
        row.extend_from_slice(&start.to_le_bytes());
        row.extend_from_slice(&REGION_SLOTS.to_le_bytes());
        store.registry.write_all(&row)?;
        store.regions.insert(bucket, (start, 0));
    }

    let region = store
        .regions
        .get_mut(&bucket)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bucket region is missing"))?;

    if region.1 == REGION_SLOTS {
        return Err(io::Error::new(io::ErrorKind::OutOfMemory, "bucket index region is full"));
    }

    let id = region.0 + region.1;
    region.1 += 1;
    let total = payload.len() as u64 + 24;
    let offset = allocate(store, bucket, total)?;

    store.data.seek(SeekFrom::Start(offset))?;
    store.data.write_all(&(payload.len() as u64).to_le_bytes())?;
    store.data.write_all(&bucket.to_le_bytes())?;
    store.data.write_all(&id.to_le_bytes())?;
    store.data.write_all(payload)?;

    store.index.seek(SeekFrom::Start(id * 16))?;
    store.index.write_all(&offset.to_le_bytes())?;
    store.index.write_all(&total.to_le_bytes())?;

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
    store.registry.sync_data()?;
    store.data.sync_data()?;
    store.index.sync_data()
}
