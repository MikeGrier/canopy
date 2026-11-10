// Copyright (c) Michael Grier.

//! Opaque document body value type for the canopy namespace (D27/D37/D54/D58).
//!
//! A [`DocumentBody`] is the byte payload canopy stores and replicates at a node but
//! **never parses** (D27/D54): there is no envelope, schema, or version that canopy
//! interprets -- any such structure lives *inside* the opaque body and is the
//! caller's concern.
//!
//! canopy can nonetheless take a [`ContentDigest`] over the raw body bytes (D37),
//! which is what a caller uses for an optional conditional (compare-and-set) write
//! (D54). The digest is **SHA-256 over exactly the bytes canopy was handed** -- canopy
//! runs no canonicalization of its own; making those bytes deterministic across
//! copies is the caller's responsibility (D37), so canopy need only "write the same
//! bytes and compare the same digest."

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// An opaque document body: bytes canopy stores and replicates but never parses (D27/D54).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DocumentBody {
    bytes: Vec<u8>,
}

impl DocumentBody {
    /// Wraps caller-supplied bytes as an opaque body. Any byte sequence is valid,
    /// including the empty body; canopy attaches no meaning to the contents.
    pub fn new(bytes: impl Into<Vec<u8>>) -> DocumentBody {
        DocumentBody {
            bytes: bytes.into(),
        }
    }

    /// The raw body bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The body length in bytes.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether the body is empty.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// The [`ContentDigest`] over the raw body bytes (D37), for conditional writes.
    pub fn digest(&self) -> ContentDigest {
        ContentDigest::of(&self.bytes)
    }

    /// Consumes the body, returning its bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl From<Vec<u8>> for DocumentBody {
    fn from(bytes: Vec<u8>) -> DocumentBody {
        DocumentBody { bytes }
    }
}

impl From<&[u8]> for DocumentBody {
    fn from(bytes: &[u8]) -> DocumentBody {
        DocumentBody {
            bytes: bytes.to_vec(),
        }
    }
}

/// A content digest over a document body's raw bytes (D37): **SHA-256**.
///
/// The algorithm is canopy's owned specification (SHA-256, RFC 6234), realized with
/// the RustCrypto `sha2` implementation; it is chosen because it matches that
/// specification, not inherited from it. Two bodies with equal digests are taken to
/// have equal bytes, which is what makes the digest usable as a conditional-write
/// (compare-and-set) token (D54).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentDigest([u8; Self::LEN]);

/// RFC 4648 base32: `A`-`Z` then `2`-`7` (`D322`).
///
/// Excluding the digits `0`, `1`, `8` and `9` is what removes the `0`/`O` and `1`/`I`/`l`
/// confusions from the human-correlation path `D321` exists to serve. Changing this table
/// changes every name on disk, so it is a format-breaking constant.
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

impl ContentDigest {
    /// The digest length in bytes (SHA-256 -> 32 bytes).
    pub const LEN: usize = 32;

    /// The digest length when rendered as unpadded base32 (`D322`).
    ///
    /// `ceil(256 / 5)`. The four spare bits of the final character are zero padding and
    /// are asserted rather than ignored -- see [`from_base32`](Self::from_base32).
    pub const BASE32_LEN: usize = 52;

    /// Computes the digest over `bytes`.
    pub fn of(bytes: &[u8]) -> ContentDigest {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let out = hasher.finalize();
        let mut digest = [0u8; Self::LEN];
        digest.copy_from_slice(&out);
        ContentDigest(digest)
    }

    /// Reconstructs a digest from its raw bytes, without recomputing it.
    ///
    /// The digest's identity *is* its bytes (D37/D54), so a digest read back from a
    /// durable record (e.g. the D53 commit log, or a client's own persisted note of
    /// the digest it last observed) is rebuilt here rather than re-hashed a body it no
    /// longer has.
    pub fn from_bytes(bytes: [u8; Self::LEN]) -> ContentDigest {
        ContentDigest(bytes)
    }

    /// The raw digest bytes.
    pub fn as_bytes(&self) -> &[u8; Self::LEN] {
        &self.0
    }

