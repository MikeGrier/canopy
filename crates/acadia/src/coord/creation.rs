// Copyright (c) Michael Grier.

//! [`Creation`]: creation exclusion and the creation reservation (canopy's DESIGN-NOTES.md,
//! "Creating a store root, modelled in `Creation.tla`").

use std::ffi::OsStr;
use std::path::{Component, Path};

use super::RESERVED_PREFIX;
use super::error::{CoordError, CoordResult};
use super::types::{Created, CreationNonce};

/// What a creation makes: the caller's inputs, the creator's nonce, and the artifacts in order.
#[derive(Debug, Clone, Copy)]
pub struct CreationPlan<'a> {
    /// Every creation input, encoded canonically by the caller, so that identical inputs are
    /// identical bytes. The interface records them and compares them and never interprets them.
    pub inputs: &'a [u8],
    /// This creator's nonce, recorded if it publishes the reservation.
    pub nonce: CreationNonce,
    /// The names, directly beneath the root, of what creation makes, in the order it makes them;
    /// the last is the marker. The names are compared as a case-insensitive filesystem compares
    /// them: none begins with [`RESERVED_PREFIX`](super::RESERVED_PREFIX), which names the
    /// coordination's own entries, none names another, and none is a sibling a publication of
    /// another is staged in ([`is_publication_staging_of`](crate::is_publication_staging_of)).
    pub artifacts: &'a [&'a OsStr],
}

/// `name` as a case-insensitive filesystem compares it: a Unicode name by each character's simple
/// uppercase mapping, any other by the uppercase of its ASCII code units.
fn folded(name: &OsStr) -> Vec<u8> {
    match name.to_str() {
        Some(text) => text
            .chars()
            .map(simple_uppercase)
            .collect::<String>()
            .into_bytes(),
        None => name.as_encoded_bytes().to_ascii_uppercase(),
    }
}

/// The simple uppercase mapping of `c` (UnicodeData.txt's field 12). `char::to_uppercase` gives the
/// full mapping, which differs only where it is several characters; of those, only the Greek
/// letters with ypogegrammeni have a simple mapping other than themselves, to the titlecase letter.
fn simple_uppercase(c: char) -> char {
    let mut upper = c.to_uppercase();
    if let (Some(single), None) = (upper.next(), upper.next()) {
        return single;
    }
    match c {
        '\u{1F80}'..='\u{1F87}' | '\u{1F90}'..='\u{1F97}' | '\u{1FA0}'..='\u{1FA7}' => {
            char::from_u32(c as u32 + 8).expect("each maps into its titlecase block")
        }
        '\u{1FB3}' => '\u{1FBC}',
        '\u{1FC3}' => '\u{1FCC}',
        '\u{1FF3}' => '\u{1FFC}',
        _ => c,
    }
}

/// Makes `plan`'s artifacts but the marker through `maker`, in order, each made durable with its
/// directory entry before the next, then publishes the marker's bytes complete or not at all with
/// `publish` and makes its entry durable. `alive` stops it as soon as its participant is
/// abandoned; `publish` must refuse once `abandon` has returned, as no check made before it can.
pub(crate) fn make_artifacts(
    plan: &CreationPlan<'_>,
    maker: &mut dyn ArtifactMaker,
    vfs: &dyn crate::Vfs,
    root: &Path,
    alive: &dyn Fn() -> CoordResult<()>,
    publish: &dyn Fn(&Path, &[u8]) -> CoordResult<()>,
) -> CoordResult<()> {
    let (marker, others) = plan
        .artifacts
        .split_last()
        .expect("a validated plan has artifacts");
    for artifact in others {
        alive()?;
        maker.make(artifact, plan.inputs)?;
        alive()?;
        vfs.fsync(root)?;
    }
    alive()?;
    let bytes = maker.marker(marker, plan.inputs)?;
    publish(&root.join(marker), &bytes)?;
    alive()?;
    vfs.fsync(root)?;
    Ok(())
}

