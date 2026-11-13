// Copyright (c) Michael Grier.

//! Seeded nonce sources for tests, and the encoding the random source shares with them.
//!
//! Unit tests draw nonces from SplitMix64 with a fixed seed, so every run makes the same
//! nonces. The generator is implemented here rather than taken from a dependency so that an
//! upgrade can never change what a seed produces: the algorithm and its constants are part of
//! what a recorded seed means, and changing either invalidates every seed recorded against it.
//! Each actor in a test gets its own stream, derived from the seed and the actor's number, so
//! adding an actor never shifts the nonces another one draws.

use super::*;
use crate::random::{RANDOM_BYTES, encode, encode_nonce};

/// The seed unit tests use unless a test has a reason to choose another.
pub(super) const TEST_SEED: u64 = 0x0CA4_0975_EED5_0001;

/// SplitMix64 (Steele, Lea and Flood, "Fast Splittable Pseudorandom Number Generators", 2014).
#[derive(Debug, Clone)]
pub(super) struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// The generator's increment, the 64-bit golden ratio.
    const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

    pub(super) const fn new(seed: u64) -> Self {
        SplitMix64 { state: seed }
    }

    /// Actor `actor`'s stream under `seed`: the seed offset by the actor's number in units of a
    /// second odd constant, then mixed once, so neighbouring actors start far apart.
    pub(super) fn stream(seed: u64, actor: u64) -> Self {
        let offset = actor.wrapping_mul(0xD1B5_4A32_D192_ED03);
        SplitMix64::new(SplitMix64::new(seed.wrapping_add(offset)).next_u64())
    }

    pub(super) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(Self::GAMMA);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 128 bits, encoded the way the random source encodes them.
    pub(super) fn nonce(&mut self) -> Nonce {
        let mut bits = [0u8; RANDOM_BYTES];
        bits[..8].copy_from_slice(&self.next_u64().to_be_bytes());
        bits[8..].copy_from_slice(&self.next_u64().to_be_bytes());
        encode_nonce(bits)
    }
}

/// A nonce source drawing actor `actor`'s stream under `seed`.
pub(super) fn seeded_source(seed: u64, actor: u64) -> impl FnMut() -> Nonce + Send + 'static {
    let mut stream = SplitMix64::stream(seed, actor);
    move || stream.nonce()
}

/// The first `count` nonces of actor `actor`'s stream under `seed`, for predicting what a
/// store opened with [`seeded_source`] will draw.
pub(super) fn seeded_nonces(seed: u64, actor: u64, count: usize) -> Vec<Nonce> {
    let mut stream = SplitMix64::stream(seed, actor);
    (0..count).map(|_| stream.nonce()).collect()
}

/// A nonce source yielding exactly `nonces`, in order, and panicking if asked for more.
pub(super) fn scripted_source(nonces: Vec<Nonce>) -> impl FnMut() -> Nonce + Send + 'static {
    let mut nonces = nonces.into_iter();
    move || nonces.next().expect("the test scripted enough nonces")
}

/// Decodes RFC 4648 base32 without padding, for checking the encoder against itself.
fn decode(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut bytes = Vec::new();
    let mut buffer: u32 = 0;
    let mut held = 0u32;
    for character in text.bytes() {
        let digit = ALPHABET
            .iter()
            .position(|&a| a == character)
            .expect("a base32 digit") as u32;
        buffer = (buffer << 5) | digit;
        held += 5;
        if held >= 8 {
            held -= 8;
            bytes.push((buffer >> held) as u8);
        }
    }
    bytes
}

#[test]
fn splitmix64_matches_its_published_output_for_seed_zero() {
    let mut generator = SplitMix64::new(0);
    assert_eq!(generator.next_u64(), 0xE220_A839_7B1D_CDAF);
    assert_eq!(generator.next_u64(), 0x6E78_9E6A_A1B9_65F4);
    assert_eq!(generator.next_u64(), 0x06C4_5D18_8009_454F);
}

#[test]
fn a_seed_reproduces_the_same_nonces() {
    assert_eq!(
        seeded_nonces(TEST_SEED, 0, 32),
        seeded_nonces(TEST_SEED, 0, 32)
    );
}

