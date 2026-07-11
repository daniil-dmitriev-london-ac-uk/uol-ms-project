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


fn write_record(file: &mut File, payload: &[u8]) -> io::Result<u64> {
    let offset = file.stream_position()?;
    file.write_all(&(payload.len() as u64).to_le_bytes())?;
    file.write_all(payload)?;

    Ok(offset)
}


fn read_record(file: &mut File, offset: u64, out: &mut Vec<u8>) -> io::Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    let mut len = [0u8; 8];
    file.read_exact(&mut len)?;
    out.resize(u64::from_le_bytes(len) as usize, 0);
    file.read_exact(out)
}



fn main() -> io::Result<()> {
    const RECORDS: u64 = 4096;
    const RECORD_BYTES: usize = 4096;
    const SYNC_EVERY: u64 = 256;
    let path = std::env::args().nth(1).unwrap_or_else(|| "heap.data".into());

    let mut file = OpenOptions::new().create(true).truncate(true).read(true).write(true).open(path)?;

    let mut state = 42u64;

    let mut payload = Vec::with_capacity(RECORD_BYTES);
    let mut offsets = Vec::with_capacity(RECORDS as usize);

    let started = Instant::now();

    for record in 0..RECORDS {
        random_data(&mut state, &mut payload, RECORD_BYTES);
        offsets.push(write_record(&mut file, &payload)?);
        if (record + 1) % SYNC_EVERY == 0 {
            file.sync_data()?;
        }
    }

    file.sync_data()?;

    let mut check = Vec::new();
    let sample = RECORDS as usize / 2;
    read_record(&mut file, offsets[sample], &mut check)?;
    let bytes = RECORDS * RECORD_BYTES as u64;

    println!("records={RECORDS} bytes={bytes} sample_bytes={} mib_s={:.2}", check.len(), bytes as f64 / started.elapsed().as_secs_f64() / 1_048_576.0);

    Ok(())
}
