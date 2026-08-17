const CRC32C_POLYNOMIAL: u32 = 0x82F6_3B78;

pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = !0u32;

    for &byte in data {
        crc ^= byte as u32;

        for _ in 0..8 {
            crc = if crc & 1 == 0 {
                crc >> 1
            } else {
                (crc >> 1) ^ CRC32C_POLYNOMIAL
            };
        }
    }

    !crc
}
