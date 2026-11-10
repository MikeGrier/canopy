// Copyright (c) Michael Grier.

//! `CVS-10`: the pointer -- location binding, target path, and a named cause for every
//! rejection.

use super::*;
use crate::body::ContentDigest;
use crate::naming::{Artifact, BlobName, BlobNameError, BlobRef, PathHash};
use crate::pointer::{Pointer, PointerError};

fn node_of(path: &str) -> NodePath {
    NodePath::parse(path).expect("valid node path")
}

fn doc_of(name: &str) -> DocumentName {
    DocumentName::new(name).expect("valid document name")
}

fn hash_of(path: &str, doc: &str) -> PathHash {
    PathHash::of(&node_of(path), &doc_of(doc))
}

/// A pointer for `TICKETS/A`'s `record.json`, targeting a blob holding `body`.
fn pointer(body: &[u8]) -> (PathHash, Pointer) {
    let path_hash = hash_of("TICKETS/A", "record.json");
    let name = BlobName::new(path_hash, ContentDigest::of(body), Artifact::Body);
    (
        path_hash,
        Pointer::new(path_hash, BlobRef::new(name, 2).unwrap()),
    )
}

#[test]
fn a_pointer_round_trips_through_its_rendering() {
    let (path_hash, original) = pointer(b"body");
    let parsed = Pointer::parse(&original.render(), path_hash).expect("our own pointer parses");

    assert_eq!(parsed, original);
    assert_eq!(parsed.path_hash(), path_hash);
    assert_eq!(parsed.target(), original.target());
}

#[test]
fn a_pointer_is_two_ascii_fields_and_a_terminator() {
    let (path_hash, p) = pointer(b"body");
    let rendered = p.render();
    let text = std::str::from_utf8(&rendered).expect("ASCII by construction");

    assert!(rendered.iter().all(u8::is_ascii));
    assert_eq!(*rendered.last().unwrap(), b'\n');
    assert_eq!(text.matches('\n').count(), 1);

    let (owner, target) = text.trim_end().split_once(';').expect("one separator");
    assert_eq!(owner, path_hash.render());
    assert_eq!(target, p.target().relative_path());
}

#[test]
fn the_separator_sits_at_the_offset_a_fixed_width_rendering_puts_it() {
    let (_, p) = pointer(b"body");
    let rendered = p.render();
    assert_eq!(rendered[ContentDigest::BASE32_LEN], b';');
}

#[test]
fn the_client_visible_path_never_appears_in_a_pointer() {
    // Which is what guarantees the file is ASCII whatever a caller names things (`D321`).
    let (_, p) = pointer(b"body");
    let text = String::from_utf8(p.render()).unwrap();
    assert!(!text.contains("TICKETS"));
    assert!(!text.contains("record.json"));
}

// -- rejections, each with its own cause (`D326`) -------------------------------

#[test]
fn a_pointer_without_its_terminator_is_truncated_not_malformed() {
    // The distinction matters to whoever reads the log: truncation says a write was
    // interrupted, which is a different investigation from a bad field.
    let (path_hash, p) = pointer(b"body");
    let mut rendered = p.render();
    rendered.pop();

    assert_eq!(
        Pointer::parse(&rendered, path_hash),
        Err(PointerError::Truncated)
    );
    assert_eq!(Pointer::parse(b"", path_hash), Err(PointerError::Truncated));
}

#[test]
fn a_truncation_that_leaves_a_plausible_prefix_is_still_caught() {
    // The nastiest truncation is the one that stops on a field boundary, where what
    // survives looks like a whole pointer. The terminator is what catches it.
    let (path_hash, p) = pointer(b"body");
    let rendered = p.render();
    let cut = ContentDigest::BASE32_LEN + 1 + 3;

    assert_eq!(
        Pointer::parse(&rendered[..cut], path_hash),
        Err(PointerError::Truncated)
    );
}

#[test]
fn content_after_the_terminator_is_refused() {
    let (path_hash, p) = pointer(b"body");
    let mut rendered = p.render();
    rendered.extend_from_slice(b"a second line\n");

    assert_eq!(
        Pointer::parse(&rendered, path_hash),
        Err(PointerError::TrailingContent)
    );
}

#[test]
fn a_non_ascii_byte_is_refused_and_says_where() {
    let (path_hash, p) = pointer(b"body");
    let mut rendered = p.render();
    rendered[3] = 0xC3;

    assert_eq!(
        Pointer::parse(&rendered, path_hash),
        Err(PointerError::NotAscii { offset: 3 })
    );
}

#[test]
fn a_separator_out_of_place_names_the_offset_rather_than_mis_slicing() {
    // A pointer written by a build using a different digest length puts its separator
    // somewhere else. Asserting the offset catches that; searching for the first `;`
    // would slice the fields apart at the wrong place and then blame whichever field
    // happened to fail.
    let (path_hash, p) = pointer(b"body");
    let mut rendered = p.render();
    rendered[ContentDigest::BASE32_LEN] = b'-';

    match Pointer::parse(&rendered, path_hash) {
        Err(PointerError::Separator { expected_at, found }) => {
            assert_eq!(expected_at, ContentDigest::BASE32_LEN);
            assert_eq!(found, '-');
        }
        other => panic!("expected a separator rejection, got {other:?}"),
    }
}

