// Copyright (c) Michael Grier.

use super::*;
use crate::VfsErrorKind;
use crate::coord::creation::{
    is_publication_staging_of_folded, make_artifacts, names_equal, validate_artifact_name,
    validate_plan,
};
use crate::coord::{
    ArtifactMaker, Created, Creation, CreationNonce, CreationPlan, RESERVED_PREFIX,
};

/// Changing these tags or names breaks creation admission between implementations.
mod format {
    pub const RESERVATION: &str = ".coord-reservation";
    pub const LOCK: &str = ".coord-shared-creation.lock";
    pub const MAGIC: [u8; 4] = *b"ACRV";
    pub const VERSION: u8 = 1;
    pub const NONCE_LEN: usize = 16;
    pub const LENGTH_WIDTH: usize = 8;
}

struct Reservation {
    implementation: ImplementationId,
    nonce: CreationNonce,
    inputs: Vec<u8>,
}

impl Reservation {
    fn encode(&self) -> Vec<u8> {
        let identity = self.implementation.as_str().as_bytes();
        let mut record = format::MAGIC.to_vec();
        record.push(format::VERSION);
        record.push(u8::try_from(identity.len()).expect("bounded implementation identity"));
        record.extend_from_slice(identity);
        record.extend_from_slice(&self.nonce.to_bytes());
        record.extend_from_slice(&(self.inputs.len() as u64).to_le_bytes());
        record.extend_from_slice(&self.inputs);
        record
    }

    fn decode(bytes: &[u8]) -> CoordResult<Self> {
        let invalid = || CoordError::Malformed {
            what: "reservation",
        };
        let mut remaining = bytes;
        let mut take = |length: usize| {
            if remaining.len() < length {
                return Err(invalid());
            }
            let (head, tail) = remaining.split_at(length);
            remaining = tail;
            Ok(head)
        };
        if take(format::MAGIC.len())? != format::MAGIC || take(1)? != [format::VERSION] {
            return Err(invalid());
        }
        let width = usize::from(take(1)?[0]);
        let implementation =
            ImplementationId::new(std::str::from_utf8(take(width)?).map_err(|_| invalid())?)
                .ok_or_else(invalid)?;
        let nonce =
            CreationNonce::from_bytes(take(format::NONCE_LEN)?.try_into().map_err(|_| invalid())?);
        let width = usize::try_from(u64::from_le_bytes(
            take(format::LENGTH_WIDTH)?
                .try_into()
                .map_err(|_| invalid())?,
        ))
        .map_err(|_| invalid())?;
        let inputs = take(width)?.to_vec();
        if !remaining.is_empty() {
            return Err(invalid());
        }
        Ok(Self {
            implementation,
            nonce,
            inputs,
        })
    }
}

enum Found {
    Empty,
    Reserved(CreationNonce),
    /// The marker under a reservation of this implementation with the plan's inputs.
    Existing,
    Marker,
    Occupied(String),
}

struct CreationHold {
    inner: Arc<ParticipantInner>,
    id: u64,
}

impl Drop for CreationHold {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.inner.abandon();
            return;
        }
        let lock = self
            .inner
            .lock(&self.inner.creation)
            .ok()
            .and_then(|mut owned| owned.remove(&self.id));
        if let Some(lock) = lock
            && crate::coord::catch_contained(|| drop(lock)).is_none()
        {
            self.inner.abandon();
        }
    }
}

fn reserved(name: &str) -> bool {
    name.as_bytes().starts_with(RESERVED_PREFIX.as_bytes())
}

