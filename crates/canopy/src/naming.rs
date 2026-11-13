// Copyright (c) Michael Grier.

//! The content-verified store's naming vocabulary (`D315`, `D321`, `D322`, `CVS-5`).
//!
//! A blob's name is `<h(P)>.<digest>.<ext>`, under a shard directory taken from the leading
//! characters of `h(P)`. The two renderings do different jobs: the **digest** establishes
//! identity and makes the blob self-validating, and the **owner hash** makes the blob the
//! property of exactly one document, which is what prevents sharing (`D315`).
//!
//! Everything here is write-side. A read is handed the blob's partition-relative path by
//! the pointer and derives nothing (`D321`), so the assembly below runs when a blob is
//! created and the parsing below runs when a directory is swept.

use crate::body::ContentDigest;
use crate::name::{DocumentName, NodePath};
use std::fmt;

/// The separator between a blob name's fields.
///
/// A `.` cannot appear in a rendered hash (base32 is alphanumeric) and cannot begin or end
/// a [`DocumentName`], so it partitions the name unambiguously. It is also what makes the
/// artifact suffix read as a conventional file extension.
const FIELD_SEPARATOR: char = '.';

/// Which artifact of a document a blob holds (`D315`).
///
/// The extension is an **enumerated vocabulary**, not a free string: a name whose suffix is
/// not one of these is not one of ours, and says so at parse time rather than by failing to
/// open later.
///
/// The body carries an explicit extension rather than a bare stem so every artifact parses
/// by the same rule. A bare stem would make "a body" the shape you get when parsing fails
/// to find a suffix, which is exactly how an unrecognised artifact type would be silently
/// adopted as a body.
///
/// Adding a variant is additive on disk: `D315`'s cleanup sweeps by prefix, so an older
/// build's removal still collects artifact types it has never heard of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Artifact {
    /// The document body.
    Body,
}

impl Artifact {
    /// The extension this artifact is written with.
    ///
    /// Uppercase, like every other field of the name: the renderings are uppercase base32
    /// and the namespace canonicalises to uppercase, so a mixed-case extension would be the
    /// only part of a path that carried case, and case is not load-bearing on the
    /// filesystems these names live on (`D322`).
    pub fn as_str(self) -> &'static str {
        match self {
            Artifact::Body => "BODY",
        }
    }

    /// The artifact an extension names, or `None` if this build does not know it.
    ///
    /// Case-insensitive for the same reason [`ContentDigest::from_base32`] is: the name may
    /// have travelled through something that folded it, and a format that emits one case
    /// but refuses the other is claiming a round-trip it does not have.
    pub fn from_extension(text: &str) -> Option<Artifact> {
        if text.eq_ignore_ascii_case(Artifact::Body.as_str()) {
            Some(Artifact::Body)
        } else {
            None
        }
    }
}

impl fmt::Display for Artifact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `h(P)` -- the hash of the path of the document that owns a blob (`D315`).
///
/// A distinct type from [`ContentDigest`] despite the same underlying bytes, because a blob
/// name carries two renderings and putting them in the wrong order produces a name that is
/// well-formed, parses cleanly, and addresses nothing. The type makes that unwriteable.
///
/// # The width is a correctness parameter
///
/// This is the **full** 256-bit hash, and it is not a length to trim for tidiness or to buy
/// back path budget. Two distinct paths colliding here, holding identical content, produce
/// the same blob name -- so the two documents share a blob, and then deleting one document
/// destroys the other's only copy. There is no detector for that: `D315` establishes that a
/// blob's lifetime is exactly its owner's, and a shared blob has no single owner, so the
/// cleanup sweep does precisely the wrong thing with complete confidence.
///
/// Truncating to 128 bits would make that a birthday problem over the store's document
/// count rather than an impossibility, in exchange for 26 characters of path. `D322`
/// already showed the budget is not tight at this width. Do not trade it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PathHash(ContentDigest);

