// Copyright (c) Michael Grier.

//! [`CoordError`]: why a coordination operation did not do what it was asked.

use std::ffi::OsString;
use std::fmt;

use crate::VfsError;

use super::types::ImplementationId;

/// The result of a coordination operation.
pub type CoordResult<T> = Result<T, CoordError>;

/// Why a coordination operation did not do what it was asked.
///
/// Each variant other than [`CoordError::Vfs`] is a refusal the contract states, and an operation
/// refused by one changes nothing unless its documentation says otherwise.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum CoordError {
    /// The filesystem failed beneath the implementation.
    Vfs(VfsError),
    /// The participant was abandoned, so it acts no more.
    Abandoned,
    /// An acquisition asked not to wait, and would have.
    WouldWait,
    /// The root is coordinated by another implementation.
    ImplementationMismatch {
        /// The implementation the root records.
        recorded: ImplementationId,
    },
    /// The root holds something creation does not make.
    Occupied {
        /// The first such name found beneath the root.
        name: OsString,
    },
    /// A creation of the root was begun by another implementation, or with other inputs; only
    /// that implementation, with those inputs, may resume it.
    Reserved,
    /// A creation's plan or a marker's name cannot be what creation makes: no artifacts, or a name
    /// that is not one plain name directly beneath the root, or one with the coordination's
    /// reserved prefix.
    InvalidPlan {
        /// What is wrong with it.
        why: &'static str,
    },
    /// The root is a store already.
    AlreadyCreated,
    /// The root is not a store: it has no marker, though it may hold a creation that was never
    /// finished.
    NotCreated,
    /// Recovery is required, so an ordinary modification is refused.
    RecoveryRequired,
    /// A modification was ended with none open.
    NoModificationOpen,
    /// Recovery was completed with a modification still open.
    ModificationOpen,
    /// A file of the coordination state does not hold what the implementation writes there.
    Malformed {
        /// Which file.
        what: &'static str,
    },
}

impl fmt::Display for CoordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoordError::Vfs(error) => write!(f, "coordination failed in the filesystem: {error}"),
            CoordError::Abandoned => f.write_str("the participant was abandoned"),
            CoordError::WouldWait => f.write_str("the acquisition would have waited"),
            CoordError::ImplementationMismatch { recorded } => {
                write!(f, "the root is coordinated by {recorded}")
            }
            CoordError::Occupied { name } => write!(
                f,
                "the root holds {} which creation does not make",
                name.to_string_lossy()
            ),
            CoordError::Reserved => f.write_str(
                "a creation of the root was begun by another implementation or with other inputs",
            ),
            CoordError::InvalidPlan { why } => write!(f, "the creation plan is invalid: {why}"),
            CoordError::AlreadyCreated => f.write_str("the root is a store already"),
            CoordError::NotCreated => f.write_str("the root is not a store"),
            CoordError::RecoveryRequired => f.write_str("recovery is required"),
            CoordError::NoModificationOpen => f.write_str("no modification is open"),
            CoordError::ModificationOpen => f.write_str("a modification is still open"),
            CoordError::Malformed { what } => {
                write!(f, "the coordination state's {what} is malformed")
            }
        }
    }
}

impl std::error::Error for CoordError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CoordError::Vfs(error) => Some(error),
            _ => None,
        }
    }
}

impl From<VfsError> for CoordError {
    fn from(error: VfsError) -> Self {
        CoordError::Vfs(error)
    }
}
