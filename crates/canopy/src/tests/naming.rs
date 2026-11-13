// Copyright (c) Michael Grier.

//! `CVS-5`: the naming vocabulary -- path hashing, base32 rendering, shard derivation,
//! and blob-name assembly and parsing.

use super::*;
use crate::body::ContentDigest;
use crate::naming::{Artifact, BlobName, BlobNameError, PathHash};

fn node(path: &str) -> NodePath {
    NodePath::parse(path).expect("valid node path")
}

fn document(name: &str) -> DocumentName {
    DocumentName::new(name).expect("valid document name")
}

fn hash(path: &str, doc: &str) -> PathHash {
    PathHash::of(&node(path), &document(doc))
}

// -- base32 rendering (`D322`) -------------------------------------------------

#[test]
fn a_rendered_digest_is_fifty_two_characters_of_the_rfc_4648_alphabet() {
    let rendered = ContentDigest::of(b"anything").to_base32();
    assert_eq!(rendered.len(), ContentDigest::BASE32_LEN);
    assert_eq!(rendered.len(), 52);
    assert!(
        rendered
            .bytes()
            .all(|c| c.is_ascii_uppercase() || (b'2'..=b'7').contains(&c)),
        "{rendered} is outside A-Z2-7"
    );
}

#[test]
fn the_alphabet_excludes_the_confusable_digits() {
    // `D322`: excluding 0, 1, 8 and 9 is what removes the 0/O and 1/I/l confusions from
    // the human-correlation path. A rendering that contained them would not be RFC 4648.
    for _ in 0..64 {
        let rendered = ContentDigest::of(&rand_like()).to_base32();
        assert!(
            !rendered.contains(['0', '1', '8', '9']),
            "{rendered} contains an excluded digit"
        );
    }
}

/// Deterministic pseudo-random bytes: the unit tests are reproducible, so this is a
/// counter-driven hash chain rather than a sampled RNG.
fn rand_like() -> Vec<u8> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    ContentDigest::of(&n.to_le_bytes()).as_bytes().to_vec()
}

#[test]
fn every_rendered_digest_ends_in_a_or_q() {
    // 256 bits is 51 characters plus one significant bit, so the final character carries
    // that bit and four zero pad bits: its value is 0 or 16, which is `A` or `Q` (`D322`).
    for _ in 0..256 {
        let rendered = ContentDigest::of(&rand_like()).to_base32();
        let last = rendered.chars().last().unwrap();
        assert!(
            last == 'A' || last == 'Q',
            "{rendered} ends in {last}, which no SHA-256 rendering can"
        );
    }
}

#[test]
fn base32_round_trips() {
    for _ in 0..64 {
        let digest = ContentDigest::of(&rand_like());
        assert_eq!(
            ContentDigest::from_base32(&digest.to_base32()),
            Some(digest)
        );
    }
}

#[test]
fn base32_decoding_accepts_either_case() {
    let digest = ContentDigest::of(b"case");
    let rendered = digest.to_base32();
    assert_eq!(
        ContentDigest::from_base32(&rendered.to_lowercase()),
        Some(digest),
        "a name that travelled through something case-folding still decodes"
    );
}

#[test]
fn base32_rejects_non_zero_pad_bits() {
    // The final character must encode one significant bit and four zeros. `B` (value 1)
    // sets a pad bit, so it is a string no encoder of ours produced.
    let digest = ContentDigest::of(b"pad");
    let mut rendered = digest.to_base32();
    rendered.pop();
    rendered.push('B');
    assert_eq!(
        ContentDigest::from_base32(&rendered),
        None,
        "a non-zero pad bit means truncation, re-encoding, or a foreign encoder"
    );
}

#[test]
fn base32_rejects_the_wrong_length_and_foreign_characters() {
    let digest = ContentDigest::of(b"x");
    let rendered = digest.to_base32();

    assert_eq!(ContentDigest::from_base32(&rendered[..51]), None);
    assert_eq!(ContentDigest::from_base32(&format!("{rendered}A")), None);

    // `0`, `1`, `8` and `9` are outside the alphabet, as are base64url's `-` and `_`.
    for foreign in ['0', '1', '8', '9', '-', '_', '=', '/'] {
        let mut broken = rendered.clone();
        broken.replace_range(0..1, &foreign.to_string());
        assert_eq!(
            ContentDigest::from_base32(&broken),
            None,
            "{foreign} is not in the alphabet"
        );
    }
}

#[test]
fn base32_is_shorter_than_hex_and_agrees_with_it() {
    let digest = ContentDigest::of(b"both renderings, one digest");
    assert_eq!(digest.to_hex().len(), 64);
    assert_eq!(digest.to_base32().len(), 52);
    // `D322`: the base32 rendering is additive, so the hex one still round-trips.
    assert_eq!(ContentDigest::from_hex(&digest.to_hex()), Some(digest));
}