#[test]
fn an_owner_field_that_is_not_a_rendering_is_refused() {
    let (path_hash, p) = pointer(b"body");
    let mut rendered = p.render();
    // `0` is outside the base32 alphabet, which is exactly why `D322` excluded it.
    rendered[0] = b'0';

    assert_eq!(
        Pointer::parse(&rendered, path_hash),
        Err(PointerError::OwnerNotARendering)
    );
}

#[test]
fn an_owner_whose_pad_bits_are_set_is_refused() {
    // The free check `D322` leaves in the rendering: the final character of a SHA-256 is
    // always `A` or `Q`, so anything else means truncation or a foreign encoder.
    let (path_hash, p) = pointer(b"body");
    let mut rendered = p.render();
    rendered[ContentDigest::BASE32_LEN - 1] = b'B';

    assert_eq!(
        Pointer::parse(&rendered, path_hash),
        Err(PointerError::OwnerNotARendering)
    );
}

#[test]
fn a_misfiled_pointer_is_caught_by_its_location_binding() {
    // The failure a digest over the file's own contents could never catch: this pointer is
    // internally perfect and simply is not where it says it belongs (`D321`).
    let (recorded, p) = pointer(b"body");
    let elsewhere = hash_of("TICKETS/B", "record.json");

    match Pointer::parse(&p.render(), elsewhere) {
        Err(PointerError::Misfiled {
            recorded: claimed,
            found_at,
        }) => {
            assert_eq!(claimed, recorded.render());
            assert_eq!(found_at, elsewhere.render());
        }
        other => panic!("expected a misfiling rejection, got {other:?}"),
    }
}

#[test]
fn the_location_binding_cannot_be_skipped() {
    // `parse` takes the expected owner rather than returning a pointer and trusting the
    // caller to check, so there is no way to obtain a `Pointer` without the check having
    // run. This test exists to fail if that signature is ever loosened.
    let (path_hash, p) = pointer(b"body");
    let rendered = p.render();

    assert!(Pointer::parse(&rendered, path_hash).is_ok());
    for other in ["TICKETS/B", "TICKETS/A/A", "OTHER"] {
        assert!(
            Pointer::parse(&rendered, hash_of(other, "record.json")).is_err(),
            "{other} should not accept this pointer"
        );
    }
}

#[test]
fn a_target_that_names_no_shard_is_refused_as_a_target_problem() {
    // The cause is attributed to the field that failed rather than to the pointer as a
    // whole, so a log line points at the blob path and not at the namespace.
    let path_hash = hash_of("TICKETS/A", "record.json");
    let name = BlobName::new(path_hash, ContentDigest::of(b"body"), Artifact::Body);
    let rendered = format!("{};{}\n", path_hash.render(), name.file_name());

    assert_eq!(
        Pointer::parse(rendered.as_bytes(), path_hash),
        Err(PointerError::Target(BlobNameError::MissingShard))
    );
}

#[test]
fn a_target_whose_shard_disagrees_with_its_name_is_refused() {
    let path_hash = hash_of("TICKETS/A", "record.json");
    let name = BlobName::new(path_hash, ContentDigest::of(b"body"), Artifact::Body);
    let rendered = format!("{};ZZ/{}\n", path_hash.render(), name.file_name());

    assert_eq!(
        Pointer::parse(rendered.as_bytes(), path_hash),
        Err(PointerError::Target(BlobNameError::ShardMismatch))
    );
}

#[test]
fn a_target_that_names_a_partition_is_refused() {
    // `D321`: a partition-qualified target would pin every read of this document to one
    // replica and silently defeat `D317`'s rotation. It cannot be expressed.
    let path_hash = hash_of("TICKETS/A", "record.json");
    let name = BlobName::new(path_hash, ContentDigest::of(b"body"), Artifact::Body);
    let rendered = format!("{};p0/{}\n", path_hash.render(), name.relative_path(2));

    assert!(matches!(
        Pointer::parse(rendered.as_bytes(), path_hash),
        Err(PointerError::Target(_))
    ));
}

