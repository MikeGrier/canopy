// Copyright (c) Michael Grier.

//! `CVS-6`..`CVS-8`: the content store -- ingest, read, and the set-difference sweep.
//!
//! What stays here is the vocabulary every part shares. `ingest`, `blob_path` and
//! `reference` are each used by more than one part, so they live in the parent rather than
//! in whichever part happened to need one first.

use super::*;
use crate::body::ContentDigest;
use crate::naming::{Artifact, BlobName, PathHash};
use crate::store::BLOB_TREE_DIR;

mod backlog;
mod ingest;
mod read;
mod scale;
mod sweep;

fn node_of(path: &str) -> NodePath {
    NodePath::parse(path).expect("valid node path")
}

fn doc_of(name: &str) -> DocumentName {
    DocumentName::new(name).expect("valid document name")
}

fn nonce_of(token: &str) -> Nonce {
    Nonce::new(token).expect("valid nonce")
}

fn ingest(store: &Store, body: &[u8], token: &str) -> BlobName {
    store
        .ingest_blob(
            &node_of("TICKETS/A"),
            &doc_of("record.json"),
            Artifact::Body,
            body,
            &nonce_of(token),
        )
        .expect("ingest")
}

fn blob_path(root: &std::path::Path, index: usize, name: &BlobName, width: usize) -> PathBuf {
    root.join(BLOB_TREE_DIR)
        .join(format!("p{index}"))
        .join(name.relative_path(width))
}

/// The reference a pointer will carry for `name`, at this store's shard width.
fn reference(store: &Store, name: &BlobName) -> crate::naming::BlobRef {
    crate::naming::BlobRef::parse(&name.relative_path(store.shard_width()))
        .expect("our own relative path parses")
}
