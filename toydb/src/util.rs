//! Small helpers that don't belong to any one layer.

/// Little-endian integer accessors for byte slices.
///
/// All on-disk integers in toydb are little-endian, *except* the
/// order-preserving key encoding in `row.rs` which is deliberately
/// big-endian so that `memcmp` order equals numeric order.
pub mod bytes {
    #[inline]
    pub fn get_u16(b: &[u8], off: usize) -> u16 {
        u16::from_le_bytes([b[off], b[off + 1]])
    }
    #[inline]
    pub fn put_u16(b: &mut [u8], off: usize, v: u16) {
        b[off..off + 2].copy_from_slice(&v.to_le_bytes());
    }
    #[inline]
    pub fn get_u32(b: &[u8], off: usize) -> u32 {
        u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
    }
    #[inline]
    pub fn put_u32(b: &mut [u8], off: usize, v: u32) {
        b[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    #[inline]
    pub fn get_u64(b: &[u8], off: usize) -> u64 {
        u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
    }
    #[inline]
    pub fn put_u64(b: &mut [u8], off: usize, v: u64) {
        b[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }
}

/// CRC-32 (IEEE 802.3, the same polynomial as zlib/PNG), table driven.
///
/// Used to detect torn or partially written WAL records. We build the
/// 256-entry lookup table at compile time with a `const fn`.
pub mod crc32 {
    const TABLE: [u32; 256] = build_table();

    const fn build_table() -> [u32; 256] {
        let mut table = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                k += 1;
            }
            table[i] = c;
            i += 1;
        }
        table
    }

    /// Incremental hasher, so a record can be checksummed piece by piece.
    #[derive(Clone, Copy)]
    pub struct Hasher(u32);

    impl Default for Hasher {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Hasher {
        pub fn new() -> Self {
            Hasher(0xFFFF_FFFF)
        }
        pub fn update(&mut self, data: &[u8]) {
            let mut c = self.0;
            for &b in data {
                c = TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
            }
            self.0 = c;
        }
        pub fn finish(self) -> u32 {
            self.0 ^ 0xFFFF_FFFF
        }
    }

    /// One-shot checksum of a byte slice.
    pub fn checksum(data: &[u8]) -> u32 {
        let mut h = Hasher::new();
        h.update(data);
        h.finish()
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn known_vector() {
            // The classic check value for CRC-32/IEEE.
            assert_eq!(super::checksum(b"123456789"), 0xCBF4_3926);
        }
    }
}

/// A tiny deterministic PRNG (SplitMix64). Used by the crash simulator and
/// by tests; not cryptographically secure, which is fine for those uses.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform-ish integer in `0..n` (n > 0).
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
    /// True with probability `num/den`.
    pub fn chance(&mut self, num: u64, den: u64) -> bool {
        self.below(den) < num
    }
}
