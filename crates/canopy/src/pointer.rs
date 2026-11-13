// Copyright (c) Michael Grier.

//! The namespace pointer: what a document's entry contains (`D321`, `CVS-10`).
//!
//! A pointer is the mutable half of the store. It is small, it is 7-bit ASCII, and it names
//! a location rather than a digest -- so a read derives nothing and the blob's expected
//! digest arrives inside the path it was handed.
//!
//! Every way a pointer can be refused is a separate [`PointerError`], because a reader's
//! obligation is to log *why* it rejected (`D326`) and it can only report a cause the parse
//! gave it.

use crate::body::ContentDigest;
use crate::naming::{BlobNameError, BlobRef, PathHash};
use std::fmt;

/// The byte that separates a pointer's two fields (`D321`).
///
/// Semicolon rather than dash because dash is in the base64url alphabet: a dash separator
/// parses correctly today and becomes ambiguous the moment the rendering is reconsidered.
/// Semicolon is outside hex, base32 and base64url alike, and is not a path character.
/// Changing this value is a breaking change to the on-disk format.
const SEPARATOR: u8 = b';';

/// The byte a pointer ends with, and the marker that it ended at all.
///
/// A pointer missing it was truncated. Requiring a terminator is what makes truncation
/// detectable from the last byte rather than by guessing at a short payload; it also makes
/// the file pleasant to `cat`, which is the point of a human-readable rendering.
const TERMINATOR: u8 = b'\n';

/// The offset the separator occupies, fixed because the rendering is fixed-width (`D322`).
const SEPARATOR_OFFSET: usize = ContentDigest::BASE32_LEN;

/// A namespace entry: which document this is, and where its body lives (`D321`).
///
/// Rendered as `<h(P)>;<partition-relative blob path>` followed by a newline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pointer {
    path_hash: PathHash,
    target: BlobRef,
}

impl Pointer {
    /// A pointer for the document whose path hashes to `path_hash`, naming `target`.
    pub fn new(path_hash: PathHash, target: BlobRef) -> Pointer {
        Pointer { path_hash, target }
    }

    /// The hash of the path of the document this pointer belongs to.
    pub fn path_hash(&self) -> PathHash {
        self.path_hash
    }

    /// Where the body lives, relative to the blob tree above the partition.
    pub fn target(&self) -> BlobRef {
        self.target
    }

    /// The bytes written to a namespace entry.
    pub fn render(&self) -> Vec<u8> {
        let mut out = String::with_capacity(SEPARATOR_OFFSET + 2 + 64);
        out.push_str(&self.path_hash.render());
        out.push(SEPARATOR as char);
        out.push_str(&self.target.relative_path());
        out.push(TERMINATOR as char);
        out.into_bytes()
    }

    /// Parses a namespace entry read from the location whose path hashes to `found_at`.
    ///
    /// **The location binding is checked here rather than left to the caller**, because a
    /// pointer that is well-formed but filed in the wrong place is internally perfect: no
    /// check available to the caller afterwards would catch it. Requiring the expected hash
    /// as an argument makes the check unskippable rather than merely documented.
    ///
    /// Each failure is a distinct [`PointerError`] so that a reader can name the cause it
    /// rejected for (`D326`).
    pub fn parse(bytes: &[u8], found_at: PathHash) -> Result<Pointer, PointerError> {
        if let Some(offset) = bytes.iter().position(|byte| !byte.is_ascii()) {
            return Err(PointerError::NotAscii { offset });
        }
        // Checked before anything is sliced: a truncated pointer is the failure most likely
        // to leave a prefix that parses into something plausible.
        match bytes.last() {
            Some(&TERMINATOR) => {}
            _ => return Err(PointerError::Truncated),
        }
        let body = &bytes[..bytes.len() - 1];
        if body.contains(&TERMINATOR) {
            return Err(PointerError::TrailingContent);
        }

        if body.len() <= SEPARATOR_OFFSET {
            return Err(PointerError::Truncated);
        }
        // Asserted at its offset rather than searched for, so a pointer written under a
        // different digest length is caught here instead of being mis-sliced into fields
        // that happen to parse.
        if body[SEPARATOR_OFFSET] != SEPARATOR {
            return Err(PointerError::Separator {
                expected_at: SEPARATOR_OFFSET,
                found: body[SEPARATOR_OFFSET] as char,
            });
        }

        let owner = std::str::from_utf8(&body[..SEPARATOR_OFFSET])
            .expect("ascii was checked above, so this is valid utf-8");
        let recorded = ContentDigest::from_base32(owner)
            .map(PathHash::from_digest)
            .ok_or(PointerError::OwnerNotARendering)?;

        let target = std::str::from_utf8(&body[SEPARATOR_OFFSET + 1..])
            .expect("ascii was checked above, so this is valid utf-8");
        let target = BlobRef::parse(target).map_err(PointerError::Target)?;

        let pointer = Pointer {
            path_hash: recorded,
            target,
        };
        pointer.check_owner(found_at)?;
        Ok(pointer)
    }