impl SharedParticipant {
    fn creation_hold(&self, mode: LockMode) -> CoordResult<CreationHold> {
        self.alive()?;
        let lock =
            self.inner
                .vfs
                .lock(&self.inner.root.join(format::LOCK), mode, LockSpan::File)?;
        let mut owned = self.inner.lock(&self.inner.creation)?;
        self.alive()?;
        let id = self
            .inner
            .next_creation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .map_err(|_| CoordError::Exhausted {
                what: "creation hold identity",
            })?;
        owned.insert(id, lock);
        Ok(CreationHold {
            inner: Arc::clone(&self.inner),
            id,
        })
    }
    fn reservation(&self) -> CoordResult<Option<Reservation>> {
        match self
            .inner
            .vfs
            .read(&self.inner.root.join(format::RESERVATION))
        {
            Ok(bytes) => Reservation::decode(&bytes).map(Some),
            Err(error) if error.kind() == VfsErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn check_implementation(&self) -> CoordResult<()> {
        if let Some(reservation) = self.reservation()?
            && reservation.implementation != IMPLEMENTATION
        {
            return Err(CoordError::ImplementationMismatch {
                recorded: reservation.implementation,
            });
        }
        Ok(())
    }

    fn reservation_course(&self, inputs: &[u8]) -> CoordResult<Found> {
        match self.reservation()? {
            Some(reservation)
                if reservation.implementation == IMPLEMENTATION && reservation.inputs == inputs =>
            {
                Ok(Found::Reserved(reservation.nonce))
            }
            Some(_) => Err(CoordError::Reserved),
            None => Ok(Found::Empty),
        }
    }

    fn read_root(&self, plan: &CreationPlan<'_>) -> CoordResult<(Found, Vec<String>)> {
        let marker = *plan.artifacts.last().expect("validated plan");
        let mut entries = self.inner.vfs.read_dir(&self.inner.root)?;
        entries.sort_by(|left, right| left.file_name.cmp(&right.file_name));
        if let Some(entry) = entries
            .iter()
            .find(|entry| names_equal(&entry.file_name, marker))
        {
            if entry.file_type != crate::FileType::File {
                return Ok((Found::Occupied(entry.file_name.clone()), Vec::new()));
            }
            return Ok((
                match self.reservation()? {
                    Some(reservation)
                        if reservation.implementation == IMPLEMENTATION
                            && reservation.inputs == plan.inputs =>
                    {
                        Found::Existing
                    }
                    _ => Found::Marker,
                },
                Vec::new(),
            ));
        }
        let mut artifacts = Vec::new();
        for entry in entries {
            let name = entry.file_name;
            if reserved(&name) {
                continue;
            }
            if !plan.artifacts.iter().any(|artifact| {
                names_equal(&name, artifact) || is_publication_staging_of_folded(&name, artifact)
            }) {
                return Ok((Found::Occupied(name), artifacts));
            }
            artifacts.push(name);
        }
        let found = match (self.reservation_course(plan.inputs)?, artifacts.first()) {
            (Found::Empty, Some(name)) => Found::Occupied(name.clone()),
            (course, _) => course,
        };
        Ok((found, artifacts))
    }

    fn creation_course(found: Found) -> CoordResult<Found> {
        match found {
            Found::Marker => Err(CoordError::AlreadyCreated),
            Found::Occupied(name) => Err(CoordError::Occupied { name }),
            course => Ok(course),
        }
    }

    /// Makes the marker of a store found made durable with its directory entry, as an open does:
    /// its creator may have died between making the marker and making it durable.
    fn found(&self, marker: &str) -> CoordResult<Created> {
        self.alive()?;
        let actual = self
            .inner
            .vfs
            .read_dir(&self.inner.root)?
            .into_iter()
            .find(|entry| names_equal(&entry.file_name, marker))
            .filter(|entry| entry.file_type == crate::FileType::File)
            .ok_or(CoordError::NotCreated)?;
        self.inner
            .vfs
            .fsync(&self.inner.root.join(actual.file_name))?;
        self.inner.vfs.fsync(&self.inner.root)?;
        Ok(Created::Existing)
    }

    /// Publishes `bytes` at `path` under the table `abandon` clears, with `hold` still in it, so
    /// nothing is published once `abandon` has returned.
    fn publish_held(&self, hold: &CreationHold, path: &Path, bytes: &[u8]) -> CoordResult<()> {
        let owned = self.inner.lock(&self.inner.creation)?;
        if !owned.contains_key(&hold.id) {
            return Err(CoordError::Abandoned);
        }
        self.alive()?;
        Ok(self.inner.vfs.publish_new(path, bytes)?)
    }

    fn settle(
        &self,
        hold: &CreationHold,
        course: Found,
        inputs: &[u8],
        nonce: CreationNonce,
    ) -> CoordResult<Created> {
        self.alive()?;
        let path = self.inner.root.join(format::RESERVATION);
        let result = match course {
            Found::Empty => {
                let reservation = Reservation {
                    implementation: IMPLEMENTATION.clone(),
                    nonce,
                    inputs: inputs.to_vec(),
                };
                match self.publish_held(hold, &path, &reservation.encode()) {
                    Err(CoordError::Vfs(error)) if error.kind() == VfsErrorKind::AlreadyExists => {
                        return Err(CoordError::Reserved);
                    }
                    result => result?,
                }
                Created::Created
            }
            Found::Reserved(_) => {
                self.inner.vfs.fsync(&path)?;
                Created::Resumed
            }
            _ => return Err(CoordError::Reserved),
        };
        self.inner.vfs.fsync(&self.inner.root)?;
        Ok(result)
    }
}

impl Creation for SharedParticipant {
    fn create(
        &self,
        plan: &CreationPlan<'_>,
        maker: &mut dyn ArtifactMaker,
    ) -> CoordResult<Created> {
        self.alive()?;
        validate_plan(plan)?;
        let marker = *plan.artifacts.last().expect("validated plan");
        let (found, _) = self.read_root(plan)?;
        let first = Self::creation_course(found)?;
        if matches!(first, Found::Existing) {
            return self.found(marker);
        }
        let creation = self.creation_hold(LockMode::Exclusive)?;
        self.alive()?;
        let (found, existing) = self.read_root(plan)?;
        let course = Self::creation_course(found)?;
        if matches!(course, Found::Existing) {
            return self.found(marker);
        }
        if let (Found::Reserved(before), Found::Reserved(now)) = (&first, &course)
            && before != now
        {
            return Err(CoordError::Reserved);
        }
        let result = self.settle(&creation, course, plan.inputs, plan.nonce)?;
        make_artifacts(
            plan,
            maker,
            &*self.inner.vfs,
            &self.inner.root,
            &existing,
            &|| self.alive(),
            &|path, bytes| self.publish_held(&creation, path, bytes),
        )?;
        Ok(result)
    }

    fn reserve(&self, inputs: &[u8], nonce: CreationNonce) -> CoordResult<Created> {
        self.alive()?;
        let first = self.reservation_course(inputs)?;
        let creation = self.creation_hold(LockMode::Exclusive)?;
        self.alive()?;
        let course = self.reservation_course(inputs)?;
        if let (Found::Reserved(before), Found::Reserved(now)) = (&first, &course)
            && before != now
        {
            return Err(CoordError::Reserved);
        }
        self.settle(&creation, course, inputs, nonce)
    }

    fn confirm_created(&self, marker: &str) -> CoordResult<()> {
        validate_artifact_name(marker)?;
        self.alive()?;
        let _creation = self.creation_hold(LockMode::Shared)?;
        self.alive()?;
        let reservation = self.reservation()?.ok_or(CoordError::NotCreated)?;
        if reservation.implementation != IMPLEMENTATION {
            return Err(CoordError::ImplementationMismatch {
                recorded: reservation.implementation,
            });
        }
        let actual = self
            .inner
            .vfs
            .read_dir(&self.inner.root)?
            .into_iter()
            .find(|entry| names_equal(&entry.file_name, marker))
            .filter(|entry| entry.file_type == crate::FileType::File)
            .ok_or(CoordError::NotCreated)?;
        self.inner
            .vfs
            .fsync(&self.inner.root.join(actual.file_name))?;
        self.inner.vfs.fsync(&self.inner.root)?;
        Ok(())
    }
}
