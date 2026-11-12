// Copyright (c) Michael Grier.

//! The seeded generator the canopy tests use: SplitMix64.

/// The SplitMix64 increment, the golden-ratio constant.
const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
/// The two multipliers of the SplitMix64 finalizer.
const MIX1: u64 = 0xBF58_476D_1CE4_E5B9;
const MIX2: u64 = 0x94D0_49BB_1331_11EB;
/// The bits of a `u64` an `f64` mantissa can hold.
const MANTISSA_BITS: u32 = 53;

/// A SplitMix64 stream: every sequence is fixed by its seed.
#[derive(Clone, Debug)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> SplitMix64 {
        SplitMix64 { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(GAMMA);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(MIX1);
        z = (z ^ (z >> 27)).wrapping_mul(MIX2);
        z ^ (z >> 31)
    }

    /// Uniform on `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        let bits = self.next_u64() >> (u64::BITS - MANTISSA_BITS);
        bits as f64 / (1u64 << MANTISSA_BITS) as f64
    }

    /// Uniform on `(0, 1]`, safe to take the logarithm of.
    pub fn open_unit(&mut self) -> f64 {
        1.0 - self.unit()
    }

    /// Uniform on `0..n`; `n` must be positive.
    pub fn below(&mut self, n: usize) -> usize {
        ((self.unit() * n as f64) as usize).min(n - 1)
    }

    /// True with probability `p`.
    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    /// An independent stream for replication `index` of a run seeded `seed`.
    pub fn stream(seed: u64, index: u64) -> SplitMix64 {
        let mut mixer = SplitMix64::new(seed ^ index.wrapping_mul(GAMMA));
        SplitMix64::new(mixer.next_u64())
    }
}
