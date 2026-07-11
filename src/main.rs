use std::fs::OpenOptions;
use std::io::{self, Write};
use std::time::Instant;

fn randomData(state: &mut u64, records: usize, record_bytes: usize) -> Vec<u8> {
    let mut data = vec![0u8; records * record_bytes];
    let mut offset = 0;

    while offset < data.len() {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut value = *state;
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^= value >> 31;
        let bytes = value.to_le_bytes();
        let take = (data.len() - offset).min(8);

        data[offset..offset + take].copy_from_slice(&bytes[..take]);
        offset += take;
    }

    data
}


fn write(file: &mut std::fs::File, batch: &[u8]) -> io::Result<()> {
    file.write_all(batch)
}



fn main() -> io::Result<()> {
    const BATCHES: u64 = 256;
    const RECORDS_PER_BATCH: usize = 16;
    const RECORD_BYTES: usize = 4096;
    const REPORT_EVERY: u64 = 32;
    let mut state = 0x1234_5678_9ABC_DEF0;

    let path = "heap.data";

    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;

    let started = Instant::now();
    let mut generated = 0u64;

    let mut records = 0u64;

    for batch_no in 0..BATCHES {
        let batch = randomData(&mut state, RECORDS_PER_BATCH, RECORD_BYTES);
        write(&mut file, &batch)?;
        generated += batch.len() as u64;
        records += RECORDS_PER_BATCH as u64;
        if ( batch_no + 1 ) % REPORT_EVERY == 0 {
            let seconds = started.elapsed().as_secs_f64();

            println!("records={records} throughput_mib_s={:.2}", generated as f64 / seconds / 1_048_576.0);
        }
    }

    file.sync_data()?;

    println!("generated_bytes={generated} records={records}");

    Ok(())
}