// -- path hashing (`D315`) -----------------------------------------------------

#[test]
fn a_path_hash_is_stable_for_the_same_path() {
    assert_eq!(
        hash("TICKETS/A", "record.json"),
        hash("TICKETS/A", "record.json")
    );
}

#[test]
fn a_path_hash_distinguishes_every_part_of_the_path() {
    let base = hash("TICKETS/A", "record.json");
    assert_ne!(base, hash("TICKETS/B", "record.json"));
    assert_ne!(base, hash("TICKETS", "record.json"));
    assert_ne!(base, hash("TICKETS/A/B", "record.json"));
    assert_ne!(base, hash("TICKETS/A", "other.json"));
}

#[test]
fn the_hashed_encoding_is_injective_across_the_node_document_boundary() {
    // The property the whole scheme rests on: no way to redistribute characters between
    // the node path and the document name and arrive at the same hash. A separator that
    // could appear inside a component would break this, which is why neither name type
    // admits one.
    assert_ne!(hash("A/B", "C.json"), hash("A", "B.json"));
    assert_ne!(hash("A/B/C", "D.json"), hash("A/B", "C.json"));
}

#[test]
fn a_path_hash_folds_case_because_the_names_do() {
    // Not a property of the hash -- a property of the names reaching it. Both canonicalise
    // to uppercase, so two spellings are one path rather than two that collide on a
    // case-folding filesystem.
    assert_eq!(
        hash("tickets/a", "Record.JSON"),
        hash("TICKETS/A", "RECORD.json")
    );
}

// -- shard derivation (`D322`) -------------------------------------------------

#[test]
fn a_shard_is_a_visible_prefix_of_the_rendering() {
    let path_hash = hash("TICKETS/A", "record.json");
    let rendered = path_hash.render();
    for width in 1..=4 {
        let shard = path_hash.shard(width);
        assert_eq!(shard.len(), width);
        assert!(
            rendered.starts_with(&shard),
            "the shard directory must be readable off the filename"
        );
    }
}

#[test]
fn a_shard_width_past_the_rendering_is_clamped_rather_than_panicking() {
    // `open` validates the width, so this cannot arrive from a store this build accepted;
    // the naming helper still must not be the thing that panics if it ever does.
    let path_hash = hash("TICKETS/A", "record.json");
    assert_eq!(
        path_hash.shard(1_000),
        path_hash.render(),
        "clamped to the whole rendering"
    );
}

// -- blob names (`D315`, `D321`) -----------------------------------------------

#[test]
fn a_blob_name_is_owner_then_content_then_artifact() {
    let path_hash = hash("TICKETS/A", "record.json");
    let digest = ContentDigest::of(b"body bytes");
    let name = BlobName::new(path_hash, digest, Artifact::Body);

    assert_eq!(
        name.file_name(),
        format!("{}.{}.BODY", path_hash.render(), digest.to_base32())
    );
    // The owner leads so that every artifact of one document shares a prefix, which is
    // what makes `D315`'s cleanup a prefix match rather than an enumeration.
    assert!(name.file_name().starts_with(&path_hash.render()));
}

#[test]
fn a_blob_name_round_trips_through_parsing() {
    let path_hash = hash("TICKETS/A", "record.json");
    let digest = ContentDigest::of(b"body bytes");
    let name = BlobName::new(path_hash, digest, Artifact::Body);

    let parsed = BlobName::parse(&name.file_name()).expect("our own name parses");
    assert_eq!(parsed, name);
    assert_eq!(parsed.path_hash(), path_hash);
    assert_eq!(parsed.digest(), digest);
    assert_eq!(parsed.artifact(), Artifact::Body);
}

#[test]
fn the_two_renderings_are_not_interchangeable() {
    // Swapping owner and content yields a name that is well-formed, parses cleanly, and
    // addresses nothing -- which is why `PathHash` is a distinct type from `ContentDigest`
    // rather than a second one of the same.
    let path_hash = hash("TICKETS/A", "record.json");
    let digest = ContentDigest::of(b"body bytes");
    let swapped = BlobName::new(
        PathHash::from_digest(digest),
        path_hash.digest(),
        Artifact::Body,
    );
    assert_ne!(
        swapped.file_name(),
        BlobName::new(path_hash, digest, Artifact::Body).file_name()
    );
    assert!(
        BlobName::parse(&swapped.file_name()).is_ok(),
        "the swap is undetectable by parsing, which is the point"
    );
}

#[test]
fn a_relative_path_names_no_partition() {
    // `D321`: a partition-qualified path would pin every read of a document to one
    // replica and silently defeat `D317`'s rotation.
    let path_hash = hash("TICKETS/A", "record.json");
    let name = BlobName::new(path_hash, ContentDigest::of(b"x"), Artifact::Body);
    let relative = name.relative_path(2);

    assert_eq!(
        relative,
        format!("{}/{}", path_hash.shard(2), name.file_name())
    );
    assert!(!relative.contains("/p"), "{relative} names a partition");
    assert!(!relative.starts_with('/'), "{relative} is not relative");
}

