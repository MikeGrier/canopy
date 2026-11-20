// Copyright (c) Michael Grier.

//! The model's writers: each a thread that makes the contract's calls on a participant of its
//! own, over its own simulated process of one filesystem, held before the first call of each
//! step until the replay lets it take that step.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use acadia::coord::file::FileFactory;
use acadia::coord::shared::{SharedFactory, SharedStep, StepHook};
use acadia::coord::{
    Access, Attachment, CoordError, CoordResult, Coordination, CoordinationFactory, ExclusionHold,
    ModificationEnd, Purpose, ReaderSafety, Upgrade, UpgradeConversion, Waiting,
};
use acadia::{FaultOp, MemFs, Vfs};

use super::super::recorder::{Recorder, Verdict, Watcher};
use super::projection::root;
use crate::store::{DrainToken, Leases, OperationLease};

/// How long a process may take to reach its next step before the replay gives up on it.
const LIMIT: Duration = Duration::from_secs(10);

/// The implementation of the contract a replay holds to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tests::conformance) enum Implementation {
    File,
    Shared,
}

impl Implementation {
    /// A participant over `vfs`, announcing the shared implementation's steps to `hook`.
    pub(super) fn participant(
        self,
        vfs: Arc<dyn Vfs>,
        hook: Option<Arc<dyn StepHook>>,
    ) -> CoordResult<Arc<dyn Coordination>> {
        let participant = match (self, hook) {
            (Implementation::File, _) => FileFactory::new().open(vfs, root())?,
            (Implementation::Shared, None) => SharedFactory::new().open(vfs, root())?,
            (Implementation::Shared, Some(hook)) => {
                SharedFactory::new().with_hook(hook).open(vfs, root())?
            }
        };
        Ok(Arc::from(participant))
    }

    /// The directory beneath the root that holds the implementation's state.
    fn state_dir(self) -> &'static str {
        match self {
            Implementation::File => ".coord",
            Implementation::Shared => ".coord-shared",
        }
    }
}

/// A kind of call, as much of it as says which of the model's steps it begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tests::conformance) enum Class {
    Gate,
    /// The file-based implementation's two tiers, one lock file of two bytes.
    Tiers,
    ReaderTier,
    WriterTier,
    Exclusion,
    Fence,
    Record,
    /// A publication of the count: an opening or closing raise, or the recovered count.
    Count,
    ClearRecord,
    ClearFence,
    RegistryLock,
    RegistryRead,
    RegistryWrite,
    Other,
}

/// Where a process is held next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stop {
    /// Before the first call of this class.
    At(Class),
    /// Before the second call of this class.
    Second(Class),
    /// Before the first call that begins any step.
    AnyStep,
}

impl Stop {
    fn matches(self, class: Class) -> bool {
        match self {
            Stop::At(stop) | Stop::Second(stop) => stop == class,
            Stop::AnyStep => class != Class::Other,
        }
    }
}

pub(in crate::tests::conformance) fn classify(
    implementation: Implementation,
    op: FaultOp,
    path: &Path,
) -> Class {
    let state = root().join(implementation.state_dir());
    if implementation == Implementation::File
        && path.parent() == Some(state.join("registry").as_path())
    {
        return match op {
            FaultOp::Read => Class::RegistryRead,
            FaultOp::Write | FaultOp::PublishNew | FaultOp::RemoveFile => Class::RegistryWrite,
            _ => Class::Other,
        };
    }
    let Some(name) = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|_| path.parent() == Some(state.as_path()))
    else {
        return Class::Other;
    };
    let lock = matches!(
        op,
        FaultOp::Lock | FaultOp::TryLock | FaultOp::LockExclusive
    );
    match (implementation, lock, op, name) {
        (_, true, _, "gate.lock") => Class::Gate,
        (_, true, _, "exclusion.lock") => Class::Exclusion,
        (Implementation::File, true, _, "tiers.lock") => Class::Tiers,
        (Implementation::Shared, true, _, "reader-tier.lock") => Class::ReaderTier,
        (Implementation::Shared, true, _, "writer-tier.lock") => Class::WriterTier,
        (Implementation::File, false, FaultOp::Write, "fence") => Class::Fence,
        (Implementation::File, false, FaultOp::RemoveFile, "fence") => Class::ClearFence,
        (Implementation::File, false, FaultOp::RemoveFile, "owner") => Class::ClearRecord,
        (Implementation::File, false, FaultOp::Write, name)
            if name.starts_with("generation.staging.") =>
        {
            Class::Record
        }
        (Implementation::File, false, FaultOp::Write, name)
            if name.starts_with("count.staging.") =>
        {
            Class::Count
        }
        (_, true, _, name)
            if name == "registry.lock"
                || name == "registry-claims.lock"
                || name == "registry-live.lock" =>
        {
            Class::RegistryLock
        }
        (_, false, FaultOp::Read, name) if name.starts_with("registry-") => Class::RegistryRead,
        (_, false, FaultOp::Write | FaultOp::PublishNew | FaultOp::RemoveFile, name)
            if name.starts_with("registry-") =>
        {
            Class::RegistryWrite
        }
        _ => Class::Other,
    }
}

