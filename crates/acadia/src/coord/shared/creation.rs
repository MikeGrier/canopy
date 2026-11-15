// Copyright (c) Michael Grier.

use super::*;
use crate::VfsErrorKind;
use crate::coord::{
    ArtifactMaker, Created, Creation, CreationNonce, CreationPlan, RESERVED_PREFIX,
};
use std::ffi::{OsStr, OsString};
use std::path::Component;

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
    Marker,
    Occupied(OsString),
}

struct CreationHold {
    inner: Arc<ParticipantInner>,
    id: u64,
}

impl Drop for CreationHold {
    fn drop(&mut self) {
        self.inner
            .creation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.id);
    }
}

fn reserved(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.starts_with(RESERVED_PREFIX))
}

fn validate_name(name: &OsStr) -> CoordResult<()> {
    let mut components = Path::new(name).components();
    if !matches!((components.next(), components.next()), (Some(Component::Normal(component)), None) if component == name)
        || reserved(name)
    {
        return Err(CoordError::InvalidPlan {
            why: "an artifact is one plain non-reserved root name",
        });
    }
    Ok(())
}

impl SharedParticipant {
    fn creation_hold(&self, mode: LockMode) -> CoordResult<CreationHold> {
        self.alive()?;
        let lock =
            self.inner
                .vfs
                .lock(&self.inner.root.join(format::LOCK), mode, LockSpan::File)?;
        let mut owned = self
            .inner
            .creation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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

    fn read_root(&self, plan: &CreationPlan<'_>) -> CoordResult<Found> {
        let mut names: Vec<_> = self
            .inner
            .vfs
            .read_dir(&self.inner.root)?
            .into_iter()
            .map(|entry| entry.file_name)
            .collect();
        names.sort();
        if names
            .iter()
            .any(|name| name == plan.artifacts.last().expect("validated plan"))
        {
            return Ok(Found::Marker);
        }
        let mut artifact = None;
        for name in names {
            if reserved(&name) {
                continue;
            }
            if !plan.artifacts.contains(&name.as_os_str()) {
                return Ok(Found::Occupied(name));
            }
            artifact.get_or_insert(name);
        }
        match (self.reservation_course(plan.inputs)?, artifact) {
            (Found::Empty, Some(name)) => Ok(Found::Occupied(name)),
            (course, _) => Ok(course),
        }
    }

    fn creation_course(found: Found) -> CoordResult<Found> {
        match found {
            Found::Marker => Err(CoordError::AlreadyCreated),
            Found::Occupied(name) => Err(CoordError::Occupied { name }),
            course => Ok(course),
        }
    }

    fn settle(&self, course: Found, inputs: &[u8], nonce: CreationNonce) -> CoordResult<Created> {
        self.alive()?;
        let path = self.inner.root.join(format::RESERVATION);
        let result = match course {
            Found::Empty => {
                let reservation = Reservation {
                    implementation: IMPLEMENTATION.clone(),
                    nonce,
                    inputs: inputs.to_vec(),
                };
                match self.inner.vfs.publish_new(&path, &reservation.encode()) {
                    Err(error) if error.kind() == VfsErrorKind::AlreadyExists => {
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
        if plan.artifacts.is_empty() {
            return Err(CoordError::InvalidPlan {
                why: "a creation includes its marker",
            });
        }
        for (index, name) in plan.artifacts.iter().enumerate() {
            validate_name(name)?;
            if plan.artifacts[..index].contains(name) {
                return Err(CoordError::InvalidPlan {
                    why: "an artifact is named twice",
                });
            }
        }
        let first = Self::creation_course(self.read_root(plan)?)?;
        let _creation = self.creation_hold(LockMode::Exclusive)?;
        self.alive()?;
        let course = Self::creation_course(self.read_root(plan)?)?;
        if let (Found::Reserved(before), Found::Reserved(now)) = (&first, &course)
            && before != now
        {
            return Err(CoordError::Reserved);
        }
        let result = self.settle(course, plan.inputs, plan.nonce)?;
        for artifact in plan.artifacts {
            self.alive()?;
            maker.make(artifact, plan.inputs)?;
            self.alive()?;
            self.inner.vfs.fsync(&self.inner.root)?;
        }
        Ok(result)
    }

    fn reserve(&self, inputs: &[u8], nonce: CreationNonce) -> CoordResult<Created> {
        self.alive()?;
        let first = self.reservation_course(inputs)?;
        let _creation = self.creation_hold(LockMode::Exclusive)?;
        self.alive()?;
        let course = self.reservation_course(inputs)?;
        if let (Found::Reserved(before), Found::Reserved(now)) = (&first, &course)
            && before != now
        {
            return Err(CoordError::Reserved);
        }
        self.settle(course, inputs, nonce)
    }

    fn confirm_created(&self, marker: &OsStr) -> CoordResult<()> {
        validate_name(marker)?;
        self.alive()?;
        let _creation = self.creation_hold(LockMode::Shared)?;
        self.alive()?;
        let reservation = self.reservation()?.ok_or(CoordError::NotCreated)?;
        if reservation.implementation != IMPLEMENTATION {
            return Err(CoordError::ImplementationMismatch {
                recorded: reservation.implementation,
            });
        }
        let path = self.inner.root.join(marker);
        if !self.inner.vfs.exists(&path)? {
            return Err(CoordError::NotCreated);
        }
        self.inner.vfs.fsync(&path)?;
        self.inner.vfs.fsync(&self.inner.root)?;
        Ok(())
    }
}