#[test]
fn every_rejection_says_something_different() {
    // `D326`: a reader can only log the cause the parse gave it, so the causes have to be
    // distinguishable both as values and as text.
    let (path_hash, p) = pointer(b"body");
    let name = BlobName::new(path_hash, ContentDigest::of(b"body"), Artifact::Body);

    let mut truncated = p.render();
    truncated.pop();
    let mut trailing = p.render();
    trailing.extend_from_slice(b"more\n");
    let mut not_ascii = p.render();
    not_ascii[1] = 0x80;
    let mut separator = p.render();
    separator[ContentDigest::BASE32_LEN] = b'x';
    let mut owner = p.render();
    owner[0] = b'1';

    let causes: Vec<String> = [
        Pointer::parse(&truncated, path_hash),
        Pointer::parse(&trailing, path_hash),
        Pointer::parse(&not_ascii, path_hash),
        Pointer::parse(&separator, path_hash),
        Pointer::parse(&owner, path_hash),
        Pointer::parse(&p.render(), hash_of("TICKETS/B", "record.json")),
        Pointer::parse(
            format!("{};{}\n", path_hash.render(), name.file_name()).as_bytes(),
            path_hash,
        ),
    ]
    .iter()
    .map(|result| result.as_ref().unwrap_err().to_string())
    .collect();

    let distinct: std::collections::BTreeSet<&String> = causes.iter().collect();
    assert_eq!(
        distinct.len(),
        causes.len(),
        "two rejections read identically in a log: {causes:#?}"
    );
    for cause in &causes {
        assert!(!cause.is_empty());
        assert!(
            !cause.eq_ignore_ascii_case("malformed"),
            "a cause must say what was wrong"
        );
    }
}

#[test]
fn a_pointer_naming_another_documents_blob_is_refused() {
    // Collection follows the blob's owner, so a pointer at another document's blob would
    // dangle the moment that document changed.
    let owner = hash_of("TICKETS/A", "record.json");
    let other = hash_of("TICKETS/B", "record.json");
    let foreign = BlobName::new(other, ContentDigest::of(b"body"), Artifact::Body);
    let pointer = Pointer::new(owner, BlobRef::new(foreign, 2).unwrap());

    assert!(matches!(
        Pointer::parse(&pointer.render(), owner),
        Err(PointerError::ForeignTarget { .. })
    ));
    assert!(matches!(
        pointer.check_owner(owner),
        Err(PointerError::ForeignTarget { .. })
    ));
}

#[test]
fn writing_a_pointer_at_another_documents_blob_is_refused_before_anything_lands() {
    let (_vfs, store) = fresh_store();
    let node = node_of("TICKETS/A");
    let document = doc_of("record.json");
    let other = hash_of("TICKETS/B", "record.json");
    let foreign = BlobName::new(other, ContentDigest::of(b"body"), Artifact::Body);
    let pointer = Pointer::new(
        PathHash::of(&node, &document),
        BlobRef::new(foreign, 2).unwrap(),
    );

    let refused = store.write_pointer(&node, &document, &pointer, &nonce("p1"));
    assert!(
        matches!(
            refused,
            Err(StoreError::InvalidPointer(
                PointerError::ForeignTarget { .. }
            ))
        ),
        "got {refused:?}"
    );
    assert!(matches!(
        store.read_pointer(&node, &document),
        Err(StoreError::PointerUnavailable { .. })
    ));
}

#[test]
fn a_reference_is_constructible_exactly_at_the_widths_its_parser_reads_back() {
    let (path_hash, _) = pointer(b"body");
    let name = BlobName::new(path_hash, ContentDigest::of(b"body"), Artifact::Body);

    for width in [0, ContentDigest::BASE32_LEN + 1, usize::MAX] {
        assert_eq!(
            BlobRef::new(name, width),
            Err(BlobNameError::ShardWidth { width }),
            "width {width}"
        );
    }
    for width in 1..=ContentDigest::BASE32_LEN {
        let reference = BlobRef::new(name, width).expect("a valid width");
        assert_eq!(reference.shard_width(), width);
        assert_eq!(
            BlobRef::parse(&reference.relative_path()),
            Ok(reference),
            "width {width} does not round-trip"
        );
    }
}

#[test]
fn writing_a_pointer_at_another_shard_width_is_refused_before_anything_lands() {
    let (_vfs, store) = fresh_store();
    let node = node_of("TICKETS/A");
    let document = doc_of("record.json");
    let owner = PathHash::of(&node, &document);
    let name = BlobName::new(owner, ContentDigest::of(b"body"), Artifact::Body);

    for width in [1, store.shard_width() + 1] {
        let pointer = Pointer::new(owner, BlobRef::new(name, width).unwrap());
        let refused = store.write_pointer(&node, &document, &pointer, &nonce("p1"));
        assert!(
            matches!(
                refused,
                Err(StoreError::InvalidPointer(PointerError::ShardWidth { expected, found }))
                    if expected == store.shard_width() && found == width
            ),
            "width {width}: got {refused:?}"
        );
    }
    assert!(matches!(
        store.read_pointer(&node, &document),
        Err(StoreError::PointerUnavailable { .. })
    ));

    let at_store_width = Pointer::new(owner, BlobRef::new(name, store.shard_width()).unwrap());
    store
        .write_pointer(&node, &document, &at_store_width, &nonce("p2"))
        .expect("the store's own width is accepted");
    assert_eq!(
        store.read_pointer(&node, &document).unwrap().pointer(),
        at_store_width
    );
}
