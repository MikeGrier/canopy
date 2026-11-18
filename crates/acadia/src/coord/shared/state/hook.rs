// Copyright (c) Michael Grier.

//! Test support: a hook each step that changes the shared words announces itself to, and a
//! reading of the words that takes no lock and changes nothing.

use super::*;

#[cfg(test)]
mod tests;

/// A step that changes the shared words, named for the step of the coordination model it begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SharedStep {
    /// Setting the fence on acquiring, for a holder found dead or a count found odd.
    SetFence,
    /// Replacing the ownership record and advancing the generation.
    Record,
    /// Reading the count before the opening raise.
    BeginRaise,
    /// Publishing the opening raise.
    PublishRaise,
    /// Reading the count before the closing raise.
    BeginClose,
    /// Publishing the closing raise.
    PublishClose,
    /// Setting the fence for an operation that ended unresolved.
    Unresolved,
    /// Publishing the recovered count and trusting the state.
    PublishRecovered,
    /// Clearing the fence at the end of recovery.
    ClearFence,
    /// Clearing the ownership record on a clean release.
    ClearRecord,
}

/// Told of each step before it changes the shared words.
///
/// A hook may hold the step for as long as it likes, and may panic to stop it before it acts,
/// which a test uses to stop a process there as its death would.
pub trait StepHook: Send + Sync {
    fn before(&self, step: SharedStep);
}

/// The shared words as one reading of them finds them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedWords {
    pub trust: Trust,
    pub fence: bool,
    /// The generation the holder's acquisition advanced to, or 0 when no holder is recorded.
    pub owner: u64,
    pub generation: u64,
    pub count: u64,
}

/// The shared words of the root `root` of `vfs`, read without taking any lock or changing
/// anything, or `None` while no participant of this boot has made them.
pub fn inspect(vfs: &dyn Vfs, root: &Path) -> CoordResult<Option<SharedWords>> {
    let directory = root.join(DIRECTORY);
    let current_boot = boot(vfs)?;
    let recorded_boot = read_optional(vfs, &directory.join("boot"))?;
    let read: Box<dyn Fn(Word) -> u64> = if vfs.is_os_filesystem() {
        let Some(bytes) = read_optional(vfs, &directory.join("cells"))? else {
            return Ok(None);
        };
        if bytes.len() != BYTE_COUNT {
            return Ok(None);
        }
        Box::new(move |word| {
            let at = word as usize * size_of::<u64>();
            u64::from_le_bytes(
                bytes[at..at + size_of::<u64>()]
                    .try_into()
                    .expect("one word"),
            )
        })
    } else {
        let Some(instance) = read_optional(vfs, &directory.join("instance"))?
            .as_ref()
            .and_then(|bytes| bytes.strip_prefix(&format::INSTANCE))
            .and_then(|bytes| <[u8; 8]>::try_from(bytes).ok())
            .map(u64::from_le_bytes)
        else {
            return Ok(None);
        };
        let key = (
            instance,
            current_boot.clone().unwrap_or_default(),
            model_root(&directory),
        );
        // Test support's weak model cache is process-wide; a failed inspector cannot end a model.
        let models = MODELS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(words) = models.get(&key).and_then(Weak::upgrade) else {
            return Ok(None);
        };
        Box::new(move |word| words.word(word).load(Ordering::Acquire))
    };
    let same_boot = match (&current_boot, &recorded_boot) {
        (Some(current), Some(recorded)) => {
            recorded.strip_prefix(&format::BOOT) == Some(current.as_slice())
        }
        _ => false,
    };
    let trust = if current_boot.is_none() || read(Word::Ready) != format::READY {
        Trust::Untrusted(Untrusted::CannotTell)
    } else if !same_boot {
        Trust::Untrusted(Untrusted::HostRestarted)
    } else if read(Word::Vouched) == format::TRUE {
        Trust::Vouched
    } else {
        Trust::Untrusted(match read(Word::Reason) {
            format::CREATED => Untrusted::Created,
            format::RESTARTED => Untrusted::HostRestarted,
            _ => Untrusted::CannotTell,
        })
    };
    Ok(Some(SharedWords {
        trust,
        fence: read(Word::Fence) != format::FALSE,
        owner: read(Word::Owner),
        generation: read(Word::Generation),
        count: read(Word::Count),
    }))
}
