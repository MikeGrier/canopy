// Copyright (c) Michael Grier.

//! A nonce source drawing from the operating system's entropy source.
//!
//! A nonce needs only to be statistically unique: 128 random bits make two operations sharing
//! one far less likely than an undetected hardware error, and a collision that does happen is
//! detected and fails the operation rather than corrupting anything (see
//! [`StoreOptions::with_nonce_source`](crate::StoreOptions::with_nonce_source)).

#[cfg(any(feature = "random-nonce", test))]
use crate::name::Nonce;

/// How many random bytes a nonce carries: 128 bits.
pub(crate) const RANDOM_BYTES: usize = 16;

/// The RFC 4648 base32 alphabet: uppercase letters and the digits 2-7, all inside the nonce
/// character set, so an encoded nonce is already canonical.
const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// The characters 128 bits take in base32: 26, the last carrying the final 3 bits.
pub(crate) const RANDOM_NONCE_LEN: usize = (RANDOM_BYTES * 8).div_ceil(5);

/// Renders `bits` in RFC 4648 base32, most significant bit first, without padding.
pub(crate) fn encode(bits: [u8; RANDOM_BYTES]) -> String {
    let mut encoded = String::with_capacity(RANDOM_NONCE_LEN);
    // Never holds more than 12 meaningful bits: at most 4 left over plus the byte just added.
    let mut buffer: u16 = 0;
    let mut held = 0u32;
    for byte in bits {
        buffer = (buffer << 8) | u16::from(byte);
        held += 8;
        while held >= 5 {
            held -= 5;
            encoded.push(char::from(BASE32[usize::from((buffer >> held) & 0x1f)]));
        }
    }
    if held > 0 {
        encoded.push(char::from(
            BASE32[usize::from((buffer << (5 - held)) & 0x1f)],
        ));
    }
    encoded
}

/// Draws a nonce of 128 random bits.
///
/// # Panics
///
/// When the operating system cannot supply entropy. A nonce that is not random would undermine
/// the uniqueness every operation relies on, so there is no fallback.
#[cfg(feature = "random-nonce")]
pub fn random_nonce() -> Nonce {
    let mut bits = [0u8; RANDOM_BYTES];
    getrandom::fill(&mut bits).expect("the operating system supplies entropy");
    Nonce::new(&encode(bits)).expect("base32 is within the nonce character set")
}

/// A nonce source for [`StoreOptions::with_nonce_source`](crate::StoreOptions::with_nonce_source) that
/// draws every nonce from [`random_nonce`].
#[cfg(feature = "random-nonce")]
pub fn random_nonce_source() -> impl FnMut() -> Nonce + Send + 'static {
    random_nonce
}

/// The nonce `bits` encode to, as [`random_nonce`] would mint it.
#[cfg(test)]
pub(crate) fn encode_nonce(bits: [u8; RANDOM_BYTES]) -> Nonce {
    Nonce::new(&encode(bits)).expect("base32 is within the nonce character set")
}
