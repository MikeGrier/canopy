// Copyright (c) Michael Grier.

//! [`CountNotification`] for the file-based implementation: one poller per process samples the
//! count for every outstanding registration, on every root, and runs a registration's ring when
//! its count differs from the value it sampled or its deadline passes.
//!
//! Each registration is sampled with a backoff of its own -- at once when it arrives, then after
//! [`FIRST_INTERVAL`], doubling to [`LAST_INTERVAL`] -- so a new registration does not make the
//! others sample faster, and the poller waits without sampling while none is outstanding.
//! Registrations through one participant that are due together are sampled once. A ring runs on
//! the poller's thread, outside its lock; one that panics is contained, so the poller survives it.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crate::coord::{CoordResult, CountNotification, ModCount, ModificationCount, Notice};
use crate::{VfsError, VfsErrorKind};

use super::FileCoordination;

/// The interval after a registration's first sample.
const FIRST_INTERVAL: Duration = Duration::from_millis(1);
/// The longest interval: a write holds the count odd for tens of milliseconds, so a longer one
/// mostly adds latency, and a shorter one mostly adds reads.
const LAST_INTERVAL: Duration = Duration::from_millis(32);

struct Registration {
    id: u64,
    participant: FileCoordination,
    sampled: ModCount,
    deadline: Instant,
    /// Taken when the registration rings.
    ring: Option<Box<dyn FnOnce() + Send>>,
    /// When it is next sampled.
    next: Instant,
    interval: Duration,
}

impl Registration {
    fn wakes_at(&self) -> Instant {
        self.next.min(self.deadline)
    }
}

#[derive(Default)]
struct State {
    next_id: u64,
    registrations: Vec<Registration>,
    /// Whether the poller thread has been started; it runs for the life of the process.
    running: bool,
    /// Set by a registration so the poller samples it at once rather than finish its wait.
    kicked: bool,
}

#[derive(Default)]
struct Poller {
    state: Mutex<State>,
    wake: Condvar,
}

/// This process's poller.
fn poller() -> &'static Poller {
    static POLLER: OnceLock<Poller> = OnceLock::new();
    POLLER.get_or_init(Poller::default)
}

/// What one round samples of a registration that is due.
struct Due {
    id: u64,
    participant: FileCoordination,
    sampled: ModCount,
    deadline: Instant,
}

impl Poller {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn register(
        &'static self,
        participant: FileCoordination,
        sampled: ModCount,
        deadline: Instant,
        ring: Box<dyn FnOnce() + Send>,
    ) -> CoordResult<u64> {
        let mut state = self.state();
        if !state.running {
            std::thread::Builder::new()
                .name("acadia-coord-poller".into())
                .spawn(move || self.run())
                .map_err(|error| VfsError::new(VfsErrorKind::Other, "spawn", error.to_string()))?;
            state.running = true;
        }
        state.next_id += 1;
        let id = state.next_id;
        state.registrations.push(Registration {
            id,
            participant,
            sampled,
            deadline,
            ring: Some(ring),
            next: Instant::now(),
            interval: FIRST_INTERVAL,
        });
        state.kicked = true;
        self.wake.notify_one();
        Ok(id)
    }

    fn withdraw(&self, id: u64) {
        let withdrawn = {
            let mut state = self.state();
            let position = state.registrations.iter().position(|r| r.id == id);
            position.map(|position| state.registrations.swap_remove(position))
        };
        drop(withdrawn);
    }

    fn run(&self) {
        let mut state = self.state();
        loop {
            state.kicked = false;
            let now = Instant::now();
            let due: Vec<_> = state
                .registrations
                .iter()
                .filter(|r| r.wakes_at() <= now)
                .map(|r| Due {
                    id: r.id,
                    participant: r.participant.same_participant(),
                    sampled: r.sampled,
                    deadline: r.deadline,
                })
                .collect();
            if !due.is_empty() {
                drop(state);
                let (ringing, quiet) = sample(due);
                state = self.state();
                let now = Instant::now();
                let mut rings = Vec::new();
                state.registrations.retain_mut(|r| {
                    if ringing.contains(&r.id) {
                        rings.extend(r.ring.take());
                        return false;
                    }
                    if quiet.contains(&r.id) {
                        r.next = now + r.interval;
                        r.interval = (r.interval * 2).min(LAST_INTERVAL);
                    }
                    true
                });
                if !rings.is_empty() {
                    drop(state);
                    for ring in rings {
                        let _ = catch_unwind(AssertUnwindSafe(ring));
                    }
                    state = self.state();
                }
                continue;
            }
            match state.registrations.iter().map(Registration::wakes_at).min() {
                None => {
                    state = self
                        .wake
                        .wait_while(state, |state| !state.kicked)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                Some(until) => {
                    state = self
                        .wake
                        .wait_timeout_while(state, until.saturating_duration_since(now), |state| {
                            !state.kicked
                        })
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
            }
        }
    }
}

/// Samples the registrations that are due, returning those to ring -- their deadline has passed,
/// or their participant's count differs from what they sampled or cannot be sampled, so that the
/// caller samples again and meets the failure itself -- and those that stay quiet.
fn sample(due: Vec<Due>) -> (Vec<u64>, Vec<u64>) {
    let now = Instant::now();
    let mut samples: Vec<(FileCoordination, Option<ModCount>)> = Vec::new();
    let mut ringing = Vec::new();
    let mut quiet = Vec::new();
    for registration in due {
        if registration.deadline <= now {
            ringing.push(registration.id);
            continue;
        }
        let participant = registration.participant;
        let current = match samples
            .iter()
            .find(|(seen, _)| Arc::ptr_eq(&seen.holds, &participant.holds))
        {
            Some((_, current)) => *current,
            None => {
                let current = participant.sample().ok();
                samples.push((participant, current));
                current
            }
        };
        if current == Some(registration.sampled) {
            quiet.push(registration.id);
        } else {
            ringing.push(registration.id);
        }
    }
    (ringing, quiet)
}

/// A registration with this process's poller, withdrawn when dropped.
#[derive(Debug)]
struct FileNotice {
    id: u64,
}

impl Notice for FileNotice {}

impl Drop for FileNotice {
    fn drop(&mut self) {
        poller().withdraw(self.id);
    }
}

impl CountNotification for FileCoordination {
    fn notify(
        &self,
        sampled: ModCount,
        deadline: Instant,
        ring: Box<dyn FnOnce() + Send>,
    ) -> CoordResult<Box<dyn Notice>> {
        self.alive()?;
        let id = poller().register(self.same_participant(), sampled, deadline, ring)?;
        Ok(Box::new(FileNotice { id }))
    }
}