#[test]
fn different_seeds_give_different_nonces() {
    assert_ne!(seeded_nonces(1, 0, 4), seeded_nonces(2, 0, 4));
}

#[test]
fn each_actor_draws_its_own_stream() {
    let streams: Vec<Vec<Nonce>> = (0..8)
        .map(|actor| seeded_nonces(TEST_SEED, actor, 64))
        .collect();
    let mut all: Vec<&Nonce> = streams.iter().flatten().collect();
    let drawn = all.len();
    all.sort();
    all.dedup();
    assert_eq!(
        all.len(),
        drawn,
        "no two actors, and no two draws, share a nonce"
    );
}

#[test]
fn an_actor_stream_does_not_depend_on_how_many_other_actors_draw() {
    let alone = seeded_nonces(TEST_SEED, 3, 8);
    let mut crowd: Vec<SplitMix64> = (0..6)
        .map(|actor| SplitMix64::stream(TEST_SEED, actor))
        .collect();
    for generator in crowd.iter_mut() {
        for _ in 0..5 {
            generator.nonce();
        }
    }
    let mut fresh = SplitMix64::stream(TEST_SEED, 3);
    let after_crowd: Vec<Nonce> = (0..8).map(|_| fresh.nonce()).collect();
    assert_eq!(alone, after_crowd);
}

#[test]
fn all_zero_bits_encode_as_all_a() {
    assert_eq!(encode([0; RANDOM_BYTES]), "A".repeat(26));
}

#[test]
fn all_one_bits_encode_with_the_last_digit_padded_with_zero_bits() {
    // 128 bits are 25 full digits and 3 bits over; those 3 bits shifted up make 0b11100.
    assert_eq!(encode([0xFF; RANDOM_BYTES]), format!("{}4", "7".repeat(25)));
}

#[test]
fn a_single_high_bit_lands_in_the_first_digit() {
    let mut bits = [0u8; RANDOM_BYTES];
    bits[0] = 0x80;
    assert_eq!(encode(bits), format!("Q{}", "A".repeat(25)));
}

#[test]
fn a_single_low_bit_lands_in_the_last_digit() {
    let mut bits = [0u8; RANDOM_BYTES];
    bits[RANDOM_BYTES - 1] = 0x01;
    assert_eq!(encode(bits), format!("{}E", "A".repeat(25)));
}

#[test]
fn every_encoding_is_26_characters_and_decodes_back() {
    let mut generator = SplitMix64::new(TEST_SEED);
    for _ in 0..500 {
        let mut bits = [0u8; RANDOM_BYTES];
        bits[..8].copy_from_slice(&generator.next_u64().to_be_bytes());
        bits[8..].copy_from_slice(&generator.next_u64().to_be_bytes());
        let text = encode(bits);
        assert_eq!(text.len(), 26);
        assert_eq!(decode(&text), bits.to_vec());
    }
}

#[test]
fn an_encoding_is_already_a_canonical_nonce() {
    let mut generator = SplitMix64::new(7);
    for _ in 0..100 {
        let nonce = generator.nonce();
        assert_eq!(Nonce::new(nonce.as_str()).unwrap(), nonce);
        assert_eq!(nonce.path_component(), nonce.as_str());
    }
}

#[test]
fn a_scripted_source_yields_its_script_in_order() {
    let mut source = scripted_source(vec![nonce("one"), nonce("two")]);
    assert_eq!(source(), nonce("one"));
    assert_eq!(source(), nonce("two"));
}

#[cfg(feature = "random-nonce")]
#[test]
fn the_random_source_draws_distinct_canonical_26_character_nonces() {
    let mut source = crate::random_nonce_source();
    let mut drawn: Vec<Nonce> = (0..1000).map(|_| source()).collect();
    for nonce in &drawn {
        assert_eq!(nonce.as_str().len(), 26);
        assert_eq!(&Nonce::new(nonce.as_str()).unwrap(), nonce);
    }
    drawn.sort();
    drawn.dedup();
    assert_eq!(drawn.len(), 1000);
}
