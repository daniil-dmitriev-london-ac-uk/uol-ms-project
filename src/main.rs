use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Instant;

struct AppendStore {
    data: File,
    index: File,
    cursor: u64,
    next_id: u64,
}

fn open_append(dir: &Path) -> io::Result<AppendStore> {
    std::fs::create_dir_all(dir)?;
    let data_path = dir.join("data.hs");
    let index_path = dir.join("index.hs");
    let cursor = data_path.metadata().map(|m| m.len()).unwrap_or(0);
    let index_len = index_path.metadata().map(|m| m.len()).unwrap_or(0);

    let data = OpenOptions::new().create(true).read(true).write(true).open(data_path)?;
    let index = OpenOptions::new().create(true).read(true).write(true).open(index_path)?;

    Ok(AppendStore { data, index, cursor, next_id: index_len / 16 })
}


fn append_record(store: &mut AppendStore, payload: &[u8]) -> io::Result<u64> {
    let id = store.next_id;
    let total = payload.len() as u64 + 16;

    store.data.seek(SeekFrom::Start(store.cursor))?;
    store.data.write_all(&(payload.len() as u64).to_le_bytes())?;
    store.data.write_all(&id.to_le_bytes())?;
    store.data.write_all(payload)?;

    store.index.seek(SeekFrom::Start(id * 16))?;
    store.index.write_all(&store.cursor.to_le_bytes())?;
    store.index.write_all(&total.to_le_bytes())?;
    store.cursor += total;
    store.next_id += 1;

    Ok(id)
}


fn read_append(store: &mut AppendStore, id: u64, out: &mut Vec<u8>) -> io::Result<()> {
    store.index.seek(SeekFrom::Start(id * 16))?;
    let mut slot = [0u8; 16];
    store.index.read_exact(&mut slot)?;
    let offset = u64::from_le_bytes(slot[..8].try_into().unwrap());
    let total = u64::from_le_bytes(slot[8..].try_into().unwrap());

    store.data.seek(SeekFrom::Start(offset))?;
    let mut header = [0u8; 16];
    store.data.read_exact(&mut header)?;
    let len = u64::from_le_bytes(header[..8].try_into().unwrap());
    let stored_id = u64::from_le_bytes(header[8..].try_into().unwrap());

    if stored_id != id || len + 16 != total {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid record header"));
    }

    out.resize(len as usize, 0);
    store.data.read_exact(out)
}


fn flush_append(store: &mut AppendStore) -> io::Result<()> {
    store.data.sync_data()?;
    store.index.sync_data()
}



fn main() -> io::Result<()> {
    let dir = Path::new("append-store");
    let mut store = open_append(dir)?;

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