/// The class of a step the shared implementation announces.
pub(in crate::tests::conformance) fn announced(step: SharedStep) -> Class {
    match step {
        SharedStep::SetFence | SharedStep::Unresolved => Class::Fence,
        SharedStep::Record => Class::Record,
        SharedStep::PublishRaise | SharedStep::PublishClose | SharedStep::PublishRecovered => {
            Class::Count
        }
        SharedStep::ClearRecord => Class::ClearRecord,
        SharedStep::ClearFence => Class::ClearFence,
        SharedStep::BeginRaise | SharedStep::BeginClose => Class::Other,
    }
}

/// A call of the contract a process makes as the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Command {
    Attach,
    Detach,
    List,
    Discover,
    BeginRead,
    EndRead,
    BeginOperation,
    EndOperation,
    StartUpgrade,
    CancelUpgrade,
    Convert,
    PollGate,
    PollTiers(ReaderSafety),
    BeginUpgrade(bool),
    EndUpgrade,
    Acquire,
    Begin,
    End(ModificationEnd),
    Recover,
    Release,
}

/// What a call returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Reply {
    Done,
    Listed {
        live: usize,
        incomplete: bool,
    },
    Discovered {
        required: bool,
    },
    Sampled(u64),
    Withdrawn,
    /// Writer exclusion was acquired, and whether the holder was told recovery is required.
    Acquired {
        told: bool,
    },
    Failed(String),
}

#[derive(Default)]
struct Gate {
    stop: Option<Stop>,
    matched: usize,
    /// How many calls and steps the process has been let take.
    grants: u64,
    /// The grant under which the process stopped, while it is stopped.
    stopped: Option<u64>,
    /// The call the process is held before fails when it is let go.
    fail: bool,
    withdraw: bool,
    probe_upgrade: bool,
    upgrade_refused: Option<bool>,
    killed: bool,
    command: Option<Command>,
    busy: bool,
    reply: Option<Reply>,
}

/// The marker a dead process unwinds with from inside the shared implementation's step.
struct Killed;

struct Turnstile {
    gate: Mutex<Gate>,
    moved: Condvar,
    implementation: Implementation,
}

impl Turnstile {
    fn gate(&self) -> MutexGuard<'_, Gate> {
        self.gate.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn check(&self, class: Class) -> Verdict {
        let mut gate = self.gate();
        let matched = if !gate.killed && gate.stop.is_some_and(|stop| stop.matches(class)) {
            gate.matched += 1;
            match gate.stop {
                Some(Stop::Second(_)) => gate.matched == 2,
                Some(_) => true,
                None => false,
            }
        } else {
            false
        };
        if matched {
            let grant = gate.grants;
            gate.stopped = Some(grant);
            self.moved.notify_all();
            loop {
                gate = self
                    .moved
                    .wait_while(gate, |gate| {
                        gate.grants == grant && !gate.probe_upgrade && !gate.killed
                    })
                    .unwrap_or_else(PoisonError::into_inner);
                if std::mem::take(&mut gate.probe_upgrade) {
                    gate.upgrade_refused = Some(Leases::control_context().is_err());
                    self.moved.notify_all();
                    continue;
                }
                break;
            }
            gate.stopped = None;
            if !gate.killed && std::mem::take(&mut gate.withdraw) {
                return Verdict::Locked;
            }
            if !gate.killed && std::mem::take(&mut gate.fail) {
                return Verdict::Fail;
            }
        }
        if gate.killed {
            Verdict::Terminate
        } else {
            Verdict::Proceed
        }
    }
}

impl Watcher for Turnstile {
    fn before(&self, op: FaultOp, path: &Path, _to: Option<&Path>) -> Verdict {
        self.check(classify(self.implementation, op, path))
    }

