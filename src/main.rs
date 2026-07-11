use std::fs::OpenOptions;
use std::io::{self, Write};
use std::time::Instant;

fn randomData(state: &mut u64, bytes: usize) -> Vec<u8> {
    let mut data = vec![0u8; bytes];
    let mut offset = 0;

    while offset < data.len() {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut value = *state;
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^= value >> 31;
        let bytes = value.to_le_bytes();
        let take = (data.len() - offset).min(bytes.len());

        data[offset..offset + take].copy_from_slice(&bytes[..take]);
        offset += take;
    }

    data
}


fn write(file: &mut std::fs::File, data: &[u8]) -> io::Result<()> {
    file.write_all(data)
}



fn main() -> io::Result<()> {
    const ITERATIONS: u64 = 512;
    const BYTES_PER_ITERATION: usize = 64 * 1024;
    const REPORT_EVERY: u64 = 64;
    let mut state = 0x1234_5678_9ABC_DEF0;

    let path = "heap.data";

    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;

    let started = Instant::now();
    let mut generated = 0u64;

    for iteration in 0..ITERATIONS {
        let data = randomData(&mut state, BYTES_PER_ITERATION);
        write(&mut file, &data)?;
        generated += data.len() as u64;
        if ( iteration + 1 ) % REPORT_EVERY == 0 {
            let seconds = started.elapsed().as_secs_f64();

            println!("progress_bytes={generated} throughput_mib_s={:.2}", generated as f64 / seconds / 1_048_576.0);
        }
    }

    let seconds = started.elapsed().as_secs_f64();
    let mib = generated as f64 / (1024.0 * 1024.0);

    println!("generated_bytes={generated}");
    println!("throughput_mib_s={:.2}", mib / seconds);

    Ok(())
}