    /// Whether this pointer may be filed for the document whose path hashes to `owner`: it
    /// must record that owner, and name a blob that owner's writes produced.
    ///
    /// The second half is what blob collection relies on: a document's blobs are collected
    /// by its own path hash, so a pointer naming another document's blob would dangle the
    /// moment that document moved on.
    pub fn check_owner(&self, owner: PathHash) -> Result<(), PointerError> {
        if self.path_hash != owner {
            return Err(PointerError::Misfiled {
                recorded: self.path_hash.render(),
                found_at: owner.render(),
            });
        }
        let target_owner = self.target.name().path_hash();
        if target_owner != owner {
            return Err(PointerError::ForeignTarget {
                owner: owner.render(),
                target_owner: target_owner.render(),
            });
        }
        Ok(())
    }
}

impl fmt::Display for Pointer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}{}",
            self.path_hash.render(),
            SEPARATOR as char,
            self.target.relative_path()
        )
    }
}

/// Why a pointer was refused (`D321`, `D326`).
///
/// Each variant is a different thing having gone wrong, with a different implication for
/// whoever reads the log: truncation says a write was interrupted, a bad separator says the
/// file came from a build writing a different format, and misfiling says the pointer is
/// intact but is not where it claims to be. A single "malformed" would tell an operator
/// only that something is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PointerError {
    /// A byte outside 7-bit ASCII. A pointer is ASCII by construction (`D321`), so this is
    /// not our file, or not intact.
    NotAscii {
        /// Where the first offending byte sits.
        offset: usize,
    },
    /// The terminating newline is absent, or the file is empty: the write did not finish.
    Truncated,
    /// Something followed the terminator. A pointer is exactly one line.
    TrailingContent,
    /// The separator is not at the offset a fixed-width rendering puts it.
    Separator {
        /// Where it should have been.
        expected_at: usize,
        /// What was found instead.
        found: char,
    },
    /// The leading field is not a valid path-hash rendering.
    OwnerNotARendering,
    /// The pointer is well formed but records an owner other than the location it was read
    /// from -- it has been copied, restored, or written to the wrong place.
    ///
    /// This is the failure a digest over the file's own contents could never catch, because
    /// such a pointer is internally perfect (`D321`).
    Misfiled {
        /// The owner the pointer claims.
        recorded: String,
        /// The owner of the location it was found at.
        found_at: String,
    },
    /// The blob path it names is not one this build can resolve.
    Target(BlobNameError),
    /// The pointer names a blob belonging to a different document. Collection follows a
    /// blob's owner, so such a pointer would dangle once that other document changed.
    ForeignTarget {
        /// The document the pointer belongs to.
        owner: String,
        /// The document the named blob belongs to.
        target_owner: String,
    },
    /// A pointer handed to a write names its blob at a shard width other than the store's,
    /// so it would name a shard the store never writes into.
    ShardWidth {
        /// The store's shard width.
        expected: usize,
        /// The width the pointer's target was built at.
        found: usize,
    },
}

impl fmt::Display for PointerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PointerError::NotAscii { offset } => {
                write!(f, "a non-ASCII byte at offset {offset}")
            }
            PointerError::Truncated => {
                write!(f, "no terminating newline: the pointer is truncated")
            }
            PointerError::TrailingContent => {
                write!(f, "content after the terminating newline")
            }
            PointerError::Separator { expected_at, found } => write!(
                f,
                "expected '{}' at offset {expected_at}, found '{found}'",
                SEPARATOR as char
            ),
            PointerError::OwnerNotARendering => {
                write!(f, "the owner field is not a valid path hash")
            }
            PointerError::Misfiled { recorded, found_at } => write!(
                f,
                "the pointer records owner {recorded} but was read from {found_at}"
            ),
            PointerError::Target(error) => write!(f, "its target path is unusable: {error}"),
            PointerError::ForeignTarget {
                owner,
                target_owner,
            } => write!(
                f,
                "the pointer for {owner} names a blob belonging to {target_owner}"
            ),
            PointerError::ShardWidth { expected, found } => write!(
                f,
                "the pointer's target is at shard width {found}, not the store's {expected}"
            ),
        }
    }
}

impl std::error::Error for PointerError {}
