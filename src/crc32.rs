//! this module computes record integrity checks.

const CRC32C_POLYNOMIAL: u32 = 0x82F6_3B78;

const TABLES: [[u32; 256]; 8] = build_tables();

const fn build_tables() -> [[u32; 256]; 8] {
    let mut tables = [[0u32; 256]; 8];
    let mut byte_value = 0;

    while byte_value < 256 {
        let mut crc = byte_value as u32;
        let mut bit_index = 0;

        while bit_index < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ CRC32C_POLYNOMIAL
            } else {
                crc >> 1
            };
            bit_index += 1;
        }

        tables[0][byte_value] = crc;
        byte_value += 1;
    }

    let mut table_index = 1;

    while table_index < 8 {
        let mut byte_value = 0;

        while byte_value < 256 {
            let prev = tables[table_index - 1][byte_value];

            tables[table_index][byte_value] = (prev >> 8) ^ tables[0][(prev & 0xFF) as usize];
            byte_value += 1;
        }

        table_index += 1;
    }

    tables
}

pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    let mut chunks = data.chunks_exact(8);

    for chunk in &mut chunks {
        let lower = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) ^ crc;
        let upper = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);

        crc = TABLES[7][(lower & 0xFF) as usize]
            ^ TABLES[6][((lower >> 8) & 0xFF) as usize]
            ^ TABLES[5][((lower >> 16) & 0xFF) as usize]
            ^ TABLES[4][(lower >> 24) as usize]
            ^ TABLES[3][(upper & 0xFF) as usize]
            ^ TABLES[2][((upper >> 8) & 0xFF) as usize]
            ^ TABLES[1][((upper >> 16) & 0xFF) as usize]
            ^ TABLES[0][(upper >> 24) as usize];
    }

    for &byte in chunks.remainder() {
        crc = (crc >> 8) ^ TABLES[0][((crc ^ byte as u32) & 0xFF) as usize];
    }

    !crc
}