impl PathHash {
    /// Hashes the path of the document named `document` under `node`.
    ///
    /// The hashed bytes are the node's components and the document name, joined by
    /// [`NodePath::SEPARATOR`] -- the rendered path, exactly as a reader would write it.
    ///
    /// **That encoding is injective**, which is the property the whole scheme rests on. No
    /// component and no document name may contain the separator, and neither may be empty,
    /// so the rendered form splits back into its parts at the separators with the last
    /// field always the document. Both are canonicalised to uppercase ASCII before they
    /// reach here, so two names that differ only in case are the same name rather than two
    /// paths that collide on a case-folding filesystem.
    pub fn of(node: &NodePath, document: &DocumentName) -> PathHash {
        let mut rendered = String::new();
        for component in node.components() {
            rendered.push_str(component);
            rendered.push(NodePath::SEPARATOR);
        }
        rendered.push_str(document.as_str());
        PathHash(ContentDigest::of(rendered.as_bytes()))
    }

    /// Rebuilds a path hash from raw bytes, without recomputing it.
    pub fn from_digest(digest: ContentDigest) -> PathHash {
        PathHash(digest)
    }

    /// The underlying digest.
    pub fn digest(&self) -> ContentDigest {
        self.0
    }

    /// The base32 rendering used in every on-disk name (`D322`).
    pub fn render(&self) -> String {
        self.0.to_base32()
    }

    /// The value replica selection rotates on (`D317`).
    ///
    /// Selection only needs a number that is stable for a path and spread evenly across
    /// documents, both of which the leading bytes of a hash already are. This is **not** an
    /// identity: it is lossy by construction, and two documents sharing a seed simply
    /// prefer the same replica, which costs nothing.
    pub fn rotation_seed(&self) -> u64 {
        let bytes = self.0.as_bytes();
        let mut seed = [0u8; 8];
        seed.copy_from_slice(&bytes[..8]);
        u64::from_le_bytes(seed)
    }

    /// The shard directory for this owner: the leading `width` characters of the rendering.
    ///
    /// A *visible* prefix of the filename, which is what lets a human correlate a pointer
    /// against a directory listing (`D321`). Fan-out is `32^width`.
    ///
    /// `width` comes from the genesis marker and is validated there, so a width past the
    /// rendering's length cannot arrive from a store this build agreed to open; it is
    /// clamped rather than panicking, because a naming helper is the wrong place to
    /// re-litigate a marker that `open` already accepted.
    pub fn shard(&self, width: usize) -> String {
        let rendered = self.render();
        let width = width.min(rendered.len());
        rendered[..width].to_string()
    }
}

impl fmt::Display for PathHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

/// A blob's name: its owner, its content, and which artifact it is (`D315`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlobName {
    path_hash: PathHash,
    digest: ContentDigest,
    artifact: Artifact,
}

impl BlobName {
    /// The name for `artifact` of the document whose path hashes to `path_hash`, holding
    /// content whose digest is `digest`.
    pub fn new(path_hash: PathHash, digest: ContentDigest, artifact: Artifact) -> BlobName {
        BlobName {
            path_hash,
            digest,
            artifact,
        }
    }

    /// The owning document's path hash.
    pub fn path_hash(&self) -> PathHash {
        self.path_hash
    }

    /// The digest of the content this blob holds.
    pub fn digest(&self) -> ContentDigest {
        self.digest
    }

    /// Which artifact this blob is.
    pub fn artifact(&self) -> Artifact {
        self.artifact
    }

    /// The file name: `<h(P)>.<digest>.<ext>`.
    ///
    /// The path hash leads so that every artifact of one document shares a prefix, which is
    /// what makes the cleanup sweep of `D315` a prefix match rather than an enumeration.
    pub fn file_name(&self) -> String {
        format!(
            "{}{FIELD_SEPARATOR}{}{FIELD_SEPARATOR}{}",
            self.path_hash.render(),
            self.digest.to_base32(),
            self.artifact.as_str()
        )
    }