/// Refuses a name that is not one plain name directly beneath the root, or that begins with the
/// reserved prefix in any case.
pub(crate) fn validate_artifact_name(name: &OsStr) -> CoordResult<()> {
    if name.as_encoded_bytes().contains(&0) {
        return Err(CoordError::InvalidPlan {
            why: "an artifact's name contains NUL",
        });
    }
    let mut components = Path::new(name).components();
    if !matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(only)), None) if only == name
    ) {
        return Err(CoordError::InvalidPlan {
            why: "an artifact is one plain name directly beneath the root",
        });
    }
    if folded(name).starts_with(RESERVED_PREFIX.to_ascii_uppercase().as_bytes()) {
        return Err(CoordError::InvalidPlan {
            why: "an artifact's name has the coordination's reserved prefix",
        });
    }
    Ok(())
}

/// Refuses a plan [`Creation::create`] could not make: one with no artifacts, a name
/// [`validate_artifact_name`] refuses, or two names one of which is the other, or a sibling a
/// publication of the other is staged in, as a case-insensitive filesystem compares them.
pub(crate) fn validate_plan(plan: &CreationPlan<'_>) -> CoordResult<()> {
    if plan.artifacts.is_empty() {
        return Err(CoordError::InvalidPlan {
            why: "a creation makes at least its marker",
        });
    }
    let infix = crate::PUBLISHING_INFIX.to_ascii_uppercase();
    let names: Vec<Vec<u8>> = plan.artifacts.iter().map(|name| folded(name)).collect();
    for (i, name) in plan.artifacts.iter().enumerate() {
        validate_artifact_name(name)?;
        for (j, other) in names.iter().enumerate() {
            if i == j {
                continue;
            }
            if names[i] == *other {
                return Err(CoordError::InvalidPlan {
                    why: "an artifact is named twice",
                });
            }
            let staged = names[i]
                .strip_prefix(other.as_slice())
                .and_then(|rest| rest.strip_prefix(infix.as_bytes()))
                .is_some_and(|suffix| !suffix.is_empty());
            if staged {
                return Err(CoordError::InvalidPlan {
                    why: "an artifact's name is one another artifact's publication is staged in",
                });
            }
        }
    }
    Ok(())
}

/// The caller's half of creation: making each artifact but the marker, and giving the marker's
/// bytes, which the coordination publishes itself so that a marker is never found incomplete.
pub trait ArtifactMaker {
    /// Makes the artifact `name` from `inputs`, and makes it and everything in it durable before
    /// returning; the coordination makes its directory entry durable. It is never asked for the
    /// marker.
    ///
    /// It may find the artifact partly made by a creation that was interrupted, and finishes it:
    /// making an artifact is idempotent.
    ///
    /// # Errors
    ///
    /// Any failure ends the creation, which a later call resumes.
    fn make(&mut self, name: &OsStr, inputs: &[u8]) -> CoordResult<()>;

    /// Gives the bytes of the marker `name` for `inputs`, once every other artifact is durable;
    /// whatever must precede the marker is done here. The coordination publishes them with
    /// [`Vfs::publish_new`](crate::Vfs::publish_new), complete or not at all.
    ///
    /// # Errors
    ///
    /// Any failure ends the creation, which a later call resumes.
    fn marker(&mut self, name: &OsStr, inputs: &[u8]) -> CoordResult<Vec<u8>>;
}

