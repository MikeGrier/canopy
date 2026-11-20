// Copyright (c) Michael Grier.

//! The model's processes: each a thread with a participant of its own, over its own simulated
//! process of the one filesystem, held at the first call of each step until the replay lets it
//! take that step.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use acadia::coord::file::FileFactory;
use acadia::coord::shared::SharedFactory;
use acadia::coord::{
    ArtifactMaker, CoordError, CoordResult, CoordinationFactory, Created, CreationNonce,
    CreationPlan,
};
use acadia::{FaultOp, MemFs, Vfs, VfsErrorKind};

use super::super::recorder::{Recorder, Verdict, Watcher};
use super::projection::{RESERVATION, artifact, root};

/// How long a process may take to reach its next step before the replay gives up on it: far
/// longer than any step takes, so only a process that waits on something never comes back.
const LIMIT: Duration = Duration::from_secs(10);

/// The creation exclusions, one for each implementation.
const CREATION_LOCKS: [&str; 2] = [".coord-creation.lock", ".coord-shared-creation.lock"];

/// A kind of call, as much of it as says which step of the model it begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Class {
    /// Reading the root's names, which begins a look and the reading again under the exclusion.
    LookRoot,
    /// Taking a creation exclusion.
    CreationLock,
    SyncRoot,
    SyncReservation,
    /// Any call on artifact `k`, counting from 1.
    Artifact(usize),
    Other,
}

pub(super) fn classify(op: FaultOp, path: &Path, artifacts: usize) -> Class {
    if path == root() {
        return match op {
            FaultOp::ReadDir => Class::LookRoot,
            FaultOp::Fsync => Class::SyncRoot,
            _ => Class::Other,
        };
    }
    let Some(name) = path.file_name().filter(|_| path.parent() == Some(root())) else {
        return Class::Other;
    };
    if matches!(
        op,
        FaultOp::Lock | FaultOp::TryLock | FaultOp::LockExclusive
    ) && CREATION_LOCKS.iter().any(|lock| name == *lock)
    {
        return Class::CreationLock;
    }
    if op == FaultOp::Fsync && name == RESERVATION {
        return Class::SyncReservation;
    }
    (1..=artifacts)
        .find(|&k| name == artifact(k).as_str())
        .map_or(Class::Other, Class::Artifact)
}

/// What a process is: a creator or an opener, of one implementation, with creation inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Part {
    pub(super) number: u64,
    pub(super) creator: bool,
    pub(super) implementation: u64,
    pub(super) inputs: u64,
}

#[derive(Default)]
struct Gate {
    /// The class of call the process stops before, or `None` to run to its end.
    stop: Option<Class>,
    /// How many steps the process has been let take.
    grants: u64,
    /// The grant under which the process stopped, while it is stopped.
    stopped: Option<u64>,
    /// Every call the process makes from here fails.
    killed: bool,
    /// How its attempt ended, once it has.
    ended: Option<String>,
}

/// Holds a process before the first call of the class its next step begins with.
struct Turnstile {
    gate: Mutex<Gate>,
    moved: Condvar,
    artifacts: usize,
}

impl Turnstile {
    fn gate(&self) -> MutexGuard<'_, Gate> {
        self.gate.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn end(&self, outcome: String) {
        self.gate().ended = Some(outcome);
        self.moved.notify_all();
    }
}

impl Watcher for Turnstile {
    fn before(&self, op: FaultOp, path: &Path, _to: Option<&Path>) -> Verdict {
        let class = classify(op, path, self.artifacts);
        let mut gate = self.gate();
        if !gate.killed && gate.stop == Some(class) {
            let grant = gate.grants;
            gate.stopped = Some(grant);
            self.moved.notify_all();
            gate = self
                .moved
                .wait_while(gate, |gate| gate.grants == grant && !gate.killed)
                .unwrap_or_else(PoisonError::into_inner);
            gate.stopped = None;
        }
        if gate.killed {
            Verdict::Terminate
        } else {
            Verdict::Proceed
        }
    }

    fn after(&self, _op: FaultOp, _path: &Path, _to: Option<&Path>) {}
}

/// One of the model's processes, running.
pub(super) struct Process {
    recorder: Arc<Recorder>,
    turnstile: Arc<Turnstile>,
    thread: Option<JoinHandle<()>>,
}

impl Process {
    /// Starts `part` over its own process of `mem`, and waits for it to stop before its first
    /// call of class `first`.
    pub(super) fn start(
        mem: &MemFs,
        part: &Part,
        artifacts: usize,
        first: Class,
    ) -> Result<Process, String> {
        let recorder = Arc::new(Recorder::over(mem.another_process()));
        let turnstile = Arc::new(Turnstile {
            gate: Mutex::new(Gate {
                stop: Some(first),
                ..Gate::default()
            }),
            moved: Condvar::new(),
            artifacts,
        });
        recorder.watch(turnstile.clone());
        let vfs: Arc<dyn Vfs> = recorder.clone();
        let ending = Arc::clone(&turnstile);
        let part = part.clone();
        let thread = std::thread::spawn(move || {
            let outcome = catch_unwind(AssertUnwindSafe(|| attempt(&part, vfs, artifacts)))
                .unwrap_or_else(|_| "a panic".to_owned());
            ending.end(outcome);
        });
        let process = Process {
            recorder,
            turnstile,
            thread: Some(thread),
        };
        process.settle()?;
        match process.ended() {
            Some(outcome) => Err(format!("it ended {outcome} before its first step")),
            None => Ok(process),
        }
    }

