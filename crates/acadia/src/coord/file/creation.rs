// Copyright (c) Michael Grier.

//! [`Creation`] for the file-based implementation.

use std::path::Path;

use crate::coord::creation::{
    ManagedFence, is_publication_staging_of_folded, make_artifacts, names_equal,
    validate_artifact_name, validate_plan,
};
use crate::coord::{
    ArtifactMaker, CoordError, CoordResult, Created, Creation, CreationNonce, CreationPlan,
    RESERVED_PREFIX, Waiting,
};
use crate::{FileType, LockMode, LockSpan, VfsErrorKind};

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
    Occupied(String),
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
        if std::thread::panicking() {
            self.participant.abandon_participant();
            return;
        }
        self.participant.release_lock_safely(self.id);
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

    fn keep_existing(&self, path: &Path, mode: LockMode) -> CoordResult<Kept<'_>> {
        let id = self.take_existing_lock(path, mode, LockSpan::File)?;
        Ok(Kept {
            participant: self,
            id,
        })
    }

    /// Refuses a root whose reservation names another implementation, whose participants would
    /// not see the locks this one takes.
    pub(super) fn check_implementation(&self) -> CoordResult<()> {
        self.alive()?;
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

    fn read_root(&self, plan: &CreationPlan<'_>) -> CoordResult<(Found, Vec<String>)> {
        let marker = *plan
            .artifacts
            .last()
            .expect("a validated plan has artifacts");
        let mut artifacts = Vec::new();
        let mut entries = self.vfs().read_dir(self.layout().root())?;
        entries.sort_by(|left, right| left.file_name.cmp(&right.file_name));
        if let Some(entry) = entries
            .iter()
            .find(|entry| names_equal(&entry.file_name, marker))
        {
            if entry.file_type != FileType::File {
                return Ok((Found::Occupied(entry.file_name.clone()), artifacts));
            }
            return Ok((
                match self.read_reservation()? {
                    Some(reservation)
                        if reservation.implementation == self.id
                            && reservation.inputs == plan.inputs =>
                    {
                        Found::Existing
                    }
                    _ => Found::Marker,
                },
                artifacts,
            ));
        }
        for entry in entries {
            let name = entry.file_name;
            if is_reserved(&name) {
                continue;
            }
            if !plan.artifacts.iter().any(|artifact| {
                names_equal(&name, artifact) || is_publication_staging_of_folded(&name, artifact)
            }) {
                return Ok((Found::Occupied(name), artifacts));
            }
            artifacts.push(name);
        }
        let found = match (self.read_reservation()?, artifacts.first()) {
            (Some(reservation), _) => Found::Reserved(reservation),
            (None, Some(artifact)) => Found::Occupied(artifact.clone()),
            (None, None) => Found::Empty,
        };
        Ok((found, artifacts))
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
    fn found(&self, marker: &str) -> CoordResult<Created> {
        self.alive()?;
        let actual = self
            .vfs()
            .read_dir(self.layout().root())?
            .into_iter()
            .find(|entry| names_equal(&entry.file_name, marker))
            .filter(|entry| entry.file_type == FileType::File)
            .ok_or(CoordError::NotCreated)?;
        self.vfs()
            .fsync(&self.layout().root().join(actual.file_name))?;
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

impl crate::coord::creation::ManagedFence for FileCoordination {
    fn alive(&self) -> CoordResult<()> {
        FileCoordination::alive(self)
    }

    fn fenced<T>(&self, operation: impl FnOnce() -> crate::VfsResult<T>) -> CoordResult<T> {
        self.publishing(|_| operation())
    }
}

impl Creation for FileCoordination {
    fn create_managed(
        &self,
        plan: &CreationPlan<'_>,
        maker: &mut dyn crate::coord::ManagedArtifactMaker,
    ) -> CoordResult<Created> {
        let managed =
            crate::coord::creation::ManagedPlan::new(plan, self.vfs(), self.layout().root())?;
        self.alive()?;
        let _hold = self.keep(&self.layout().creation_lock(), LockMode::Exclusive)?;
        let reservation = self.read_reservation()?;
        let compatible = reservation
            .as_ref()
            .map(|record| record.implementation == self.id && record.inputs == plan.inputs);
        managed.preflight(compatible)?;
        let _resolver = self.keep(
            &crate::names::lock_path(managed.root()),
            LockMode::Exclusive,
        )?;
        let mut directory = self
            .fenced(|| crate::names::ComponentDirectory::open_held(self.vfs(), managed.root()))?;
        managed.run_held(
            &mut directory,
            self,
            maker,
            compatible,
            || {
                self.settle_reservation(
                    reservation.map_or(Course::Create, |record| Course::Resume(record.nonce)),
                    plan.inputs,
                    plan.nonce,
                )
            },
            |path, bytes| self.publishing(|fs| fs.publish_new(path, bytes)),
        )
    }

    fn confirm_managed_created(&self, marker: &str) -> CoordResult<()> {
        let _hold = self.keep(&self.layout().creation_lock(), LockMode::Shared)?;
        self.check_implementation()?;
        self.read_reservation()?.ok_or(CoordError::NotCreated)?;
        let root =
            crate::names::ComponentDirectory::stable_parent(self.vfs(), self.layout().root())?;
        let (_, owned) = crate::names::validate_control_entries(self.vfs(), &root)?;
        if !owned || !crate::names::has_map(&self.vfs().read_dir(&root)?) {
            return Err(CoordError::NotCreated);
        }
        let _resolver = self.keep_existing(&crate::names::lock_path(&root), LockMode::Shared)?;
        let directory =
            self.fenced(|| crate::names::ComponentDirectory::open_held(self.vfs(), &root))?;
        crate::coord::creation::confirm_managed_held(self.vfs(), &root, marker, &directory, self)
    }

    fn create(
        &self,
        plan: &CreationPlan<'_>,
        maker: &mut dyn ArtifactMaker,
    ) -> CoordResult<Created> {
        validate_plan(plan)?;
        self.alive()?;
        let marker = *plan
            .artifacts
            .last()
            .expect("a validated plan has artifacts");
        let (found, _) = self.read_root(plan)?;
        let first = self.decide(found, plan)?;
        if first == Course::Existing {
            return self.found(marker);
        }
        let _exclusion = self.keep(&self.layout().creation_lock(), LockMode::Exclusive)?;
        let (found, existing) = self.read_root(plan)?;
        let course = self.decide(found, plan)?;
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
            &existing,
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

    fn confirm_created(&self, marker: &str) -> CoordResult<()> {
        validate_artifact_name(marker)?;
        self.alive()?;
        let _opening = self.keep(&self.layout().creation_lock(), LockMode::Shared)?;
        let reservation = self.read_reservation()?.ok_or(CoordError::NotCreated)?;
        if reservation.implementation != self.id {
            return Err(CoordError::ImplementationMismatch {
                recorded: reservation.implementation,
            });
        }
        let actual = self
            .vfs()
            .read_dir(self.layout().root())?
            .into_iter()
            .find(|entry| names_equal(&entry.file_name, marker))
            .filter(|entry| entry.file_type == FileType::File)
            .ok_or(CoordError::NotCreated)?;
        self.vfs()
            .fsync(&self.layout().root().join(actual.file_name))?;
        self.vfs().fsync(self.layout().root())?;
        Ok(())
    }
}

fn is_reserved(name: &str) -> bool {
    name.as_bytes().starts_with(RESERVED_PREFIX.as_bytes())
}