/// Creation exclusion and the creation reservation.
///
/// Within one implementation, creating a root excludes every other creator and opener of it until
/// its marker is in place. Between implementations, whose creation exclusions need not exclude each
/// other, the reservation arbitrates: whoever publishes it owns the creation.
pub trait Creation {
    /// Creates the store root, resumes a creation of it that was interrupted, or finds it made.
    ///
    /// The root is read: a marker under a reservation that names this implementation and records
    /// `plan.inputs` is the store asked for, whose marker is made durable with its directory entry
    /// before the call returns [`Created::Existing`]; any other marker refuses with
    /// [`AlreadyCreated`](super::CoordError::AlreadyCreated); anything beneath it that is neither an
    /// artifact of `plan` nor the coordination's own -- or an artifact with no reservation, which no
    /// creation leaves -- refuses with [`Occupied`](super::CoordError::Occupied). A sibling that
    /// [`Vfs::publish_new`](crate::Vfs::publish_new) stages an artifact in
    /// ([`is_publication_staging_of`](crate::is_publication_staging_of)) is read as that artifact,
    /// so a crash during its publication leaves a creation to finish. A reservation that names another implementation or
    /// records other inputs refuses with [`Reserved`](super::CoordError::Reserved). Then this
    /// implementation's creation exclusion is taken, waiting for it, and the root read again under
    /// it with the same decisions, so a creation that finished while this call waited is found.
    ///
    /// With no reservation, one is published -- this implementation, `plan.inputs` and
    /// `plan.nonce` -- complete or not at all even across a power loss, and made durable with its
    /// directory entry before any artifact exists; a reservation published first by another
    /// creator refuses this one. With a reservation this call may resume, it is made durable in
    /// case its creator died first. Each artifact but the marker is then made through `maker`, in
    /// order, and made durable before the next; last, the marker's bytes from `maker` are published
    /// complete or not at all and made durable with their directory entry. The exclusion is
    /// released once the marker is durable.
    ///
    /// # Errors
    ///
    /// [`InvalidPlan`](super::CoordError::InvalidPlan) for a plan with no artifacts, with a name
    /// that is not one plain name or has the reserved prefix, or with names that collide as
    /// [`CreationPlan::artifacts`] describes; the refusals above, which change
    /// nothing in the root; a failure of `maker` or of the filesystem, which leaves a creation a
    /// later call resumes.
    fn create(
        &self,
        plan: &CreationPlan<'_>,
        maker: &mut dyn ArtifactMaker,
    ) -> CoordResult<Created>;

    /// Records this implementation as the one that coordinates a root whose store the caller makes
    /// or changes itself, which [`create`](Creation::create) cannot be given: the reservation alone,
    /// with no artifact.
    ///
    /// It follows `create`'s steps for the reservation. A reservation is read; one naming another
    /// implementation or recording other `inputs` refuses with
    /// [`Reserved`](super::CoordError::Reserved). Then this implementation's creation exclusion is
    /// taken, waiting for it, and the reservation read again under it. With none, one is published --
    /// this implementation, `inputs` and `nonce` -- complete or not at all even across a power loss,
    /// and made durable with its directory entry; a reservation published first by another creator
    /// refuses this call. With one this call may resume, it is made durable in case its creator died
    /// first, and the call returns [`Created::Resumed`]. The exclusion is released on return, so the
    /// caller's own exclusion of the root is what keeps others out while it makes what it makes.
    /// What the root holds is not read: the caller knows what it is reserving.
    ///
    /// # Errors
    ///
    /// [`Reserved`](super::CoordError::Reserved) as above, which changes nothing in the root; a
    /// failure of the filesystem, which a later call resumes.
    fn reserve(&self, inputs: &[u8], nonce: CreationNonce) -> CoordResult<Created>;

    /// Confirms that the root is a store this implementation coordinates, before an open relies on
    /// it: the reservation must name this implementation, and the marker -- the last of the
    /// artifacts, named by `marker` -- must exist, and is made durable with its directory entry,
    /// since its creator may have died between publishing it and making it durable.
    ///
    /// It waits for this implementation's creation exclusion, shared with other openers, so an open
    /// that meets a creation in progress waits for it to finish or die.
    ///
    /// # Errors
    ///
    /// [`ImplementationMismatch`](super::CoordError::ImplementationMismatch) for a root another
    /// implementation coordinates; [`NotCreated`](super::CoordError::NotCreated) for one with no
    /// marker, a creation that was never finished included, or with no reservation, which no
    /// creation through this interface leaves; [`InvalidPlan`](super::CoordError::InvalidPlan) for a
    /// marker name that could not be an artifact.
    fn confirm_created(&self, marker: &OsStr) -> CoordResult<()>;
}
