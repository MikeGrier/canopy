// Copyright (c) Michael Grier.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use acadia::coord::CoordinationFactory;
use acadia::coord::file::FileFactory;
use acadia::coord::shared::{SharedFactory, SharedStep, StepHook};
use acadia::{FaultOp, MemFs};

use super::super::coordination::process as contract;
use super::super::pointer_publication::class as publication;
use super::super::recorder::{Recorder, Verdict, Watcher};
use super::projection::World;
use crate::tests::test_agent;
use crate::{Recovery, RecoveryPhase, Store, StoreOptions, Swap, SwapSet};

pub(super) use contract::Implementation;

const LIMIT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Class {
    Contract(contract::Class),
    Document(publication::Class),
    DirectoryRead,
    DirectorySync,
    DirectoryClear,
    DirectoryClearSync,
    Other,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Command {
    Open,
    Write {
        document: usize,
        version: usize,
        logged: bool,
    },
    BracketedWrite {
        document: usize,
        version: usize,
        logged: bool,
        quiesce_before: bool,
        quiesce_after: bool,
    },
    Acquire,
    Empty,
    Quiesce,
    Recover,
    BeginRecovery,
    RecoveryUnit,
    Close,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Reply {
    Done,
    Recovery {
        phase: RecoveryPhase,
        finished: bool,
    },
    Failed(String),
}

#[derive(Default)]
struct Gate {
    stop: Option<Class>,
    grants: u64,
    stopped: Option<(u64, Class)>,
    atomic_stop: bool,
    fail: bool,
    ended: bool,
    command: Option<Command>,
    busy: bool,
    reply: Option<Reply>,
}

struct Turnstile {
    gate: Mutex<Gate>,
    moved: Condvar,
    world: Arc<World>,
    implementation: Implementation,
}

impl Turnstile {
    fn gate(&self) -> MutexGuard<'_, Gate> {
        self.gate.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn check(&self, class: Class, atomic: bool) -> Verdict {
        let mut gate = self.gate();
        if !gate.ended && gate.stop == Some(class) {
            let grant = gate.grants;
            gate.stopped = Some((grant, class));
            gate.atomic_stop = atomic;
            self.moved.notify_all();
            gate = self
                .moved
                .wait_while(gate, |gate| gate.grants == grant && !gate.ended)
                .unwrap_or_else(PoisonError::into_inner);
            gate.stopped = None;
            if !gate.ended && std::mem::take(&mut gate.fail) {
                return Verdict::Fail;
            }
        }
        if gate.ended {
            Verdict::Die
        } else {
            Verdict::Proceed
        }
    }
}

impl Watcher for Turnstile {
    fn before(&self, op: FaultOp, path: &Path, to: Option<&Path>) -> Verdict {
        if path == self.world.directory_record() {
            let class = match op {
                FaultOp::Read => Class::DirectoryRead,
                FaultOp::Truncate => Class::DirectoryClear,
                FaultOp::Fsync => Class::DirectoryClearSync,
                _ => Class::Other,
            };
            return self.check(class, false);
        }
        if op == FaultOp::Fsync && self.world.directory_target().parent() == Some(path) {
            return self.check(Class::DirectorySync, false);
        }
        let class = match contract::classify(self.implementation, op, path) {
            contract::Class::Other => {
                match publication::classify_root(&self.world.root, op, path, to) {
                    publication::Class::Other => Class::Other,
                    class => Class::Document(class),
                }
            }
            class => Class::Contract(class),
        };
        self.check(class, false)
    }

    fn after(&self, _op: FaultOp, _path: &Path, _to: Option<&Path>) {}
}

struct ProcessEnded;

impl StepHook for Turnstile {
    fn before(&self, step: SharedStep) {
        let class = match contract::announced(step) {
            contract::Class::Other => Class::Other,
            class => Class::Contract(class),
        };
        if matches!(self.check(class, true), Verdict::Die) {
            resume_unwind(Box::new(ProcessEnded));
        }
    }
}

pub(super) fn factory(
    implementation: Implementation,
    hook: Option<Arc<dyn StepHook>>,
) -> Arc<dyn CoordinationFactory> {
    match (implementation, hook) {
        (Implementation::File, _) => Arc::new(FileFactory::new()),
        (Implementation::Shared, Some(hook)) => Arc::new(SharedFactory::new().with_hook(hook)),
        (Implementation::Shared, None) => Arc::new(SharedFactory::new()),
    }
}

pub(super) struct Process {
    recorder: Arc<Recorder>,
    turnstile: Arc<Turnstile>,
    store: Arc<Mutex<Option<Store>>>,
    thread: Option<JoinHandle<()>>,
}

impl Process {
    pub(super) fn start(
        mem: &MemFs,
        world: Arc<World>,
        implementation: Implementation,
        actor: usize,
    ) -> Process {
        let recorder = Arc::new(Recorder::over(mem.another_process()));
        let turnstile = Arc::new(Turnstile {
            gate: Mutex::default(),
            moved: Condvar::new(),
            world,
            implementation,
        });
        recorder.watch(turnstile.clone());
        let store = Arc::new(Mutex::new(None));
        let serving = (recorder.clone(), turnstile.clone(), store.clone());
        let thread = std::thread::spawn(move || serve(serving.0, serving.1, serving.2, actor));
        Process {
            recorder,
            turnstile,
            store,
            thread: Some(thread),
        }
    }

    pub(super) fn command(&self, command: Command, stop: Option<Class>) -> Result<(), String> {
        {
            let mut gate = self.turnstile.gate();
            if gate.busy {
                return Err("a Store call is already running".to_owned());
            }
            gate.stop = stop;
            gate.grants += 1;
            gate.busy = true;
            gate.reply = None;
            gate.command = Some(command);
        }
        self.turnstile.moved.notify_all();
        self.settle()
    }

    pub(super) fn step(&self, stop: Option<Class>, fail: bool) -> Result<(), String> {
        {
            let mut gate = self.turnstile.gate();
            if gate.stopped.map(|(grant, _)| grant) != Some(gate.grants) {
                return Err("the Store call is not paused".to_owned());
            }
            if fail && gate.atomic_stop {
                return Err("a shared atomic step is infallible".to_owned());
            }
            gate.stop = stop;
            gate.fail = fail;
            gate.grants += 1;
        }
        self.turnstile.moved.notify_all();
        self.settle()
    }

    fn settle(&self) -> Result<(), String> {
        let gate = self.turnstile.gate();
        let grant = gate.grants;
        let (_gate, waited) = self
            .turnstile
            .moved
            .wait_timeout_while(gate, LIMIT, |gate| {
                gate.stopped.map(|(stopped, _)| stopped) != Some(grant) && gate.busy
            })
            .unwrap_or_else(PoisonError::into_inner);
        if waited.timed_out() {
            return Err("the Store call neither paused nor returned".to_owned());
        }
        Ok(())
    }

    pub(super) fn returned(&self) -> Option<Reply> {
        let gate = self.turnstile.gate();
        if gate.busy { None } else { gate.reply.clone() }
    }

    pub(super) fn paused(&self) -> Option<Class> {
        self.turnstile.gate().stopped.map(|(_, class)| class)
    }

    pub(super) fn end(&mut self) {
        // A call parked inside a change to the coordination state fails first: abandonment waits
        // for a change in progress, and a parked one would never finish.
        self.recorder.kill();
        self.turnstile.gate().ended = true;
        self.turnstile.moved.notify_all();
        if let Some(store) = self
            .store
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            store.abandon_for_replay();
        }
        self.recorder.mem().abandon();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.end();
    }
}

fn serve(
    recorder: Arc<Recorder>,
    turnstile: Arc<Turnstile>,
    visible: Arc<Mutex<Option<Store>>>,
    actor: usize,
) {
    let mut store: Option<Store> = None;
    let mut recovery: Option<Recovery> = None;
    loop {
        let command = {
            let mut gate = turnstile
                .moved
                .wait_while(turnstile.gate(), |gate| {
                    gate.command.is_none() && !gate.ended
                })
                .unwrap_or_else(PoisonError::into_inner);
            if gate.ended {
                break;
            }
            gate.command.take().expect("a Store command is pending")
        };
        let result = catch_unwind(AssertUnwindSafe(|| -> Result<Reply, String> {
            if matches!(command, Command::Open) {
                let nonces = turnstile.world.clone();
                let options =
                    StoreOptions::with_nonce_source(test_agent(), move || nonces.nonce(actor))
                        .with_coordination(factory(
                            turnstile.implementation,
                            Some(turnstile.clone()),
                        ));
                let (opened, _) = Store::open(recorder.clone(), &turnstile.world.root, options)
                    .map_err(|error| error.to_string())?;
                *visible.lock().unwrap_or_else(PoisonError::into_inner) = Some(opened.clone());
                store = Some(opened);
                return Ok(Reply::Done);
            }
            if matches!(command, Command::Close) {
                recovery = None;
                visible
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                store = None;
                return Ok(Reply::Done);
            }
            let store = store.as_ref().ok_or("a Store call needs an open handle")?;
            match command {
                Command::BracketedWrite {
                    document,
                    version,
                    logged,
                    quiesce_before,
                    quiesce_after,
                } => {
                    let _writer = store.writable().map_err(|error| error.to_string())?;
                    if quiesce_before {
                        store
                            .quiesce_commit_log()
                            .map_err(|error| error.to_string())?;
                    }
                    write(store, &turnstile.world, actor, document, version, logged)?;
                    if quiesce_after {
                        store
                            .quiesce_commit_log()
                            .map_err(|error| error.to_string())?;
                    }
                    Ok(Reply::Done)
                }
                Command::Acquire => {
                    let _writer = store.enter_exclusion().map_err(|error| error.to_string())?;
                    Ok(Reply::Done)
                }
                Command::Empty => {
                    let _writer = store.writable().map_err(|error| error.to_string())?;
                    let _modification = store
                        .begin_modification()
                        .map_err(|error| error.to_string())?;
                    Ok(Reply::Done)
                }
                Command::Quiesce => {
                    store
                        .quiesce()
                        .map_err(|error| error.to_string())?
                        .agent_stamp
                        .map_err(|error| {
                            format!("metadata stamp after successful quiescence: {error}")
                        })?;
                    Ok(Reply::Done)
                }
                Command::Recover => {
                    store
                        .recover()
                        .map_err(|error| error.to_string())?
                        .agent_stamp
                        .map_err(|error| {
                            format!("metadata stamp after successful recovery: {error}")
                        })?;
                    Ok(Reply::Done)
                }
                Command::Write {
                    document,
                    version,
                    logged,
                } => write(store, &turnstile.world, actor, document, version, logged),
                Command::BeginRecovery => {
                    let begun = store.recovery().map_err(|error| error.to_string())?;
                    let finished = begun.progress().finished();
                    let phase = begun.progress().phase();
                    recovery = Some(begun);
                    Ok(Reply::Recovery { phase, finished })
                }
                Command::RecoveryUnit => {
                    let progress = recovery
                        .as_mut()
                        .ok_or("no recovery run is active")?
                        .step()
                        .map_err(|error| error.to_string())?;
                    Ok(Reply::Recovery {
                        phase: progress.value.phase(),
                        finished: progress.value.finished(),
                    })
                }
                Command::Open | Command::Close => unreachable!(),
            }
        }));
        let reply = match result {
            Ok(Ok(reply)) => reply,
            Ok(Err(error)) => Reply::Failed(error),
            Err(_) => Reply::Failed("the process ended".to_owned()),
        };
        let mut gate = turnstile.gate();
        gate.reply = Some(reply);
        gate.busy = false;
        turnstile.moved.notify_all();
        if gate.ended {
            break;
        }
    }
    std::mem::forget(recovery);
    std::mem::forget(store);
}

fn write(
    store: &Store,
    world: &World,
    actor: usize,
    document: usize,
    version: usize,
    logged: bool,
) -> Result<Reply, String> {
    let document = &world.documents[document - 1];
    let nonce = world.nonce(actor);
    if logged {
        store
            .apply_transaction_as(
                &SwapSet::single(Swap::new(
                    document.node.clone(),
                    document.name.clone(),
                    World::body(version),
                )),
                &nonce,
            )
            .map_err(|error| error.to_string())?;
    } else {
        store
            .write_document_replicated_as(
                &document.node,
                &document.name,
                &World::body(version),
                &nonce,
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(Reply::Done)
}
