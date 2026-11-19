// Copyright (c) Michael Grier.

//! Driving the writers through a `Coordination` trace, one step at a time.

use std::sync::Arc;

use acadia::coord::{Coordination, ModificationEnd, Waiting};
use acadia::{MemFs, Vfs};
use serde_json::{Map, Value};

use super::super::corpus::Trace;
use super::process::{Class, Command, Implementation, Process, Reply, Stop};
use super::projection::{self, Projection, root};

/// Where the code left a trace, and how.
#[derive(Debug)]
pub(super) struct Departure {
    pub(super) step: usize,
    pub(super) detail: String,
}

/// What a model step asks of a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The client makes a call of the contract.
    Command(Command),
    /// A call already made goes on through the step that begins with a call of this class.
    Continue(Class),
    /// As `Continue`, and the step's first call fails.
    Fail(Class),
    /// The process stops recovery, which a call of the contract's completion of it cannot.
    StopRecovery,
    /// Nothing the contract sees: the client's own data, or a step within a call that makes no
    /// call of its own.
    Silent,
    Terminate,
}

fn kind(implementation: Implementation, action: &str) -> Kind {
    let tiers = |shared: Class| match implementation {
        Implementation::File => Class::Tiers,
        Implementation::Shared => shared,
    };
    match action {
        "BeginAttach" => Kind::Command(Command::Attach),
        "TakeGateS" => Kind::Continue(Class::Gate),
        "TakeReaderS" => Kind::Continue(tiers(Class::ReaderTier)),
        "TakeWriterS" => Kind::Continue(tiers(Class::WriterTier)),
        "Exit" => Kind::Command(Command::Detach),
        "Request" => Kind::Command(Command::Acquire),
        "Lock" => Kind::Continue(Class::Exclusion),
        "SetFence" => Kind::Continue(Class::Fence),
        "Record" => Kind::Continue(Class::Record),
        "BeginOpen" => Kind::Command(Command::Begin),
        "EndOpen" | "EndClose" => Kind::Continue(Class::Count),
        "OpenFails" | "CloseFails" => Kind::Fail(Class::Count),
        "BeginClose" => Kind::Command(Command::End(ModificationEnd::Settled)),
        "FailUnresolved" | "PrepareFails" => {
            Kind::Command(Command::End(ModificationEnd::Unresolved))
        }
        "Release" => Kind::Command(Command::Release),
        "RecCount" => Kind::Command(Command::Recover),
        "RecClear" => Kind::Continue(Class::ClearFence),
        "RecStop" => Kind::StopRecovery,
        "Terminate" => Kind::Terminate,
        _ => Kind::Silent,
    }
}

/// Follows `trace` with `implementation`: each writer is a process of its own, held before each
/// step's first call, and the contract's state is compared with the model's after every step.
pub(super) fn replay(trace: &Trace, implementation: Implementation) -> Result<(), Departure> {
    let mut replay = Replay::new(trace, implementation);
    for k in 0..trace.steps.len() {
        replay
            .step(k)
            .map_err(|detail| Departure { step: k, detail })?;
    }
    Ok(())
}

struct Replay<'t> {
    trace: &'t Trace,
    implementation: Implementation,
    mem: MemFs,
    /// A participant of the replay's own that stays attached, so that the shared state lives as
    /// long as the model's does: across every process's death, until a crash.
    keeper: Keeper,
    observer: Arc<dyn Coordination>,
    processes: Vec<Option<Process>>,
    /// The call each process last made.
    calls: Vec<Option<Command>>,
    seen: Projection,
}

struct Keeper {
    mem: Arc<MemFs>,
    participant: Arc<dyn Coordination>,
}

impl Keeper {
    /// Attaches, and recovers state that cannot be vouched for, as the model's state starts.
    fn start(mem: &MemFs, implementation: Implementation) -> Keeper {
        let process = Arc::new(mem.another_process());
        let participant = implementation
            .participant(process.clone() as Arc<dyn Vfs>, None)
            .expect("the keeper's participant");
        let mut hold = participant
            .acquire(Waiting::Wait)
            .expect("the keeper takes exclusion");
        hold.complete_recovery().expect("the keeper recovers");
        hold.release().expect("the keeper lets go");
        Keeper {
            mem: process,
            participant,
        }
    }