#[test]
fn the_owner_prefix_matches_every_version_and_artifact() {
    let path_hash = hash("TICKETS/A", "record.json");
    let prefix = BlobName::owner_prefix(path_hash);
    for body in [&b"one"[..], b"two", b"three"] {
        let name = BlobName::new(path_hash, ContentDigest::of(body), Artifact::Body);
        assert!(name.file_name().starts_with(&prefix));
    }
    // And nothing else's.
    let other = BlobName::new(
        hash("TICKETS/B", "record.json"),
        ContentDigest::of(b"one"),
        Artifact::Body,
    );
    assert!(!other.file_name().starts_with(&prefix));
}

#[test]
fn the_version_prefix_matches_one_version_and_not_its_successor() {
    // The removal side of `D315`'s set difference: by prefix rather than by assembled
    // name, so a sweep collects artifact types added by later builds.
    let path_hash = hash("TICKETS/A", "record.json");
    let old = ContentDigest::of(b"old");
    let new = ContentDigest::of(b"new");

    let prefix = BlobName::version_prefix(path_hash, old);
    assert!(
        BlobName::new(path_hash, old, Artifact::Body)
            .file_name()
            .starts_with(&prefix)
    );
    assert!(
        !BlobName::new(path_hash, new, Artifact::Body)
            .file_name()
            .starts_with(&prefix)
    );
}

#[test]
fn a_rewrite_of_identical_content_lands_on_the_same_name() {
    // Why `D315` expresses cleanup as a set difference rather than "delete the old one":
    // writing identical content to the same path produces the same name, so "delete the
    // old" would delete the new.
    let path_hash = hash("TICKETS/A", "record.json");
    let first = BlobName::new(path_hash, ContentDigest::of(b"same"), Artifact::Body);
    let second = BlobName::new(path_hash, ContentDigest::of(b"same"), Artifact::Body);
    assert_eq!(first.file_name(), second.file_name());
}

#[test]
fn parsing_rejects_a_name_that_is_not_ours() {
    let path_hash = hash("TICKETS/A", "record.json");
    let digest = ContentDigest::of(b"x");
    let good = BlobName::new(path_hash, digest, Artifact::Body).file_name();

    assert_eq!(
        BlobName::parse("record.json"),
        Err(BlobNameError::FieldCount)
    );
    assert_eq!(
        BlobName::parse(&good[..good.len() - 5]),
        Err(BlobNameError::FieldCount)
    );
    assert_eq!(
        BlobName::parse(&format!("{good}.EXTRA")),
        Err(BlobNameError::FieldCount)
    );
    assert_eq!(
        BlobName::parse(&format!("NOTAHASH.{}.BODY", digest.to_base32())),
        Err(BlobNameError::PathHash)
    );
    assert_eq!(
        BlobName::parse(&format!("{}.NOTADIGEST.BODY", path_hash.render())),
        Err(BlobNameError::Digest)
    );
    assert_eq!(
        BlobName::parse(&format!(
            "{}.{}.CHUNKMAP",
            path_hash.render(),
            digest.to_base32()
        )),
        Err(BlobNameError::UnknownArtifact)
    );
}

#[test]
fn an_artifact_extension_is_an_enumerated_vocabulary() {
    assert_eq!(Artifact::Body.as_str(), "BODY");
    assert_eq!(Artifact::from_extension("BODY"), Some(Artifact::Body));
    assert_eq!(Artifact::from_extension("body"), Some(Artifact::Body));
    assert_eq!(Artifact::from_extension("JSON"), None);
    assert_eq!(Artifact::from_extension(""), None);
}

#[test]
fn the_body_carries_an_explicit_extension() {
    // A bare stem would make "a body" the shape you get when parsing finds no suffix,
    // which is how an unrecognised artifact type gets silently adopted as a body.
    let name = BlobName::new(
        hash("TICKETS/A", "record.json"),
        ContentDigest::of(b"x"),
        Artifact::Body,
    );
    assert_eq!(name.file_name().split('.').count(), 3);
    assert!(name.file_name().ends_with(".BODY"));
}

#[test]
fn a_blob_name_fits_the_path_budget_it_was_sized_for() {
    // `D322` computed 110 characters for a name at this width, with roughly 137 left for a
    // store root under MAX_PATH. Both renderings are fixed-length, so this is exact.
    let name = BlobName::new(
        hash("TICKETS/A", "record.json"),
        ContentDigest::of(b"x"),
        Artifact::Body,
    );
    assert_eq!(name.file_name().len(), 52 + 1 + 52 + 1 + 4);
    assert_eq!(name.file_name().len(), 110);
    assert!(
        name.file_name().len() < 255,
        "a name must fit one NTFS path component"
    );
}
