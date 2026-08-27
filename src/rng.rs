pub trait RandomSource {
    fn next_u64(&mut self) -> u64;

    fn below(&mut self, bound: u64) -> u64 {
        ((self.next_u64() as u128 * bound as u128) >> 64) as u64
    }

    fn fill(&mut self, buf: &mut [u8]) {
        let mut chunks = buf.chunks_exact_mut(8);

        for chunk in &mut chunks {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes());
        }

        let remainder = chunks.into_remainder();

        if !remainder.is_empty() {
            let last = self.next_u64().to_le_bytes();

            remainder.copy_from_slice(&last[..remainder.len()]);
        }
    }
}

pub struct SplitMix64(pub u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        RandomSource::next_u64(self)
    }

    pub fn below(&mut self, bound: u64) -> u64 {
        RandomSource::below(self, bound)
    }

    pub fn fill(&mut self, buf: &mut [u8]) {
        RandomSource::fill(self, buf)
    }
}

impl RandomSource for SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);

        let mut value = self.0;

        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);

        value ^ (value >> 31)
    }
}

pub fn payload_for(seed: u64, key: u64, len: usize, out: &mut Vec<u8>) {
    out.clear();
    out.resize(len, 0);
    SplitMix64::new(seed ^ key.wrapping_mul(0xA076_1D64_78BD_642F)).fill(out);
}