    fn after(&self, _op: FaultOp, _path: &Path, _to: Option<&Path>) {}
}

impl StepHook for Turnstile {
    fn before(&self, step: SharedStep) {
        if matches!(self.check(announced(step)), Verdict::Terminate) {
            resume_unwind(Box::new(Killed));
        }
    }
}

/// One of the model's writers, running.
pub(super) struct Process {
    recorder: Arc<Recorder>,
    turnstile: Arc<Turnstile>,
    participant: Arc<dyn Coordination>,
    thread: Option<JoinHandle<()>>,
}

impl Process {
    pub(super) fn start(
        mem: &MemFs,
        implementation: Implementation,
        register: bool,
    ) -> Result<Process, String> {
        Self::start_view(mem.another_process(), implementation, register)
    }

    pub(super) fn listing(&self, implementation: Implementation) -> Result<Process, String> {
        Self::start_view(self.recorder.mem().same_process(), implementation, false)
    }

    fn start_view(
        mem: MemFs,
        implementation: Implementation,
        register: bool,
    ) -> Result<Process, String> {
        let recorder = Arc::new(Recorder::over(mem));
        let turnstile = Arc::new(Turnstile {
            gate: Mutex::new(Gate::default()),
            moved: Condvar::new(),
            implementation,
        });
        recorder.watch(turnstile.clone());
        let participant = implementation
            .participant(
                recorder.clone(),
                Some(turnstile.clone() as Arc<dyn StepHook>),
            )
            .map_err(|error| format!("{error:?} opening a participant"))?;
        let serving = (Arc::clone(&turnstile), Arc::clone(&participant), register);
        let thread = std::thread::spawn(move || serve(&serving.0, serving.1.as_ref(), serving.2));
        Ok(Process {
            recorder,
            turnstile,
            participant,
            thread: Some(thread),
        })
    }

    /// Makes `command`, held before the first call that `stop` holds.
    pub(super) fn command(&self, command: Command, stop: Option<Stop>) -> Result<(), String> {
        {
            let mut gate = self.turnstile.gate();
            if gate.busy {
                return Err(format!("{command:?} began while a call was still running"));
            }
            gate.stop = stop;
            gate.matched = 0;
            gate.grants += 1;
            gate.busy = true;
            gate.reply = None;
            gate.command = Some(command);
        }
        self.turnstile.moved.notify_all();
        self.settle()
    }

    /// Lets the process take the step it is held before, failing that step's first call if
    /// `fail`, up to the call `stop` holds.
    pub(super) fn step(&self, stop: Option<Stop>, fail: bool) -> Result<(), String> {
        {
            let mut gate = self.turnstile.gate();
            if gate.stopped != Some(gate.grants) {
                return Err("it is not held before a step".to_owned());
            }
            gate.stop = stop;
            gate.matched = 0;
            gate.fail = fail;
            gate.grants += 1;
        }
        self.turnstile.moved.notify_all();
        self.settle()
    }

    pub(super) fn refuse_upgrade(&self) -> Result<bool, String> {
        let mut gate = self.turnstile.gate();
        if gate.stopped != Some(gate.grants) {
            return Err("it is not held inside an operation".to_owned());
        }
        gate.probe_upgrade = true;
        gate.upgrade_refused = None;
        self.turnstile.moved.notify_all();
        let (checked, timed) = self
            .turnstile
            .moved
            .wait_timeout_while(gate, LIMIT, |gate| gate.upgrade_refused.is_none())
            .unwrap_or_else(PoisonError::into_inner);
        drop(checked);
        if timed.timed_out() {
            return Err("the process did not answer the upgrade check".to_owned());
        }
        let mut gate = self.turnstile.gate();
        Ok(gate.upgrade_refused.take().expect("upgrade check answered"))
    }

    pub(super) fn withdraw(&self) -> Result<(), String> {
        {
            let mut gate = self.turnstile.gate();
            if gate.stopped != Some(gate.grants) {
                return Err("it is not held before an acquisition".to_owned());
            }
            gate.stop = None;
            gate.withdraw = true;
            gate.grants += 1;
        }
        self.turnstile.moved.notify_all();
        self.settle()
    }

