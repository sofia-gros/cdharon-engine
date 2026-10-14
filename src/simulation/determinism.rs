//! バージョン固定の決定論的乱数と、診断用fingerprint primitives。

/// Philox4x32-10. Random123の標準定数とround順を使用します。
pub fn philox4x32_10(mut counter: [u32; 4], mut key: [u32; 2]) -> [u32; 4] {
    const M0: u32 = 0xD251_1F53;
    const M1: u32 = 0xCD9E_8D57;
    const W0: u32 = 0x9E37_79B9;
    const W1: u32 = 0xBB67_AE85;

    for round in 0..10 {
        let product0 = u64::from(M0) * u64::from(counter[0]);
        let product1 = u64::from(M1) * u64::from(counter[2]);
        counter = [
            (product1 >> 32) as u32 ^ counter[1] ^ key[0],
            product1 as u32,
            (product0 >> 32) as u32 ^ counter[3] ^ key[1],
            product0 as u32,
        ];
        if round != 9 {
            key[0] = key[0].wrapping_add(W0);
            key[1] = key[1].wrapping_add(W1);
        }
    }
    counter
}

/// seed/tick/座標/domain/sampleを固定tupleとして1つのu32へ写像します。
pub fn sample_u32(seed: u64, tick: u64, x: i32, y: i32, domain: u32, sample_index: u32) -> u32 {
    let base = philox4x32_10(
        [tick as u32, (tick >> 32) as u32, x as u32, y as u32],
        [seed as u32, (seed >> 32) as u32],
    );
    philox4x32_10(base, [domain, sample_index])[0]
}

/// 非暗号用途のFNV-1a 64-bit診断fingerprint。
#[derive(Clone, Copy, Debug)]
pub struct Fnv64(u64);

impl Default for Fnv64 {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv64 {
    pub fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }

    pub fn write_u16(&mut self, value: u16) {
        self.write(&value.to_le_bytes());
    }

    pub fn write_u32(&mut self, value: u32) {
        self.write(&value.to_le_bytes());
    }

    pub fn write_u64(&mut self, value: u64) {
        self.write(&value.to_le_bytes());
    }

    pub const fn finish(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn philox_matches_random123_zero_vector() {
        assert_eq!(
            philox4x32_10([0; 4], [0; 2]),
            [0x6627_E8D5, 0xE169_C58D, 0xBC57_AC4C, 0x9B00_DBD8]
        );
    }

    #[test]
    fn sample_tuple_includes_domain_and_index() {
        let baseline = sample_u32(7, 11, -3, 5, 1, 0);
        assert_ne!(baseline, sample_u32(7, 11, -3, 5, 2, 0));
        assert_ne!(baseline, sample_u32(7, 11, -3, 5, 1, 1));
    }
}
