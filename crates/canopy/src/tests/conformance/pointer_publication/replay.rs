// Copyright (c) Michael Grier.

//! Driving the store through a trace and judging every change it makes.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use acadia::{FaultOp, MemFs, Vfs};
use serde_json::Value;

use super::super::corpus::Trace;
use super::super::recorder::{Recorder, Runs, Verdict, Watcher, mutates};
use super::class::{Class, begins, classify, first_call};
use super::projection::{Projection, World, model, observe, observe_live};
use crate::tests::{test_agent, test_config};
use crate::{Nonce, Store, StoreError, StoreOptions, Swap, SwapSet};
/// How an operation of the trace ends.
#[derive(Clone, Copy, Debug)]
enum Ending {
    /// It finishes, failed or not.
    Finish,
    /// The process dies at the first call after the first `after` changes.
    Crash { after: usize },
    /// The trace ends inside it, after the first `after` changes.
    Cut { after: usize },
}

/// A failure the trace's step `step` injects: the call that begins the step the model was about
/// to take, once the first `after` changes are made. A transaction write that fails before its
/// prepare record goes on to roll back, so one operation can meet more than one.
#[derive(Clone, Copy, Debug)]
struct Injected {
    first: Class,
    after: usize,
    step: usize,
}

struct Progress {
    runs: Runs<Class, Projection>,
    expected: Vec<Projection>,
    faults: Vec<Injected>,
    ending: Ending,
    /// How many of the faults have been injected.
    injected: usize,
    /// Whether the process has died.
    dead: bool,
    /// The first change that is not the model's, by its index among the changes.
    mismatch: Option<(usize, String)>,
}

impl Progress {
    fn check(&mut self, index: usize) {
        if self.mismatch.is_some() {
            return;
        }
        let (class, observed) = &self.runs.changes()[index];
        let detail = match self.expected.get(index) {
            Some(expected) if expected == observed => return,
            Some(expected) => {
                format!("{class:?} left {observed:?}, where the model has {expected:?}")
            }
            None => format!("{class:?} left {observed:?}, where the model changes nothing more"),
        };
        self.mismatch = Some((index, detail));
    }
}

/// Watches one operation's calls and judges each change it makes.
struct Tracker {
    world: Arc<World>,
    mem: MemFs,
    progress: Mutex<Progress>,
}

impl Tracker {
    fn progress(&self) -> std::sync::MutexGuard<'_, Progress> {
        self.progress.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Watcher for Tracker {
    fn before(&self, op: FaultOp, path: &Path, to: Option<&Path>) -> Verdict {
        let class = classify(&self.world, op, path, to);
        let mut progress = self.progress();
        if let Some(index) = progress.runs.begin(class) {
            progress.check(index);
        }
        if progress.mismatch.is_some() {
            return Verdict::Terminate;
        }
        if progress.dead {
            return Verdict::Proceed;
        }
        let made = progress.runs.changes().len();
        if let Some(fault) = progress.faults.get(progress.injected)
            && made == fault.after
            && begins(fault.first, class)
        {
            progress.injected += 1;
            return Verdict::Fail;
        }
        match progress.ending {
            Ending::Crash { after } | Ending::Cut { after }
                if made == after && progress.injected == progress.faults.len() =>
            {
                progress.dead = true;
                Verdict::Terminate
            }
            _ => Verdict::Proceed,
        }
    }

    fn after(&self, op: FaultOp, _path: &Path, _to: Option<&Path>) {
        if !mutates(op) {
            return;
        }
        let mut progress = self.progress();
        // Only a sync changes what a crash would leave, so another call reads the live state alone.
        let state = if matches!(op, FaultOp::Fsync | FaultOp::PublishNew) {
            observe(&self.world, &self.mem)
        } else {
            observe_live(&self.world, &self.mem, progress.runs.current())
        };
        progress.runs.observe(state);
    }
}

/// Where the store left the trace: somewhere in the steps `from` through `to`.
///
/// A step that changes nothing the projection shows -- a settle that finds no record, a read --
/// cannot be seen to happen, so when the store's change is not the trace's next one, all that is
/// known is that it left after the last change both made and no later than the step whose change
/// it did not make.
pub(super) struct Departure {
    pub(super) from: usize,
    pub(super) to: usize,
    pub(super) detail: String,
}

struct Replay<'t> {
    trace: &'t Trace,
    world: Arc<World>,
    recorder: Arc<Recorder>,
    store: Option<Store>,
    nonces: Arc<AtomicU64>,
}

