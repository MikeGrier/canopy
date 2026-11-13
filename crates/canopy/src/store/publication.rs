// Copyright (c) Michael Grier.

//! The durable publication record, which guards an unlogged write against `STRANDED-VOTE`.
//!
//! An unlogged write records the version it is about to publish, durably, before it renames its
//! first pointer replica, and clears the record once every replica names that version durably.
//! Every operation on the store settles first: it makes the record file durable whether or not
//! the file shows a record, rolls every replica of a recorded document forward to the recorded
//! version, and clears the record. A write that fails part-way therefore leaves its document
//! holding at most its old version and the recorded one, and the next operation finishes the
//! publication, so the replicas never lose their majority. The rules are the ones
//! `spec/PointerPublication.tla` checks under its record guard (DESIGN-NOTES "`STRANDED-VOTE` is
//! guarded by a durable publication record").

use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

/// The publication record at the store root: NDJSON lines, each naming a document an unlogged
/// write is publishing and the pointer it is publishing. One writer at a time, and a settle
/// before every write, keep it to at most one record. Changing this value is a breaking change
/// to the on-disk layout. A dotfile for the reason [`MARKER_NAME`] is: no node name begins with
/// `.`, so it cannot collide with a shard directory.
pub const PUBLICATION_RECORD_NAME: &str = ".publication";

/// The on-disk shape of one record. canopy owns it; the value types stay serde-free and are
/// validated again when a record is read.
#[derive(Serialize, Deserialize)]
struct WirePublication {
    node: String,
    document: String,
    /// Base16 of the published body's digest, as the commit log writes one.
    digest: String,
    /// The nonce the write stages its pointer replicas under, which a roll-forward reuses.
    nonce: String,
}

/// One record: the document and the pointer its write is publishing.
struct Publication {
    node: NodePath,
    document: DocumentName,
    digest: ContentDigest,
    nonce: Nonce,
}

/// Whether the record file's name is known durable in the store root, shared by every clone so
/// the root's fsync is paid once per open rather than once per write.
#[derive(Debug, Default)]
pub(crate) struct PublicationDurability {
    name_synced: AtomicBool,
}

impl Store {
    fn publication_path(&self) -> PathBuf {
        self.root.join(PUBLICATION_RECORD_NAME)
    }

    /// Durably records that (`node`, `document`) is being published as the body `digest`
    /// names, under `nonce`.
    ///
    /// **The write moves no pointer unless this returns `Ok`.** A record that reached the file
    /// and not the disk is made durable by the next settle before it rolls anything forward, so
    /// a crash can never leave renamed replicas behind a record that is gone (T24).
    pub(crate) fn record_publication(
        &self,
        node: &NodePath,
        document: &DocumentName,
        digest: ContentDigest,
        nonce: &Nonce,
    ) -> Result<(), StoreError> {
        let wire = WirePublication {
            node: node.to_string(),
            document: document.as_str().to_owned(),
            digest: digest.to_hex(),
            nonce: nonce.as_str().to_owned(),
        };
        let mut line = serde_json::to_vec(&wire)?;
        line.push(b'\n');
        let path = self.publication_path();
        let created = match self.vfs.metadata(&path) {
            Ok(_) => false,
            Err(error) if error.kind() == VfsErrorKind::NotFound => true,
            Err(error) => return Err(StoreError::Vfs(error)),
        };
        if created {
            self.publication.name_synced.store(false, Ordering::Release);
        }
        self.vfs.append(&path, &line)?;
        self.vfs.fsync(&path)?;
        self.sync_publication_name()
    }

    /// Clears the record, once every replica names its version durably.
    ///
    /// A truncation whose fsync fails leaves the durable record behind an empty file, which is
    /// why a settle fsyncs the file before it reads it (T25).
    pub(crate) fn clear_publication(&self) -> Result<(), StoreError> {
        let path = self.publication_path();
        self.vfs.truncate(&path, 0)?;
        self.vfs.fsync(&path)?;
        Ok(())
    }

    /// Settles the record: makes the file durable whether or not it shows a record, rolls every
    /// replica of each recorded document forward to the recorded version, clears the record,
    /// and collects what the roll-forward superseded.
    ///
    /// Run before every operation by [`writable`](Store::writable), and by `open`. A settle
    /// that fails fails its operation with the record still pending, for the next one to settle.
    /// An unterminated last line is an append that never completed, and so a write that moved
    /// no pointer; it names nothing and is cleared with the rest.
    pub(crate) fn settle_publication(&self) -> Result<(), StoreError> {
        let path = self.publication_path();
        match self.vfs.metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == VfsErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(StoreError::Vfs(error)),
        }
        self.vfs.fsync(&path)?;
        self.sync_publication_name()?;
        let bytes = self.vfs.read(&path)?;
        if bytes.is_empty() {
            return Ok(());
        }
        let complete = match bytes.iter().rposition(|&b| b == b'\n') {
            Some(last_newline) => &bytes[..=last_newline],
            None => &bytes[..0],
        };
        let mut settled = Vec::new();
        for line in complete
            .split(|&b| b == b'\n')
            .filter(|line| !line.is_empty())
        {
            let publication = self.parse_publication(line)?;
            self.write_body_pointer(
                &publication.node,
                &publication.document,
                publication.digest,
                &publication.nonce,
            )?;
            settled.push(publication);
        }
        self.clear_publication()?;
        for publication in &settled {
            self.collect_document_blobs(&publication.node, &publication.document)?;
        }
        Ok(())
    }

    /// Makes the record file's name durable in the store root, once per open.
    fn sync_publication_name(&self) -> Result<(), StoreError> {
        if !self.publication.name_synced.load(Ordering::Acquire) {
            self.vfs.fsync(&self.root)?;
            self.publication.name_synced.store(true, Ordering::Release);
        }
        Ok(())
    }

    /// One complete record line. A complete line that does not parse is not one this store
    /// wrote, and is refused rather than skipped: skipping it could leave a document's
    /// publication unfinished.
    fn parse_publication(&self, line: &[u8]) -> Result<Publication, StoreError> {
        let malformed = |detail: String| StoreError::MalformedPublicationRecord {
            root: self.root.clone(),
            detail,
        };
        let wire: WirePublication =
            serde_json::from_slice(line).map_err(|error| malformed(error.to_string()))?;
        let node = NodePath::parse(&wire.node)
            .map_err(|error| malformed(format!("invalid node: {error}")))?;
        let document = DocumentName::new(&wire.document)
            .map_err(|error| malformed(format!("invalid document: {error}")))?;
        let digest = ContentDigest::from_hex(&wire.digest)
            .ok_or_else(|| malformed(format!("invalid digest: {:?}", wire.digest)))?;
        let nonce = Nonce::new(&wire.nonce)
            .map_err(|error| malformed(format!("invalid nonce: {error}")))?;
        Ok(Publication {
            node,
            document,
            digest,
            nonce,
        })
    }
}