    /// The blob's path **relative to the blob tree above the partition** (`D321`).
    ///
    /// Partition-relative, never partition-qualified: a name that pinned a partition would
    /// defeat the replica rotation of `D317` by sending every read of a document to the
    /// same copy forever.
    pub fn relative_path(&self, shard_width: usize) -> String {
        format!("{}/{}", self.path_hash.shard(shard_width), self.file_name())
    }

    /// The prefix matching **every artifact of every version** this document has ever had:
    /// `<h(P)>.`.
    ///
    /// The enumeration side of `D315`'s set-difference cleanup -- "after this transaction
    /// the blobs belonging to `P` are exactly the new one; remove any others". Lookup never
    /// needs this, and it must exist anyway, because only a prefix sweep stays complete as
    /// the artifact vocabulary grows.
    pub fn owner_prefix(path_hash: PathHash) -> String {
        format!("{}{FIELD_SEPARATOR}", path_hash.render())
    }

    /// The prefix matching **every artifact of one version**:
    /// `<h(P)>.<digest>.`.
    ///
    /// The removal side of the same sweep. By prefix rather than by assembled name so that
    /// removing a superseded version collects artifact types added by later builds (`D315`).
    pub fn version_prefix(path_hash: PathHash, digest: ContentDigest) -> String {
        format!(
            "{}{FIELD_SEPARATOR}{}{FIELD_SEPARATOR}",
            path_hash.render(),
            digest.to_base32()
        )
    }

    /// The owner and content fields of `file_name`, **without requiring the artifact to be
    /// one this build knows**.
    ///
    /// This is what the cleanup sweep matches on (`D315`). Full [`parse`](BlobName::parse)
    /// would be wrong there: it rejects an unknown extension, so a build older than the
    /// artifact type that wrote a file would skip it and leave a superseded blob behind
    /// forever. Ownership and version are decidable from the first two fields alone, and
    /// those are the only two a sweep needs.
    ///
    /// Returns `None` for anything that is not shaped like one of our names, which is how a
    /// foreign file that wandered into a shard directory survives a sweep.
    pub fn owner_and_version(file_name: &str) -> Option<(&str, &str)> {
        let mut fields = file_name.split(FIELD_SEPARATOR);
        let owner = fields.next()?;
        let version = fields.next()?;
        // A third field must exist -- a two-field name is not one of ours -- but what it
        // says is deliberately not inspected.
        fields.next()?;
        if owner.len() != ContentDigest::BASE32_LEN || version.len() != ContentDigest::BASE32_LEN {
            return None;
        }
        Some((owner, version))
    }

    /// Parses a blob file name back into its three fields.
    ///
    /// Strict in both directions: a name with the wrong field count, a rendering that is not
    /// well-formed base32 of the right length with zero pad bits, or an artifact this build
    /// does not know all fail here rather than producing a name that addresses nothing.
    pub fn parse(file_name: &str) -> Result<BlobName, BlobNameError> {
        let mut fields = file_name.split(FIELD_SEPARATOR);
        let (Some(owner), Some(content), Some(extension), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(BlobNameError::FieldCount);
        };

        let path_hash = ContentDigest::from_base32(owner)
            .map(PathHash::from_digest)
            .ok_or(BlobNameError::PathHash)?;
        let digest = ContentDigest::from_base32(content).ok_or(BlobNameError::Digest)?;
        let artifact = Artifact::from_extension(extension).ok_or(BlobNameError::UnknownArtifact)?;

        Ok(BlobName {
            path_hash,
            digest,
            artifact,
        })
    }
}

impl fmt::Display for BlobName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.file_name())
    }
}

/// A blob's partition-relative path, as a pointer carries it (`D321`).
///
/// The parsed form of what [`BlobName::relative_path`] produces, and its inverse. A read is
/// handed one of these and derives nothing: no path hash, no shard computation, no name
/// assembly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlobRef {
    name: BlobName,
    shard_width: usize,
}

