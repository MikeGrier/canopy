// Copyright (c) Michael Grier.

//! Which step of the model a store's call belongs to.

use std::path::Path;

use acadia::FaultOp;

use super::projection::{PUBLICATION, World};
use crate::commit_log::LOG_NAME;
use crate::store::{BLOB_TREE_DIR, NAMESPACE_TREE_DIR, STAGING_DIR};
/// The kind of step a store's call takes, as far as the model can tell steps apart: which file
/// of the document's it touches, how, and on which replica.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tests::conformance) enum Class {
    RecordLook,
    RecordAppend,
    RecordSync,
    RecordTruncate,
    PtrRename(usize),
    PtrSync(usize),
    PtrRead,
    BlobRename(usize),
    BlobSync(usize),
    BlobList(usize),
    BlobRemove(usize),
    LogAppend,
    LogRead,
    LogTruncate,
    Other,
}

pub(super) fn classify(world: &World, op: FaultOp, path: &Path, to: Option<&Path>) -> Class {
    classify_root(&world.root, op, path, to)
}

pub(in crate::tests::conformance) fn classify_root(
    root: &Path,
    op: FaultOp,
    path: &Path,
    to: Option<&Path>,
) -> Class {
    let target = if op == FaultOp::Rename {
        to.unwrap_or(path)
    } else {
        path
    };
    let Ok(relative) = target.strip_prefix(root) else {
        return Class::Other;
    };
    let parts: Vec<String> = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    let part = |index: usize| parts.get(index).map(String::as_str);
    let partition = part(1)
        .and_then(|name| name.strip_prefix('p'))
        .and_then(|index| index.parse::<usize>().ok());
    match (part(0), parts.len(), partition) {
        (Some(PUBLICATION), 1, _) => match op {
            FaultOp::Metadata => Class::RecordLook,
            FaultOp::Append => Class::RecordAppend,
            FaultOp::Fsync => Class::RecordSync,
            FaultOp::Truncate => Class::RecordTruncate,
            _ => Class::Other,
        },
        (Some(LOG_NAME), 1, _) => match op {
            FaultOp::Read => Class::LogRead,
            FaultOp::Append => Class::LogAppend,
            FaultOp::Truncate => Class::LogTruncate,
            _ => Class::Other,
        },
        (Some(NAMESPACE_TREE_DIR), length, Some(index))
            if length >= 2 && part(2) != Some(STAGING_DIR) =>
        {
            match op {
                FaultOp::Rename => Class::PtrRename(index),
                FaultOp::Fsync => Class::PtrSync(index),
                FaultOp::Read => Class::PtrRead,
                _ => Class::Other,
            }
        }
        (Some(BLOB_TREE_DIR), length, Some(index))
            if length >= 3 && part(2) != Some(STAGING_DIR) =>
        {
            match (op, length) {
                (FaultOp::Rename, 4) => Class::BlobRename(index),
                (FaultOp::RemoveFile, 4) => Class::BlobRemove(index),
                (FaultOp::Fsync, 3) => Class::BlobSync(index),
                (FaultOp::ReadDir, 3) => Class::BlobList(index),
                _ => Class::Other,
            }
        }
        _ => Class::Other,
    }
}

/// Whether a call of `class` is the first call of a step that begins with `first`: of the same
/// kind, at the same replica or a later one. A repair takes a step at each replica, renaming
/// only those that hold nothing, so the model fails at a replica that is whole where the store
/// fails its next rename.
pub(super) fn begins(first: Class, class: Class) -> bool {
    match (first, class) {
        (Class::PtrRename(from), Class::PtrRename(at))
        | (Class::PtrSync(from), Class::PtrSync(at))
        | (Class::BlobRename(from), Class::BlobRename(at))
        | (Class::BlobSync(from), Class::BlobSync(at))
        | (Class::BlobList(from), Class::BlobList(at)) => at >= from,
        _ => first == class,
    }
}

/// The first call of the step the model takes from `phase` at replica `idx` (counted from 1).
pub(super) fn first_call(phase: &str, idx: usize) -> Class {
    let index = idx - 1;
    match phase {
        "rfRecSync" => Class::RecordLook,
        "recWrite" => Class::RecordAppend,
        "recSync" | "rfClearSync" | "recClearSync" => Class::RecordSync,
        "rfClear" | "recClear" => Class::RecordTruncate,
        "rfRename" | "ptrRename" | "txRfRename" | "repRename" | "resRename" => {
            Class::PtrRename(index)
        }
        "rfSync" | "ptrSync" | "txRfSync" | "repSync" | "resSync" | "collSync" | "rfCollSync" => {
            Class::PtrSync(index)
        }
        "ingestRename" => Class::BlobRename(index),
        "ingestSync" | "collFsync" | "rfCollFsync" => Class::BlobSync(index),
        "collRemove" | "rfCollRemove" => Class::BlobList(index),
        "collRead" | "rfCollRead" | "chkRead" | "repRead" | "resStart" => Class::PtrRead,
        "txBegin" | "txPrepare" | "txEnd" | "txRfEnd" => Class::LogAppend,
        "rbReset" => Class::LogTruncate,
        other => panic!("no call starts the model's phase {other}"),
    }
}
