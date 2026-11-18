// Copyright (c) Michael Grier.

use super::*;
use crate::coord::{CountNotification, ModCount, ModificationCount, Notice};
use std::sync::{Condvar, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

#[cfg(test)]
mod tests;

const FIRST_INTERVAL: Duration = Duration::from_millis(1);
const LAST_INTERVAL: Duration = Duration::from_millis(32);

impl ModificationCount for SharedState {
    fn sample(&self) -> CoordResult<ModCount> {
        self.alive()?;
        Ok(ModCount::new(
            self.words
                .word(Word::Count)
                .fetch_or(format::FALSE, Ordering::AcqRel),
        ))
    }
}

struct Arm {
    id: u64,
    participant: SharedState,
    sampled: ModCount,
    deadline: Instant,
    next: Instant,
    interval: Duration,
    ring: Option<Box<dyn FnOnce() + Send>>,
}

impl Arm {
    fn wakes_at(&self) -> Instant {
        self.deadline.min(self.next)
    }
}

#[derive(Default)]
struct Pending {
    arms: Vec<Arm>,
    next_id: u64,
    running: bool,
    kicked: bool,
}

#[derive(Default)]
struct Poller {
    state: Mutex<Pending>,
    wake: Condvar,
}

fn poller() -> &'static Poller {
    static POLLER: OnceLock<Poller> = OnceLock::new();
    POLLER.get_or_init(Poller::default)
}

impl Poller {
    fn state(&self) -> MutexGuard<'_, Pending> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn register(
        &'static self,
        participant: SharedState,
        sampled: ModCount,
        deadline: Instant,
        ring: Box<dyn FnOnce() + Send>,
    ) -> CoordResult<u64> {
        let mut state = self.state();
        participant.alive()?;
        if !state.running {
            std::thread::Builder::new()
                .name("acadia-shared-notify".into())
                .spawn(move || self.run())
                .map_err(|error| {
                    VfsError::new(
                        VfsErrorKind::Other,
                        "shared_notify_spawn",
                        error.to_string(),
                    )
                })?;
            state.running = true;
        }
        state.next_id = state.next_id.checked_add(1).ok_or(CoordError::Exhausted {
            what: "notification identity",
        })?;
        let id = state.next_id;
        state.arms.push(Arm {
            id,
            participant,
            sampled,
            deadline,
            next: Instant::now(),
            interval: FIRST_INTERVAL,
            ring: Some(ring),
        });
        state.kicked = true;
        self.wake.notify_one();
        Ok(id)
    }

    fn withdraw(&self, id: u64) {
        let withdrawn = {
            let mut state = self.state();
            let removed = state
                .arms
                .iter()
                .position(|arm| arm.id == id)
                .map(|index| state.arms.swap_remove(index));
            state.kicked = true;
            self.wake.notify_one();
            removed
        };
        drop(withdrawn);
    }

    fn run(&self) {
        let mut state = self.state();
        loop {
            state.kicked = false;
            let now = Instant::now();
            let due: Vec<_> = state
                .arms
                .iter()
                .filter(|arm| arm.wakes_at() <= now)
                .map(|arm| {
                    (
                        arm.id,
                        arm.participant.same_participant(),
                        arm.sampled,
                        arm.deadline,
                    )
                })
                .collect();
            if !due.is_empty() {
                drop(state);
                let mut ring_ids = Vec::new();
                let mut quiet_ids = Vec::new();
                let mut samples: Vec<(SharedState, Option<ModCount>)> = Vec::new();
                for (id, participant, sampled, deadline) in due {
                    if Instant::now() >= deadline {
                        ring_ids.push(id);
                        continue;
                    }
                    let current = if let Some((_, current)) = samples
                        .iter()
                        .find(|(seen, _)| Arc::ptr_eq(&seen.holds, &participant.holds))
                    {
                        *current
                    } else {
                        let current = participant.sample().ok();
                        samples.push((participant, current));
                        current
                    };
                    if current == Some(sampled) {
                        quiet_ids.push(id);
                    } else {
                        ring_ids.push(id);
                    }
                }
                // Sets, so the pass under the lock below costs one lookup per arm.
                let ring_ids: std::collections::HashSet<u64> = ring_ids.into_iter().collect();
                let quiet_ids: std::collections::HashSet<u64> = quiet_ids.into_iter().collect();
                state = self.state();
                let mut rings = Vec::new();
                let mut retired = Vec::new();
                let mut index = 0;
                while index < state.arms.len() {
                    let arm = &mut state.arms[index];
                    if ring_ids.contains(&arm.id) {
                        rings.extend(arm.ring.take());
                        retired.push(state.arms.swap_remove(index));
                    } else {
                        if quiet_ids.contains(&arm.id) {
                            arm.next = Instant::now() + arm.interval;
                            arm.interval = (arm.interval * 2).min(LAST_INTERVAL);
                        }
                        index += 1;
                    }
                }
                drop(state);
                drop(retired);
                for ring in rings {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(ring));
                }
                drop(samples);
                state = self.state();
                continue;
            }
            match state.arms.iter().map(Arm::wakes_at).min() {
                None => {
                    state = self
                        .wake
                        .wait_while(state, |state| !state.kicked)
                        .unwrap_or_else(PoisonError::into_inner)
                }
                Some(until) => {
                    state = self
                        .wake
                        .wait_timeout_while(state, until.saturating_duration_since(now), |state| {
                            !state.kicked
                        })
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
            }
        }
    }
}

#[derive(Debug)]
struct SharedNotice {
    id: u64,
}

impl Notice for SharedNotice {}
impl Drop for SharedNotice {
    fn drop(&mut self) {
        poller().withdraw(self.id);
    }
}

impl CountNotification for SharedState {
    fn notify(
        &self,
        sampled: ModCount,
        deadline: Instant,
        ring: Box<dyn FnOnce() + Send>,
    ) -> CoordResult<Box<dyn Notice>> {
        self.alive()?;
        Ok(Box::new(SharedNotice {
            id: poller().register(self.same_participant(), sampled, deadline, ring)?,
        }))
    }
}