impl BlobRef {
    /// Parses `<shard>/<h(P)>.<digest>.<ext>`.
    ///
    /// **The shard is checked against the name rather than trusted**, because the two carry
    /// the same information and a disagreement means the reference was assembled wrongly or
    /// damaged in transit. It is the cheapest of `D321`'s cross-checks and the only one
    /// available before any I/O happens.
    pub fn parse(relative_path: &str) -> Result<BlobRef, BlobNameError> {
        let (shard, file_name) = relative_path
            .split_once('/')
            .ok_or(BlobNameError::MissingShard)?;
        if shard.is_empty() || file_name.contains('/') {
            return Err(BlobNameError::MissingShard);
        }

        let name = BlobName::parse(file_name)?;
        if !name.path_hash().render().starts_with(shard) {
            return Err(BlobNameError::ShardMismatch);
        }

        Ok(BlobRef {
            name,
            shard_width: shard.len(),
        })
    }

    /// The reference to `name`'s blob at `shard_width`.
    ///
    /// Refused unless the width names a non-empty prefix of the rendered path hash, which is
    /// exactly the set of references [`parse`](BlobRef::parse) can read back: a reference
    /// that renders to something its own parser rejects must not be constructible.
    pub fn new(name: BlobName, shard_width: usize) -> Result<BlobRef, BlobNameError> {
        if shard_width == 0 || shard_width > ContentDigest::BASE32_LEN {
            return Err(BlobNameError::ShardWidth { width: shard_width });
        }
        Ok(BlobRef { name, shard_width })
    }

    /// The shard width this reference was built at.
    pub fn shard_width(&self) -> usize {
        self.shard_width
    }

    /// The blob's name.
    pub fn name(&self) -> BlobName {
        self.name
    }

    /// The shard directory this reference names.
    pub fn shard(&self) -> String {
        self.name.path_hash().shard(self.shard_width)
    }

    /// The partition-relative path, as a pointer carries it.
    pub fn relative_path(&self) -> String {
        self.name.relative_path(self.shard_width)
    }
}

impl fmt::Display for BlobRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.relative_path())
    }
}

/// Why a blob file name could not be parsed.
///
/// Each field is named separately rather than collapsed into one "malformed" case, because
/// these distinguish a foreign file that wandered into the tree from one of ours that is
/// damaged, and a sweep should treat those differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobNameError {
    /// The name did not have exactly three `.`-separated fields.
    FieldCount,
    /// The owner field was not a well-formed rendering.
    PathHash,
    /// The content-digest field was not a well-formed rendering.
    Digest,
    /// The extension is not an artifact type this build knows.
    UnknownArtifact,
    /// A blob reference carried no shard component.
    MissingShard,
    /// A blob reference's shard directory is not a prefix of its own path hash.
    ShardMismatch,
    /// A blob reference was asked for a shard width that names no non-empty prefix of the
    /// rendered path hash.
    ShardWidth {
        /// The rejected width.
        width: usize,
    },
}

impl fmt::Display for BlobNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlobNameError::FieldCount => {
                write!(f, "a blob name has exactly three '.'-separated fields")
            }
            BlobNameError::PathHash => write!(f, "the owner field is not a valid path hash"),
            BlobNameError::Digest => write!(f, "the content field is not a valid digest"),
            BlobNameError::UnknownArtifact => {
                write!(f, "the extension is not a known artifact type")
            }
            BlobNameError::MissingShard => {
                write!(f, "a blob reference is '<shard>/<name>'")
            }
            BlobNameError::ShardMismatch => {
                write!(f, "the shard directory is not a prefix of the path hash")
            }
            BlobNameError::ShardWidth { width } => write!(
                f,
                "shard width {width} names no non-empty prefix of a path hash"
            ),
        }
    }
}

impl std::error::Error for BlobNameError {}