    /// Waits for the process to be held before its next step, or for its call to return.
    fn settle(&self) -> Result<(), String> {
        let gate = self.turnstile.gate();
        let grant = gate.grants;
        let (_gate, waited) = self
            .turnstile
            .moved
            .wait_timeout_while(gate, LIMIT, |gate| gate.stopped != Some(grant) && gate.busy)
            .unwrap_or_else(PoisonError::into_inner);
        if waited.timed_out() {
            return Err("it neither reached its next step nor returned".to_owned());
        }
        Ok(())
    }

    /// What its last call returned, once it has.
    pub(super) fn returned(&self) -> Option<Reply> {
        let gate = self.turnstile.gate();
        if gate.busy { None } else { gate.reply.clone() }
    }

    /// Kills the process: every call it makes from here fails, it lets go of nothing cleanly,
    /// and its locks are released as the operating system releases a dead process's.
    pub(super) fn condemn(&self) {
        self.turnstile.gate().killed = true;
        self.recorder.kill();
        self.turnstile.moved.notify_all();
        self.participant.abandon();
        self.recorder.mem().abandon();
    }

    /// Waits for a condemned process's thread to end.
    pub(super) fn join(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    pub(super) fn kill(&mut self) {
        self.condemn();
        self.join();
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.kill();
    }
}

#[derive(Default)]
struct WorkerState {
    attachment: Option<Box<dyn Attachment>>,
    conversion: Option<Box<dyn UpgradeConversion>>,
    upgrade: Option<Box<dyn Upgrade>>,
    hold: Option<Box<dyn ExclusionHold>>,
    leases: Arc<Leases>,
    operations: Vec<OperationLease>,
    drain: Option<Arc<DrainToken>>,
}

/// The process's thread: makes each call it is given, until it is killed.
fn serve(turnstile: &Turnstile, participant: &dyn Coordination, register: bool) {
    let mut state = WorkerState::default();
    loop {
        let command = {
            let mut gate = turnstile
                .moved
                .wait_while(turnstile.gate(), |gate| {
                    gate.command.is_none() && !gate.killed
                })
                .unwrap_or_else(PoisonError::into_inner);
            if gate.killed {
                break;
            }
            gate.command.take().expect("a command was given")
        };
        let reply = catch_unwind(AssertUnwindSafe(|| {
            call(command, participant, &mut state, register)
        }))
        .unwrap_or_else(|_| Reply::Failed("the process died".to_owned()));
        let mut gate = turnstile.gate();
        gate.reply = Some(reply);
        gate.busy = false;
        turnstile.moved.notify_all();
        if gate.killed {
            break;
        }
    }
    // A dead process runs no clean release: its handles are never dropped.
    std::mem::forget(state.hold);
    std::mem::forget(state.attachment);
}

/// The hold `command` needs, or the reply that it has none.
fn held(
    command: Command,
    hold: &mut Option<Box<dyn ExclusionHold>>,
) -> Result<&mut dyn ExclusionHold, Reply> {
    match hold {
        Some(hold) => Ok(hold.as_mut()),
        None => Err(Reply::Failed(format!("{command:?} without exclusion"))),
    }
}

fn call(
    command: Command,
    participant: &dyn Coordination,
    state: &mut WorkerState,
    register: bool,
) -> Reply {
    let WorkerState {
        attachment,
        conversion,
        upgrade,
        hold,
        leases,
        operations,
        drain,
    } = state;
    let done = |result: CoordResult<()>| match result {
        Ok(()) => Reply::Done,
        Err(error) => Reply::Failed(format!("{error:?}")),
    };
    match command {
        Command::Attach => match participant.attach(Access::Write, Waiting::Never) {
            Ok(mut attached) => {
                if register && let Err(error) = attached.register(b"coordination replay") {
                    return Reply::Failed(format!("{error:?}"));
                }
                *attachment = Some(attached);
                Reply::Done
            }
            Err(CoordError::WouldWait) => Reply::Withdrawn,
            Err(error) => Reply::Failed(format!("{error:?}")),
        },
        Command::Detach => {
            drop(attachment.take());
            Reply::Done
        }
        Command::List => match participant.list() {
            Ok(listing) => Reply::Listed {
                live: listing.live.len(),
                incomplete: listing.may_be_incomplete,
            },
            Err(error) => Reply::Failed(format!("{error:?}")),
        },
        Command::Discover => match participant.discover_recovery() {
            Ok(need) => Reply::Discovered {
                required: need.required(),
            },
            Err(error) => Reply::Failed(format!("{error:?}")),
        },
        Command::BeginRead => match leases.enter() {
            Ok(lease) => {
                operations.push(lease);
                match participant.sample() {
                    Ok(count) => Reply::Sampled(count.get()),
                    Err(error) => Reply::Failed(format!("{error:?}")),
                }
            }
            Err(error) => Reply::Failed(format!("{error:?}")),
        },
        Command::EndRead => match participant.sample() {
            Ok(count) => {
                operations.pop();
                Reply::Sampled(count.get())
            }
            Err(error) => Reply::Failed(format!("{error:?}")),
        },
        Command::BeginOperation => match leases.enter() {
            Ok(lease) => {
                operations.push(lease);
                Reply::Done
            }
            Err(error) => Reply::Failed(format!("{error:?}")),
        },
        Command::EndOperation => match operations.pop() {
            Some(_) => Reply::Done,
            None => Reply::Failed("an operation end without a lease".to_owned()),
        },
        Command::StartUpgrade => match leases.begin() {
            Ok(token) => {
                *drain = Some(token);
                Reply::Done
            }
            Err(error) => Reply::Failed(format!("{error:?}")),
        },
        Command::CancelUpgrade => {
            if let Some(token) = drain.take() {
                leases.cancel(&token);
            }
            drop(conversion.take());
            drop(upgrade.take());
            Reply::Done
        }
        Command::Convert => {
            let Some(token) = drain.as_ref() else {
                return Reply::Failed("conversion without a drain".to_owned());
            };
            if let Err(error) = leases.detach(token) {
                return Reply::Failed(format!("{error:?}"));
            }
            let Some(attached) = attachment.take() else {
                return Reply::Failed("conversion without attachment".to_owned());
            };
            match attached.begin_conversion() {
                Ok(converting) => {
                    *conversion = Some(converting);
                    Reply::Done
                }
                Err(error) => Reply::Failed(format!("{error:?}")),
            }
        }
        Command::PollGate => match conversion
            .as_mut()
            .expect("conversion is pending")
            .poll_gate()
        {
            Ok(true) => Reply::Done,
            Ok(false) => Reply::Failed("the gate is not ready".to_owned()),
            Err(error) => Reply::Failed(format!("{error:?}")),
        },
        Command::PollTiers(safety) => match conversion
            .as_mut()
            .expect("conversion is pending")
            .poll_tiers(safety)
        {
            Ok(Some(acquired)) => {
                *upgrade = Some(acquired);
                *conversion = None;
                Reply::Done
            }
            Ok(None) => Reply::Failed("the tiers are not ready".to_owned()),
            Err(error) => Reply::Failed(format!("{error:?}")),
        },
        Command::BeginUpgrade(with_exclusion) => {
            if with_exclusion {
                match participant.acquire(Waiting::Never) {
                    Ok(acquired) if acquired.acquired().recovery_required() => {
                        if let Err(error) = acquired.release() {
                            return Reply::Failed(format!("{error:?}"));
                        }
                        return Reply::Failed("RecoveryRequired".to_owned());
                    }
                    Ok(acquired) => *hold = Some(acquired),
                    Err(error) => return Reply::Failed(format!("{error:?}")),
                }
            }
            Reply::Done
        }
        Command::EndUpgrade => {
            drop(upgrade.take());
            leases.conversion_finished();
            Reply::Done
        }
        Command::Acquire => {
            let lease = match leases.enter() {
                Ok(lease) => lease,
                Err(error) => return Reply::Failed(format!("{error:?}")),
            };
            match participant.acquire(Waiting::Never) {
                Ok(acquired) => {
                    let told = acquired.acquired().recovery_required();
                    *hold = Some(acquired);
                    operations.push(lease);
                    Reply::Acquired { told }
                }
                Err(CoordError::WouldWait) => Reply::Withdrawn,
                Err(error) => Reply::Failed(format!("{error:?}")),
            }
        }
        Command::Begin => match held(command, hold) {
            Ok(hold) => done(hold.begin_modification(Purpose::Ordinary)),
            Err(reply) => reply,
        },
        Command::End(end) => match held(command, hold) {
            Ok(hold) => done(hold.end_modification(end)),
            Err(reply) => reply,
        },
        Command::Recover => match held(command, hold) {
            Ok(hold) => done(hold.complete_recovery()),
            Err(reply) => reply,
        },
        Command::Release => match hold.take() {
            Some(hold) => {
                let reply = done(hold.release());
                operations.pop();
                reply
            }
            None => Reply::Failed("a release without exclusion".to_owned()),
        },
    }
}