impl<'t> Replay<'t> {
    fn vfs(&self) -> Arc<dyn Vfs> {
        Arc::clone(&self.recorder) as Arc<dyn Vfs>
    }

    fn options(&self) -> StoreOptions {
        let nonces = Arc::clone(&self.nonces);
        StoreOptions::with_nonce_source(test_agent(), move || {
            let n = nonces.fetch_add(1, Ordering::Relaxed) + 1;
            Nonce::new(&format!("S{n}")).expect("a valid nonce")
        })
    }

    fn nonce(&self, kind: &str) -> Nonce {
        let n = self.nonces.fetch_add(1, Ordering::Relaxed) + 1;
        Nonce::new(&format!("{kind}{n}")).expect("a valid nonce")
    }

    fn depart(&self, step: usize, detail: String) -> Departure {
        Departure {
            from: step,
            to: step,
            detail,
        }
    }

    fn task(&self, state: usize) -> &str {
        self.trace.states[state]["task"]
            .as_str()
            .expect("task is a string")
    }

    /// Sets up the model's initial state: the initial version, durable everywhere, written by a
    /// transaction whose ended record is still in the log.
    fn start(trace: &'t Trace) -> Result<Replay<'t>, Departure> {
        let world = Arc::new(World::of(trace));
        let recorder = Arc::new(Recorder::new());
        let replay = Replay {
            trace,
            world: Arc::clone(&world),
            recorder,
            store: None,
            nonces: Arc::new(AtomicU64::new(0)),
        };
        replay
            .vfs()
            .create_dir_all(&world.root)
            .expect("create the store root");
        let config = test_config()
            .with_pointer_replica_count(world.pointers)
            .expect("an odd pointer replica count")
            .with_blob_replica_count(world.blobs)
            .expect("a blob replica count");
        let store = Store::init(replay.vfs(), &world.root, config, replay.options())
            .expect("init the store");
        let swap = Swap::new(
            world.node.clone(),
            world.document.clone(),
            world.body(&world.initial),
        );
        store
            .apply_transaction_as(&SwapSet::single(swap), &replay.nonce("T"))
            .expect("write the initial version");
        let mut replay = replay;
        replay.store = Some(store);
        let held = observe(&world, replay.recorder.mem());
        let initial = model(&world, &trace.states[0]);
        if held != initial {
            return Err(replay.depart(
                0,
                format!("initially the store holds {held:?}, where the model has {initial:?}"),
            ));
        }
        Ok(replay)
    }

    /// Takes step `k` and any steps of the operation it starts, returning the next step.
    fn step(&mut self, k: usize) -> Result<usize, Departure> {
        let held = observe(&self.world, self.recorder.mem());
        let expected = model(&self.world, &self.trace.states[k]);
        if held != expected {
            return Err(self.depart(
                k,
                format!("before it the store holds {held:?}, where the model has {expected:?}"),
            ));
        }
        let step = &self.trace.steps[k];
        match step.name.as_str() {
            "StartWrite" | "StartTx" | "StartOp" | "Recover" | "Open" => self.operation(k),
            "Crash" => {
                self.crash();
                self.settled(k)
            }
            "Close" => {
                drop(self.store.take());
                self.settled(k)
            }
            "Damage" => {
                let replica = step.args[0].as_u64().expect("Damage names a replica");
                let entry = self
                    .world
                    .entry(usize::try_from(replica).expect("a replica") - 1);
                let mem = self.recorder.mem();
                mem.remove_file(&entry).expect("remove the replica");
                mem.fsync(entry.parent().expect("an entry has a parent"))
                    .expect("sync its directory");
                self.settled(k)
            }
            // The reset every quiescence path ends in. `quiesce_commit_log` settles the record
            // first when it is the outermost operation, which the model's `Quiesce` does not.
            "Quiesce" => match self.store.as_ref().map(|store| store.commit_log().reset()) {
                Some(Ok(())) => self.settled(k),
                Some(Err(error)) => Err(self.depart(k, format!("quiescing failed: {error}"))),
                None => Err(self.depart(k, "no store is open".to_owned())),
            },
            other => Err(self.depart(k, format!("{other} is no action this bridge drives"))),
        }
    }

    /// After a step the store takes without an operation of its own, it must hold the model's state.
    fn settled(&self, k: usize) -> Result<usize, Departure> {
        let held = observe(&self.world, self.recorder.mem());
        let expected = model(&self.world, &self.trace.states[k + 1]);
        if held == expected {
            Ok(k + 1)
        } else {
            Err(self.depart(
                k,
                format!("after it the store holds {held:?}, where the model has {expected:?}"),
            ))
        }
    }

    /// Process death and power loss: every call fails from here, the handle goes, and what was
    /// not durable is lost.
    fn crash(&mut self) {
        self.recorder.kill();
        drop(self.store.take());
        self.recorder.revive();
        self.recorder.mem().abandon();
        let restarted = self.recorder.mem().simulate_restart();
        self.recorder = Arc::new(Recorder::over(restarted));
    }

    /// Runs the operation step `k` starts, through the step at which its task ends.
    fn operation(&mut self, k: usize) -> Result<usize, Departure> {
        let trace = self.trace;
        let last = trace.steps.len() - 1;
        let end = (k..=last)
            .find(|&j| self.task(j + 1) == "none")
            .unwrap_or(last);
        let cut = self.task(end + 1) != "none";
        let crashes = !cut && trace.steps[end].name == "Crash";
        let through = if crashes { end } else { end + 1 };

        let start = model(&self.world, &trace.states[k]);
        let mut expected = Vec::new();
        let mut taken_at = Vec::new();
        let mut faults = Vec::new();
        let mut previous = start.clone();
        for j in k..through {
            if trace.steps[j].name == "Fail" {
                let state = &trace.states[j];
                let phase = state["phase"].as_str().expect("phase is a string");
                let idx = state["idx"].as_u64().expect("idx is a number");
                faults.push(Injected {
                    first: first_call(phase, usize::try_from(idx).expect("an index")),
                    after: expected.len(),
                    step: j,
                });
            }
            let next = model(&self.world, &trace.states[j + 1]);
            if next != previous {
                expected.push(next.clone());
                taken_at.push(j);
                previous = next;
            }
        }
        let after = expected.len();
        let ending = if cut {
            Ending::Cut { after }
        } else if crashes {
            Ending::Crash { after }
        } else {
            Ending::Finish
        };
        let failing = !faults.is_empty();

        let tracker = Arc::new(Tracker {
            world: Arc::clone(&self.world),
            mem: self.recorder.mem().another_process(),
            progress: Mutex::new(Progress {
                runs: Runs::new(start),
                expected,
                faults,
                ending,
                injected: 0,
                dead: false,
                mismatch: None,
            }),
        });
        self.recorder
            .watch(Arc::clone(&tracker) as Arc<dyn Watcher>);
        let result = self.perform(k);
        self.recorder.unwatch();

        let mut progress = tracker.progress();
        if let Some(index) = progress.runs.end() {
            progress.check(index);
        }
        // The steps between the last change both made and the model's change at `index`.
        let between = |index: usize, detail: String| Departure {
            from: index.checked_sub(1).map_or(k, |last| taken_at[last] + 1),
            to: taken_at.get(index).copied().unwrap_or(end),
            detail,
        };
        if let Some((index, detail)) = progress.mismatch.take() {
            return Err(between(index, detail));
        }
        let made = progress.runs.changes().len();
        let missing = |_: &Self, progress: &Progress| {
            between(
                made,
                format!(
                    "the store made no further change, where the model has {:?}",
                    progress.expected[made]
                ),
            )
        };
        if made < after {
            return Err(missing(self, &progress));
        }
        if let Some(fault) = progress.faults.get(progress.injected) {
            return Err(self.depart(
                fault.step,
                format!(
                    "the store never made the {:?} call the model fails",
                    fault.first
                ),
            ));
        }
        match ending {
            Ending::Finish => match (failing, result) {
                (false, Err(error)) => {
                    return Err(self.depart(end, format!("the operation failed: {error}")));
                }
                (true, Ok(())) => {
                    return Err(self.depart(
                        end,
                        "the operation succeeded through a failed call".to_owned(),
                    ));
                }
                _ => {}
            },
            Ending::Crash { .. } => {
                drop(progress);
                self.crash();
                return self.settled(end);
            }
            Ending::Cut { .. } => return Ok(trace.steps.len()),
        }
        drop(progress);
        if trace.states[end + 1]["mode"].as_str() == Some("down") {
            drop(self.store.take());
        }
        self.settled(end)
    }

    /// The store's operation for the model's step `k`.
    fn perform(&mut self, k: usize) -> Result<(), StoreError> {
        let step = &self.trace.steps[k];
        let arg = || {
            step.args
                .first()
                .and_then(Value::as_str)
                .expect("a string argument")
        };
        if step.name == "Open" {
            let (store, _) = Store::open(self.vfs(), &self.world.root, self.options())?;
            self.store = Some(store);
            if self.task(k + 1) == "none" {
                return Ok(());
            }
            return self.store().recover().map(drop);
        }
        let world = Arc::clone(&self.world);
        let store = self.store();
        match step.name.as_str() {
            "StartWrite" => store.write_document_replicated_as(
                world.node.clone(),
                &world.document,
                &world.body(arg()),
                &self.nonce("W"),
            ),
            "StartTx" => {
                let swap = Swap::new(
                    world.node.clone(),
                    world.document.clone(),
                    world.body(arg()),
                );
                store
                    .apply_transaction_as(&SwapSet::single(swap), &self.nonce("T"))
                    .map(drop)
            }
            "Recover" => store.recover().map(drop),
            "StartOp" => match arg() {
                "check" => {
                    let _writer = store.writable()?;
                    crate::read_once(store, &world.node, &world.document).map(drop)
                }
                "collect" => {
                    let _writer = store.writable()?;
                    store.collect_document_blobs(&world.node, &world.document)
                }
                "repair" => {
                    let _writer = store.writable()?;
                    match store.read_pointer(&world.node, &world.document) {
                        Ok(read) => store
                            .repair_pointer_as(
                                &world.node,
                                &world.document,
                                &read,
                                &self.nonce("R"),
                            )
                            .map(drop),
                        Err(StoreError::PointerUnavailable { .. }) => Ok(()),
                        Err(error) => Err(error),
                    }
                }
                "scrub" => store.scrub_content().map(drop),
                other => panic!("no operation {other}"),
            },
            other => panic!("{other} starts no operation"),
        }
    }

    fn store(&self) -> &Store {
        self.store
            .as_ref()
            .expect("an operation runs on an open store")
    }
}

/// Replays `trace`, returning where the store left it, if it did.
pub(super) fn replay(trace: &Trace) -> Result<(), Departure> {
    let mut replay = Replay::start(trace)?;
    let mut k = 0;
    while k < trace.steps.len() {
        k = replay.step(k)?;
    }
    Ok(())
}
