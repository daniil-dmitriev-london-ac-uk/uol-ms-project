use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::time::Instant;

fn next_random(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut value = *state;
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}


fn random_data(state: &mut u64, data: &mut Vec<u8>, bytes: usize) {
    data.clear();
    data.resize(bytes, 0);

    for chunk in data.chunks_mut(8) {
        let value = next_random(state).to_le_bytes();
        chunk.copy_from_slice(&value[..chunk.len()]);
    }
}


fn write_record(data: &mut File, index: &mut File, id: u64, payload: &[u8]) -> io::Result<()> {
    let offset = data.stream_position()?;
    data.write_all(&(payload.len() as u64).to_le_bytes())?;
    data.write_all(&id.to_le_bytes())?;
    data.write_all(payload)?;

    index.write_all(&offset.to_le_bytes())?;
    index.write_all(&(payload.len() as u64 + 16).to_le_bytes())
}


fn read_record(data: &mut File, index: &mut File, id: u64, out: &mut Vec<u8>) -> io::Result<()> {
    index.seek(SeekFrom::Start(id * 16))?;
    let mut slot = [0u8; 16];
    index.read_exact(&mut slot)?;
    let offset = u64::from_le_bytes(slot[..8].try_into().unwrap());

    data.seek(SeekFrom::Start(offset))?;
    let mut header = [0u8; 16];
    data.read_exact(&mut header)?;
    let len = u64::from_le_bytes(header[..8].try_into().unwrap()) as usize;
    let stored_id = u64::from_le_bytes(header[8..].try_into().unwrap());

    if stored_id != id {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "record id mismatch"));
    }

    out.resize(len, 0);
    data.read_exact(out)
}



fn run(record_bytes: usize, records: u64, sync_every: u64) -> io::Result<()> {
    let mut data = OpenOptions::new().create(true).truncate(true).read(true).write(true).open("heap.data")?;
    let mut index = OpenOptions::new().create(true).truncate(true).read(true).write(true).open("heap.index")?;

    let mut state = 42u64;

    let mut payload = Vec::with_capacity(record_bytes);

    let started = Instant::now();

    for id in 0..records {
        random_data(&mut state, &mut payload, record_bytes);
        write_record(&mut data, &mut index, id, &payload)?;
        if (id + 1) % sync_every == 0 {
            data.sync_data()?;
            index.sync_data()?;
        }
    }

    let mut out = Vec::new();

    for id in [0, records / 2, records - 1] {
        read_record(&mut data, &mut index, id, &mut out)?;
    }

    let bytes = records * record_bytes as u64;

    println!("records={records} bytes={bytes} mib_s={:.2}", bytes as f64 / started.elapsed().as_secs_f64() / 1_048_576.0);

    Ok(())
}



fn main() -> io::Result<()> {
    let sizes = [1024usize, 4096, 64 * 1024];

    for size in sizes {
        run(size, 4096, 256)?;
    }

    Ok(())
}