    /// The lowercase-hex presentation of the digest (its identity is the bytes).
    pub fn to_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(Self::LEN * 2);
        for byte in self.0 {
            s.push(HEX[(byte >> 4) as usize] as char);
            s.push(HEX[(byte & 0x0f) as usize] as char);
        }
        s
    }

    /// The RFC 4648 base32 presentation of the digest: uppercase, unpadded, always
    /// [`BASE32_LEN`](Self::BASE32_LEN) characters (`D322`).
    ///
    /// This is the rendering the store's on-disk names use. It is **additive**: `D298`'s
    /// hex rendering is unchanged for callers that carry it.
    ///
    /// base32 rather than the denser base64url because base64url's alphabet distinguishes
    /// `a` from `A` while NTFS and APFS fold case, which makes it non-injective on the
    /// filesystem it would live on. Hex is case-safe too but costs 64 characters here, and
    /// twice that in a name carrying two renderings.
    ///
    /// **The last character is always `A` or `Q`.** 256 does not divide by 5: fifty-one
    /// characters carry 255 bits, so the fifty-second holds one significant bit and four
    /// zero pad bits. See [`from_base32`](Self::from_base32), which rejects anything else.
    pub fn to_base32(&self) -> String {
        let mut out = String::with_capacity(Self::BASE32_LEN);
        let mut acc: u16 = 0;
        let mut bits: u32 = 0;
        for byte in self.0 {
            acc = (acc << 8) | u16::from(byte);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                out.push(BASE32_ALPHABET[((acc >> bits) & 0x1f) as usize] as char);
            }
        }
        if bits > 0 {
            out.push(BASE32_ALPHABET[((acc << (5 - bits)) & 0x1f) as usize] as char);
        }
        out
    }

    /// Decodes an RFC 4648 base32 digest back into a `ContentDigest`, or `None` if `text`
    /// is not exactly [`BASE32_LEN`](Self::BASE32_LEN) characters of that alphabet whose
    /// four trailing pad bits are zero. The inverse of [`to_base32`](Self::to_base32).
    ///
    /// Accepts either case even though [`to_base32`](Self::to_base32) emits uppercase: the
    /// names live on case-folding filesystems, so case cannot be load-bearing on the way
    /// back in without the format lying about what round-trips.
    ///
    /// Rejecting non-zero pad bits is what makes the final character a real check rather
    /// than a curiosity: a truncated, re-encoded, or foreign-produced string fails here
    /// instead of decoding to a digest nobody ever computed.
    pub fn from_base32(text: &str) -> Option<ContentDigest> {
        if text.len() != Self::BASE32_LEN {
            return None;
        }
        let mut bytes = [0u8; Self::LEN];
        let mut written = 0usize;
        let mut acc: u16 = 0;
        let mut bits: u32 = 0;
        for ch in text.bytes() {
            let value = match ch.to_ascii_uppercase() {
                c @ b'A'..=b'Z' => c - b'A',
                c @ b'2'..=b'7' => c - b'2' + 26,
                _ => return None,
            };
            acc = (acc << 5) | u16::from(value);
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                bytes[written] = ((acc >> bits) & 0xff) as u8;
                written += 1;
            }
        }
        debug_assert_eq!(written, Self::LEN);
        if acc & ((1 << bits) - 1) != 0 {
            return None;
        }
        Some(ContentDigest(bytes))
    }

    /// Decodes a lowercase-hex digest string back into a `ContentDigest`, or
    /// `None` if `text` is not exactly `LEN * 2` valid hex digits. The inverse of
    /// [`to_hex`](Self::to_hex).
    pub fn from_hex(text: &str) -> Option<ContentDigest> {
        let digits = text.as_bytes();
        if digits.len() != Self::LEN * 2 {
            return None;
        }
        let mut bytes = [0u8; Self::LEN];
        for (index, pair) in digits.as_chunks::<2>().0.iter().enumerate() {
            let hi = (pair[0] as char).to_digit(16)?;
            let lo = (pair[1] as char).to_digit(16)?;
            bytes[index] = ((hi << 4) | lo) as u8;
        }
        Some(ContentDigest(bytes))
    }
}

impl fmt::Display for ContentDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for ContentDigest {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ContentDigest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        ContentDigest::from_hex(&text).ok_or_else(|| {
            serde::de::Error::custom(format!("'{text}' is not a valid SHA-256 hex digest"))
        })
    }
}