    /// Attaches to state the model cannot vouch for, as it is after a crash.
    fn rejoin(mem: &MemFs, implementation: Implementation) -> Keeper {
        let process = Arc::new(mem.another_process());
        let participant = implementation
            .participant(process.clone() as Arc<dyn Vfs>, None)
            .expect("the keeper's participant");
        participant.trust().expect("the keeper opens the state");
        Keeper {
            mem: process,
            participant,
        }
    }

    fn end(&self) {
        self.participant.abandon();
        self.mem.abandon();
    }
}

/// Process `p`'s value of the model variable `name` in `state`.
fn of<'s>(state: &'s Map<String, Value>, name: &str, p: usize) -> &'s Value {
    &state[name][p - 1]
}

impl<'t> Replay<'t> {
    fn new(trace: &'t Trace, implementation: Implementation) -> Replay<'t> {
        let mem = MemFs::new();
        mem.create_dir_all(root()).expect("the root is made");
        mem.fsync(root()).expect("the root is durable");
        let keeper = Keeper::start(&mem, implementation);
        let observer = acadia::coord::file::FileFactory::new();
        let observer: Arc<dyn Coordination> = Arc::from(
            acadia::coord::CoordinationFactory::open(
                &observer,
                Arc::new(mem.another_process()),
                root(),
            )
            .expect("the observer's participant"),
        );
        let procs = trace.states[0]["pc"]
            .as_array()
            .expect("pc is a sequence")
            .len();
        let seen = projection::observe(&mem, implementation, observer.as_ref());
        let model = projection::model(&trace.states[0]);
        assert_eq!(
            (seen.trusted, seen.fence, seen.owner, seen.count_odd),
            (model.trusted, model.fence, model.owner, model.count_odd),
            "{}: the contract's state does not start as the model's",
            trace.file
        );
        Replay {
            trace,
            implementation,
            mem,
            keeper,
            observer,
            processes: (0..procs).map(|_| None).collect(),
            calls: vec![None; procs],
            seen,
        }
    }

    /// The process step `k` is of, and its action; `None` for a step of no process. TLC names
    /// some steps by the enclosing `Next`, and those are read from what they change.
    fn action(&self, k: usize) -> Result<Option<(usize, String)>, String> {
        let step = &self.trace.steps[k];
        if step.name != "Next" {
            return Ok(step
                .args
                .first()
                .and_then(Value::as_u64)
                .and_then(|p| usize::try_from(p).ok())
                .map(|p| (p, step.name.clone())));
        }
        let (before, after) = (&self.trace.states[k], &self.trace.states[k + 1]);
        for p in 1..=self.processes.len() {
            let att = (
                of(before, "att", p).as_str().unwrap_or_default(),
                of(after, "att", p).as_str().unwrap_or_default(),
            );
            let pc = (
                of(before, "pc", p).as_str().unwrap_or_default(),
                of(after, "pc", p).as_str().unwrap_or_default(),
            );
            let name = match (att, pc) {
                (("wGate", "hasGate"), _) => "TakeGateS",
                (("hasGate", "gateRd"), _) => "TakeReaderS",
                (("rd" | "gateWr" | "rdChecked", _), _) if att.0 != att.1 => "TakeWriterS",
                (_, ("waiting", "locked")) => "Lock",
                _ => continue,
            };
            return Ok(Some((p, name.to_owned())));
        }
        Err(format!(
            "no action of the contract's writers is {}",
            step.action
        ))
    }

    /// Where process `p`, taking step `k`, is held next: before the first call of its next step
    /// if that is a step within this call, before any step if it dies or the trace ends first,
    /// and nowhere if its next step is a call of its own.
    fn stop_after(&self, k: usize, p: usize) -> Result<Option<Stop>, String> {
        for j in k + 1..self.trace.steps.len() {
            if self.trace.steps[j].name == "Crash" {
                return Ok(Some(Stop::AnyStep));
            }
            let Some((q, action)) = self.action(j)? else {
                continue;
            };
            if q != p {
                continue;
            }
            return Ok(match kind(self.implementation, &action) {
                Kind::Continue(class) | Kind::Fail(class) => Some(Stop::At(class)),
                Kind::Terminate => Some(Stop::AnyStep),
                Kind::Command(_) => None,
                Kind::Silent | Kind::StopRecovery => continue,
            });
        }
        Ok(Some(Stop::AnyStep))
    }

    fn step(&mut self, k: usize) -> Result<(), String> {
        let trace = self.trace;
        let step = &trace.steps[k];
        let after = &trace.states[k + 1];
        match step.name.as_str() {
            "Crash" => self.crash(after),
            "Terminal" => {}
            _ => {
                if let Some((p, action)) = self.action(k)? {
                    self.act(k, p, &action)
                        .map_err(|detail| format!("process {p}, {action}: {detail}"))?;
                }
            }
        }
        let model_before = projection::model(&trace.states[k]);
        let model = projection::model(after);
        let code = projection::observe(&self.mem, self.implementation, self.observer.as_ref());
        if let Some(difference) = projection::differ(&model_before, &model, &self.seen, &code) {
            return Err(difference);
        }
        self.seen = code;
        Ok(())
    }

    fn act(&mut self, k: usize, p: usize, action: &str) -> Result<(), String> {
        let stop = if action == "Release" {
            None
        } else {
            self.stop_after(k, p)?
        };
        match kind(self.implementation, action) {
            Kind::Command(command) => {
                if self.processes[p - 1].is_none() {
                    self.processes[p - 1] = Some(Process::start(&self.mem, self.implementation)?);
                }
                self.calls[p - 1] = Some(command);
                self.process(p)?.command(command, stop)?;
            }
            Kind::Continue(_) => self.process(p)?.step(stop, false)?,
            Kind::Fail(_) => self.process(p)?.step(stop, true)?,
            Kind::StopRecovery => {
                if self.process(p)?.returned().is_none() {
                    return Err(
                        "the contract's completion of recovery publishes the count and \
                                clears the fence in one call, which its caller cannot stop \
                                between them"
                            .to_owned(),
                    );
                }
                return Ok(());
            }
            Kind::Terminate => {
                if let Some(process) = self.processes[p - 1].as_mut() {
                    process.kill();
                }
                return Ok(());
            }
            Kind::Silent => return Ok(()),
        }
        self.check_return(k, p, stop)
    }

    fn process(&self, p: usize) -> Result<&Process, String> {
        self.processes[p - 1]
            .as_ref()
            .ok_or_else(|| "it has made no call".to_owned())
    }

    /// Whether the process's call returned where the model's ends, and with what it ends with.
    fn check_return(&self, k: usize, p: usize, stop: Option<Stop>) -> Result<(), String> {
        let returned = self.process(p)?.returned();
        let pc = of(&self.trace.states[k + 1], "pc", p)
            .as_str()
            .unwrap_or_default();
        let command = self.calls[p - 1].expect("a process that made a call");
        match (stop, returned) {
            (None, None) => Err(format!("its {command:?} went on where the model's ended")),
            (Some(Stop::At(class)), Some(reply)) => Err(format!(
                "its {command:?} returned {reply:?} before reaching {class:?}"
            )),
            (Some(_), _) => Ok(()),
            (None, Some(reply)) => {
                let expected = match (command, &reply) {
                    (Command::Acquire, Reply::Acquired { told }) => *told == (pc == "told"),
                    (Command::Begin, Reply::Done) => pc == "open",
                    (Command::Begin, Reply::Failed(_)) => pc == "closed",
                    (Command::End(ModificationEnd::Settled), Reply::Done) => pc == "closed",
                    (Command::End(ModificationEnd::Settled), Reply::Failed(_)) => pc == "told",
                    (_, Reply::Done) => true,
                    _ => false,
                };
                if expected {
                    Ok(())
                } else {
                    Err(format!(
                        "its {command:?} returned {reply:?} where the model's is {pc}"
                    ))
                }
            }
        }
    }

    /// Kills every process, keeps or loses what the model's crash keeps or loses, and loses
    /// what is not durable.
    fn crash(&mut self, after: &Map<String, Value>) {
        for process in self.processes.iter().flatten() {
            process.condemn();
        }
        for process in self.processes.iter_mut().flatten() {
            process.join();
        }
        self.keeper.end();
        if self.implementation == Implementation::File {
            let kept = projection::model(after);
            projection::settle_crash(&self.mem, kept.owner, kept.fence);
        }
        self.mem = self.mem.simulate_restart();
        let observer = acadia::coord::file::FileFactory::new();
        self.observer = Arc::from(
            acadia::coord::CoordinationFactory::open(
                &observer,
                Arc::new(self.mem.another_process()),
                root(),
            )
            .expect("the observer reopens after the crash"),
        );
        self.keeper = Keeper::rejoin(&self.mem, self.implementation);
    }
}

impl Drop for Replay<'_> {
    fn drop(&mut self) {
        for process in self.processes.iter().flatten() {
            process.condemn();
        }
        for process in self.processes.iter_mut().flatten() {
            process.join();
        }
        self.keeper.end();
    }
}
