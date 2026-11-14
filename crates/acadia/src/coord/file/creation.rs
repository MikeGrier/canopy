// Copyright (c) Michael Grier.

//! [`Creation`] for the file-based implementation.

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path};

use crate::coord::{
    ArtifactMaker, CoordError, CoordResult, Created, Creation, CreationNonce, CreationPlan,
    RESERVED_PREFIX, Waiting,
};
use crate::{LockMode, LockSpan, VfsErrorKind};

use super::reservation::Reservation;
use super::{FileCoordination, HoldId};

/// What a reading of the root found.
enum Found {
    /// The marker: the root is a store.
    Marker,
    /// Something creation does not make.
    Occupied(OsString),
    /// A creation was begun.
    Reserved(Reservation),
    /// Nothing but the coordination's own.
    Empty,
}

/// What a creator does about what it found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Course {
    Create,
    /// Resume the creation whose reservation has this nonce.
    Resume(CreationNonce),
}

/// A lock kept in the participant's table of holds until this is dropped.
struct Kept<'a> {
    participant: &'a FileCoordination,
    id: HoldId,
}

impl Drop for Kept<'_> {
    fn drop(&mut self) {
        drop(self.participant.release_lock(self.id));
    }
}

impl FileCoordination {
    fn keep(&self, path: &Path, mode: LockMode) -> CoordResult<Kept<'_>> {
        let id = self.take_lock(path, mode, LockSpan::File, Waiting::Wait)?;
        Ok(Kept {
            participant: self,
            id,
        })
    }

    fn read_reservation(&self) -> CoordResult<Option<Reservation>> {
        match self.vfs().read(&self.layout().reservation()) {
            Ok(bytes) => Reservation::decode(&bytes).map(Some),
            Err(error) if error.kind() == VfsErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn read_root(&self, plan: &CreationPlan<'_>) -> CoordResult<Found> {
        let marker = plan
            .artifacts
            .last()
            .expect("a validated plan has artifacts");
        let mut artifact_found = None;
        let mut names: Vec<OsString> = self
            .vfs()
            .read_dir(self.layout().root())?
            .into_iter()
            .map(|entry| entry.file_name)
            .collect();
        names.sort();
        if names.iter().any(|name| name == marker) {
            return Ok(Found::Marker);
        }
        for name in names {
            if is_reserved(&name) {
                continue;
            }
            if !plan.artifacts.iter().any(|artifact| *artifact == name) {
                return Ok(Found::Occupied(name));
            }
            artifact_found.get_or_insert(name);
        }
        match (self.read_reservation()?, artifact_found) {
            (Some(reservation), _) => Ok(Found::Reserved(reservation)),
            (None, Some(artifact)) => Ok(Found::Occupied(artifact)),
            (None, None) => Ok(Found::Empty),
        }
    }

    fn decide(&self, found: Found, plan: &CreationPlan<'_>) -> CoordResult<Course> {
        match found {
            Found::Marker => Err(CoordError::AlreadyCreated),
            Found::Occupied(name) => Err(CoordError::Occupied { name }),
            Found::Reserved(reservation)
                if reservation.implementation == self.id && reservation.inputs == plan.inputs =>
            {
                Ok(Course::Resume(reservation.nonce))
            }
            Found::Reserved(_) => Err(CoordError::Reserved),
            Found::Empty => Ok(Course::Create),
        }
    }
}

impl Creation for FileCoordination {
    fn create(
        &self,
        plan: &CreationPlan<'_>,
        maker: &mut dyn ArtifactMaker,
    ) -> CoordResult<Created> {
        validate(plan)?;
        self.alive()?;
        let first = self.decide(self.read_root(plan)?, plan)?;
        let _exclusion = self.keep(&self.layout().creation_lock(), LockMode::Exclusive)?;
        let course = self.decide(self.read_root(plan)?, plan)?;
        if let (Course::Resume(read), Course::Resume(now)) = (first, course)
            && read != now
        {
            return Err(CoordError::Reserved);
        }
        let root = self.layout().root();
        let reservation = self.layout().reservation();
        let created = match course {
            Course::Create => {
                let bytes = Reservation {
                    implementation: self.id.clone(),
                    nonce: plan.nonce,
                    inputs: plan.inputs.to_vec(),
                }
                .encode();
                match self.vfs().publish_new(&reservation, &bytes) {
                    Err(error) if error.kind() == VfsErrorKind::AlreadyExists => {
                        return Err(CoordError::Reserved);
                    }
                    published => published?,
                }
                Created::Created
            }
            Course::Resume(_) => {
                self.vfs().fsync(&reservation)?;
                Created::Resumed
            }
        };
        self.vfs().fsync(root)?;
        for artifact in plan.artifacts {
            maker.make(artifact, plan.inputs)?;
            self.vfs().fsync(root)?;
        }
        Ok(created)
    }

    fn confirm_created(&self, marker: &OsStr) -> CoordResult<()> {
        validate_name(marker)?;
        self.alive()?;
        let _opening = self.keep(&self.layout().creation_lock(), LockMode::Shared)?;
        let reservation = self.read_reservation()?.ok_or(CoordError::NotCreated)?;
        if reservation.implementation != self.id {
            return Err(CoordError::ImplementationMismatch {
                recorded: reservation.implementation,
            });
        }
        let path = self.layout().root().join(marker);
        if !self.vfs().exists(&path)? {
            return Err(CoordError::NotCreated);
        }
        self.vfs().fsync(&path)?;
        self.vfs().fsync(self.layout().root())?;
        Ok(())
    }
}

fn is_reserved(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.starts_with(RESERVED_PREFIX))
}

fn validate(plan: &CreationPlan<'_>) -> CoordResult<()> {
    if plan.artifacts.is_empty() {
        return Err(CoordError::InvalidPlan {
            why: "a creation makes at least its marker",
        });
    }
    for (i, artifact) in plan.artifacts.iter().enumerate() {
        validate_name(artifact)?;
        if plan.artifacts[..i].contains(artifact) {
            return Err(CoordError::InvalidPlan {
                why: "an artifact is named twice",
            });
        }
    }
    Ok(())
}

fn validate_name(name: &OsStr) -> CoordResult<()> {
    let mut components = Path::new(name).components();
    if !matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(only)), None) if only == name
    ) {
        return Err(CoordError::InvalidPlan {
            why: "an artifact is one plain name directly beneath the root",
        });
    }
    if is_reserved(name) {
        return Err(CoordError::InvalidPlan {
            why: "an artifact's name has the coordination's reserved prefix",
        });
    }
    Ok(())
}
