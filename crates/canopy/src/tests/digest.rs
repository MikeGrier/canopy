// Copyright (c) Michael Grier.
// Split from tests.rs at 2a4dd8f.

//! `ContentDigest` determinism and the opaque `DocumentBody` accessors.

use super::*;

// -- ContentDigest / DocumentBody ----------------------------------------------

#[test]
fn body_accessors_report_bytes_len_and_emptiness() {
    let body = DocumentBody::new(vec![1u8, 2, 3]);
    assert_eq!(body.as_bytes(), &[1, 2, 3]);
    assert_eq!(body.len(), 3);
    assert!(!body.is_empty());
    assert!(DocumentBody::new(Vec::new()).is_empty());
}

#[test]
fn body_from_slice_and_vec_are_equal() {
    let from_vec = DocumentBody::new(vec![9u8, 8, 7]);
    let from_slice: DocumentBody = [9u8, 8, 7].as_slice().into();
    assert_eq!(from_vec, from_slice);
}

#[test]
fn into_bytes_round_trips() {
    let bytes = vec![4u8, 5, 6];
    let body = DocumentBody::new(bytes.clone());
    assert_eq!(body.into_bytes(), bytes);
}

#[test]
fn digest_is_deterministic_for_equal_bytes() {
    let a = DocumentBody::new(b"hello world".to_vec());
    let b = DocumentBody::new(b"hello world".to_vec());
    assert_eq!(a.digest(), b.digest());
    // Recomputation is stable.
    assert_eq!(a.digest(), a.digest());
}

#[test]
fn digest_differs_for_different_bytes() {
    let a = DocumentBody::new(b"hello".to_vec());
    let b = DocumentBody::new(b"world".to_vec());
    assert_ne!(a.digest(), b.digest());
}

#[test]
fn digest_of_empty_body_is_the_known_sha256() {
    let digest = DocumentBody::new(Vec::new()).digest();
    assert_eq!(
        digest.to_hex(),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn digest_of_abc_is_the_known_sha256() {
    let digest = ContentDigest::of(b"abc");
    assert_eq!(
        digest.to_hex(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn body_digest_matches_content_digest_of_its_bytes() {
    let body = DocumentBody::new(b"payload".to_vec());
    assert_eq!(body.digest(), ContentDigest::of(body.as_bytes()));
}

#[test]
fn digest_hex_is_thirty_two_bytes_of_lowercase_hex() {
    let digest = ContentDigest::of(b"anything");
    assert_eq!(digest.as_bytes().len(), ContentDigest::LEN);
    let hex = digest.to_hex();
    assert_eq!(hex.len(), ContentDigest::LEN * 2);
    assert!(
        hex.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    assert_eq!(format!("{digest}"), hex);
}

#[test]
fn from_hex_round_trips_with_to_hex() {
    let digest = ContentDigest::of(b"round-trip me");
    let hex = digest.to_hex();
    assert_eq!(ContentDigest::from_hex(&hex), Some(digest));
}

#[test]
fn from_hex_rejects_the_wrong_length() {
    assert_eq!(ContentDigest::from_hex(""), None);
    assert_eq!(ContentDigest::from_hex("ab"), None);
    let too_long = "a".repeat(ContentDigest::LEN * 2 + 2);
    assert_eq!(ContentDigest::from_hex(&too_long), None);
}

#[test]
fn from_hex_rejects_a_non_hex_character() {
    let mostly_valid = "g".repeat(ContentDigest::LEN * 2);
    assert_eq!(ContentDigest::from_hex(&mostly_valid), None);
}

#[test]
fn from_hex_accepts_uppercase_like_to_digit_does() {
    let digest = ContentDigest::of(b"case");
    let uppercase = digest.to_hex().to_uppercase();
    assert_eq!(ContentDigest::from_hex(&uppercase), Some(digest));
}

#[test]
fn serde_round_trips_through_the_same_hex_string_as_display() {
    let digest = ContentDigest::of(b"serde");
    let json = serde_json::to_string(&digest).unwrap();
    assert_eq!(json, format!("\"{digest}\""));
    let back: ContentDigest = serde_json::from_str(&json).unwrap();
    assert_eq!(back, digest);
}

#[test]
fn serde_deserialize_rejects_a_malformed_hex_string() {
    let result: Result<ContentDigest, _> = serde_json::from_str("\"not-hex\"");
    assert!(result.is_err());
}