    /// Waits for the process to stop before its next step, or to end.
    fn settle(&self) -> Result<(), String> {
        let gate = self.turnstile.gate();
        let grant = gate.grants;
        let (_gate, waited) = self
            .turnstile
            .moved
            .wait_timeout_while(gate, LIMIT, |gate| {
                gate.stopped != Some(grant) && gate.ended.is_none()
            })
            .unwrap_or_else(PoisonError::into_inner);
        if waited.timed_out() {
            return Err("it neither reached its next step nor ended".to_owned());
        }
        Ok(())
    }

    /// Lets the process take the step it is stopped before, up to the first call of class
    /// `next` or to its end.
    pub(super) fn step(&self, next: Option<Class>) -> Result<(), String> {
        {
            let mut gate = self.turnstile.gate();
            gate.stop = next;
            gate.grants += 1;
        }
        self.turnstile.moved.notify_all();
        self.settle()
    }

    /// How the process's attempt ended, once it has.
    pub(super) fn ended(&self) -> Option<String> {
        self.turnstile.gate().ended.clone()
    }

    /// Kills the process: its locks are released at once, as the operating system releases a
    /// dead process's, and every call it makes from here fails.
    pub(super) fn condemn(&self) {
        self.recorder.kill();
        self.recorder.mem().abandon();
        self.turnstile.gate().killed = true;
        self.turnstile.moved.notify_all();
    }

    /// Waits for the process's thread to finish.
    pub(super) fn join(&mut self) {
        if let Some(thread) = self.thread.take() {
            thread.join().expect("an attempt catches its panics");
        }
    }
}

/// Makes an artifact from the inputs, keeping one a creation before made, and makes its bytes
/// durable.
struct Maker {
    vfs: Arc<dyn Vfs>,
}

impl ArtifactMaker for Maker {
    fn make(&mut self, name: &str, inputs: &[u8]) -> CoordResult<()> {
        let path = root().join(name);
        match self.vfs.metadata(&path) {
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == VfsErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        self.vfs.write(&path, inputs)?;
        self.vfs.fsync(&path)?;
        Ok(())
    }

    fn marker(&mut self, _: &str, inputs: &[u8]) -> CoordResult<Vec<u8>> {
        Ok(inputs.to_vec())
    }
}

/// A process's whole attempt: a creation for a creator, an open for an opener, and the model's
/// name for how it ended.
fn attempt(part: &Part, vfs: Arc<dyn Vfs>, artifacts: usize) -> String {
    let factory: Box<dyn CoordinationFactory> = match part.implementation {
        1 => Box::new(FileFactory::new()),
        2 => Box::new(SharedFactory::new()),
        other => panic!("the model has no implementation {other}"),
    };
    let participant = match factory.open(Arc::clone(&vfs), root()) {
        Ok(participant) => participant,
        Err(error) => return format!("{error:?} opening a participant"),
    };
    let names: Vec<String> = (1..=artifacts).map(artifact).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    if !part.creator {
        return outcome(
            participant
                .confirm_created(names[artifacts - 1])
                .map(|()| "opened"),
        );
    }
    let inputs = part.inputs.to_string();
    let nonce = u8::try_from(part.number).expect("a process number fits a nonce byte");
    let plan = CreationPlan {
        inputs: inputs.as_bytes(),
        nonce: CreationNonce::from_bytes([nonce; 16]),
        artifacts: &names,
    };
    outcome(
        participant
            .create(&plan, &mut Maker { vfs })
            .map(|created| match created {
                Created::Created => "created",
                Created::Resumed => "resumed",
                Created::Existing => "existing",
            }),
    )
}

/// The model's name for how an attempt ended; a refusal for the reservation is `reserved`, which
/// [`expected`] gives for each of the model's.
fn outcome(result: CoordResult<&'static str>) -> String {
    let name = match result {
        Ok(name) => name,
        Err(CoordError::AlreadyCreated) => "exists",
        Err(CoordError::Occupied { .. }) => "occupied",
        Err(CoordError::Reserved) => "reserved",
        Err(CoordError::Malformed { .. }) => "partial",
        Err(CoordError::NotCreated) => "notAStore",
        Err(CoordError::ImplementationMismatch { .. }) => "mismatch",
        Err(error) => return format!("{error:?}"),
    };
    name.to_owned()
}

/// What the code returns where the model's attempt ends `model`: the code refuses every
/// reservation it may not use alike, where the model names why.
pub(super) fn expected(model: &str) -> &str {
    match model {
        "otherImpl" | "differentInputs" | "lostRace" | "changed" => "reserved",
        other => other,
    }
}
