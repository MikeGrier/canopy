// Copyright (c) Michael Grier.

//! [`Creation`] for the file-based implementation.

use std::ffi::{OsStr, OsString};
use std::path::Path;

use crate::coord::creation::{make_artifacts, validate_artifact_name, validate_plan};
use crate::coord::{
    ArtifactMaker, CoordError, CoordResult, Created, Creation, CreationNonce, CreationPlan,
    RESERVED_PREFIX, Waiting,
};
use crate::{LockMode, LockSpan, VfsErrorKind};

use super::reservation::Reservation;
use super::{FileCoordination, HoldId};

/// What a reading of the root found.
enum Found {
    /// The marker under a reservation of this implementation with the plan's inputs: the store
    /// asked for.
    Existing,
    /// Any other marker: the root is another store.
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
    /// Find the store made.
    Existing,
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

    /// Refuses a root whose reservation names another implementation, whose participants would
    /// not see the locks this one takes.
    pub(super) fn check_implementation(&self) -> CoordResult<()> {
        match self.read_reservation()? {
            Some(reservation) if reservation.implementation != self.id => {
                Err(CoordError::ImplementationMismatch {
                    recorded: reservation.implementation,
                })
            }
            _ => Ok(()),
        }
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
            return Ok(match self.read_reservation()? {
                Some(reservation)
                    if reservation.implementation == self.id
                        && reservation.inputs == plan.inputs =>
                {
                    Found::Existing
                }
                _ => Found::Marker,
            });
        }
        for name in names {
            if is_reserved(&name) {
                continue;
            }
            if !plan.artifacts.iter().any(|artifact| {
                *artifact == name || crate::is_publication_staging_of(&name, artifact)
            }) {
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
            Found::Existing => Ok(Course::Existing),
            Found::Marker => Err(CoordError::AlreadyCreated),
            Found::Occupied(name) => Err(CoordError::Occupied { name }),
            Found::Reserved(reservation) => self.decide_reserved(reservation, plan.inputs),
            Found::Empty => Ok(Course::Create),
        }
    }

    /// What to do about a reservation read: resume it if it is this implementation's with these
    /// inputs, refuse it otherwise.
    fn decide_reserved(&self, reservation: Reservation, inputs: &[u8]) -> CoordResult<Course> {
        if reservation.implementation == self.id && reservation.inputs == inputs {
            Ok(Course::Resume(reservation.nonce))
        } else {
            Err(CoordError::Reserved)
        }
    }

    fn decide_reservation(&self, inputs: &[u8]) -> CoordResult<Course> {
        match self.read_reservation()? {
            Some(reservation) => self.decide_reserved(reservation, inputs),
            None => Ok(Course::Create),
        }
    }

    /// Makes the marker of a store found made durable with its directory entry, as an open does:
    /// its creator may have died between making the marker and making it durable.
    fn found(&self, marker: &OsStr) -> CoordResult<Created> {
        self.alive()?;
        self.vfs().fsync(&self.layout().root().join(marker))?;
        self.vfs().fsync(self.layout().root())?;
        Ok(Created::Existing)
    }

    /// Publishes the reservation, or makes the one being resumed durable, and makes the root's
    /// directory entry for it durable.
    fn settle_reservation(
        &self,
        course: Course,
        inputs: &[u8],
        nonce: CreationNonce,
    ) -> CoordResult<Created> {
        let reservation = self.layout().reservation();
        let created = match course {
            Course::Existing => return Ok(Created::Existing),
            Course::Create => {
                let bytes = Reservation {
                    implementation: self.id.clone(),
                    nonce,
                    inputs: inputs.to_vec(),
                }
                .encode();
                match self.publishing(|vfs| vfs.publish_new(&reservation, &bytes)) {
                    Err(CoordError::Vfs(error)) if error.kind() == VfsErrorKind::AlreadyExists => {
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
        self.vfs().fsync(self.layout().root())?;
        Ok(created)
    }
}

impl Creation for FileCoordination {
    fn create(
        &self,
        plan: &CreationPlan<'_>,
        maker: &mut dyn ArtifactMaker,
    ) -> CoordResult<Created> {
        validate_plan(plan)?;
        self.alive()?;
        let marker = plan
            .artifacts
            .last()
            .expect("a validated plan has artifacts");
        let first = self.decide(self.read_root(plan)?, plan)?;
        if first == Course::Existing {
            return self.found(marker);
        }
        let _exclusion = self.keep(&self.layout().creation_lock(), LockMode::Exclusive)?;
        let course = self.decide(self.read_root(plan)?, plan)?;
        if course == Course::Existing {
            return self.found(marker);
        }
        if let (Course::Resume(read), Course::Resume(now)) = (first, course)
            && read != now
        {
            return Err(CoordError::Reserved);
        }
        let created = self.settle_reservation(course, plan.inputs, plan.nonce)?;
        make_artifacts(
            plan,
            maker,
            self.vfs(),
            self.layout().root(),
            &|| self.alive(),
            &|path, bytes| self.publishing(|vfs| vfs.publish_new(path, bytes)),
        )?;
        Ok(created)
    }

    fn reserve(&self, inputs: &[u8], nonce: CreationNonce) -> CoordResult<Created> {
        self.alive()?;
        let first = self.decide_reservation(inputs)?;
        let _exclusion = self.keep(&self.layout().creation_lock(), LockMode::Exclusive)?;
        let course = self.decide_reservation(inputs)?;
        if let (Course::Resume(read), Course::Resume(now)) = (first, course)
            && read != now
        {
            return Err(CoordError::Reserved);
        }
        self.settle_reservation(course, inputs, nonce)
    }

    fn confirm_created(&self, marker: &OsStr) -> CoordResult<()> {
        validate_artifact_name(marker)?;
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
    name.as_encoded_bytes()
        .starts_with(RESERVED_PREFIX.as_bytes())
}
