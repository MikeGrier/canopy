// Copyright (c) Michael Grier.

use std::sync::Arc;

use acadia::coord::Coordination;
use acadia::{MemFs, Vfs};

use super::super::coordination::{process::Class as Contract, projection as contract};
use super::super::corpus::Trace;
use super::super::pointer_publication::class::Class as Document;
use super::action::{self, Action};
use super::process::{self, Class, Command, Implementation, Process, Reply};
use super::projection::{self, World};
use crate::Store;

#[derive(Debug)]
pub(super) struct Departure {
    pub(super) step: usize,
    pub(super) detail: String,
}

fn checkpoint(action: &Action, implementation: Implementation) -> Option<Class> {
    match action.name {
        "DirectorySync" => return Some(Class::DirectorySync),
        "DirectoryClear" => return Some(Class::DirectoryClear),
        "DirectoryClearSync" => return Some(Class::DirectoryClearSync),
        _ => {}
    }
    let replica = action.replica.saturating_sub(1);
    let contract = match action.name {
        "TakeGateS" => Some(Contract::Gate),
        "TakeReaderS" => Some(if implementation == Implementation::File {
            Contract::Tiers
        } else {
            Contract::ReaderTier
        }),
        "TakeWriterS" => Some(if implementation == Implementation::File {
            Contract::Tiers
        } else {
            Contract::WriterTier
        }),
        "Lock" => Some(Contract::Exclusion),
        "SetFence" => Some(Contract::Fence),
        "Record" => Some(Contract::Record),
        "EndOpen" | "EndClose" | "OpenFails" | "CloseFails" | "RecCount" => Some(Contract::Count),
        "RecClear" => Some(Contract::ClearFence),
        "Release" => Some(Contract::ClearRecord),
        _ => None,
    };
    if let Some(class) = contract {
        return Some(Class::Contract(class));
    }
    let document = match action.name {
        "IngRen" => Document::BlobRename(replica),
        "IngSync" | "CollFsync" | "RvCollFsync" | "RbCollFsync" => Document::BlobSync(replica),
        "RecWrite" => Document::RecordAppend,
        "RecSync" | "RecClrSync" | "RvClrSync" => Document::RecordSync,
        "RecClr" | "RvClr" => Document::RecordTruncate,
        "RvRecSync" => Document::RecordLook,
        "PtrRen" | "RvRen" | "RsRen" | "TxRen" => Document::PtrRename(replica),
        "PtrSync" | "RvSync" | "RsSync" | "TxSync" | "CollSync" | "RvCollSync" | "RbCollSync" => {
            Document::PtrSync(replica)
        }
        "CollRead" | "RvCollRead" | "RbCollRead" => Document::PtrRead,
        "CollRem" | "RvCollRem" | "RbCollRem" => Document::BlobList(replica),
        "Begin" | "Prepare" | "End" => Document::LogAppend,
        "Quiesce" | "RecReset" | "RollbackBeforePrepare" => Document::LogTruncate,
        _ => return None,
    };
    Some(Class::Document(document))
}

pub(super) fn replay(trace: &Trace, implementation: Implementation) -> Result<(), Departure> {
    let mut replay =
        Replay::start(trace, implementation).map_err(|detail| Departure { step: 0, detail })?;
    for index in 0..trace.steps.len() {
        replay.step(index).map_err(|detail| Departure {
            step: index,
            detail,
        })?;
    }
    Ok(())
}

fn phase_checkpoint(phase: &str, replica: usize, implementation: Implementation) -> Option<Class> {
    let name = match phase {
        "ingRen" => "IngRen",
        "ingSync" => "IngSync",
        "recWrite" => "RecWrite",
        "recSync" => "RecSync",
        "ptrRen" => "PtrRen",
        "ptrSync" => "PtrSync",
        "recClr" => "RecClr",
        "recClrSync" => "RecClrSync",
        "collRead" => "CollRead",
        "collSync" => "CollSync",
        "collRem" => "CollRem",
        "collFsync" => "CollFsync",
        "rvRecSync" => "RvRecSync",
        "rvRen" => "RvRen",
        "rvSync" => "RvSync",
        "rvClr" => "RvClr",
        "rvClrSync" => "RvClrSync",
        "rvCollRead" => "RvCollRead",
        "rvCollSync" => "RvCollSync",
        "rvCollRem" => "RvCollRem",
        "rvCollFsync" => "RvCollFsync",
        "rbCollRead" => "RbCollRead",
        "rbCollSync" => "RbCollSync",
        "rbCollRem" => "RbCollRem",
        "rbCollFsync" => "RbCollFsync",
        "txRen" => "TxRen",
        "txSync" => "TxSync",
        "rsRen" => "RsRen",
        "rsSync" => "RsSync",
        "dirSync" => "DirectorySync",
        "dirClear" => "DirectoryClear",
        "dirClearSync" => "DirectoryClearSync",
        _ => return None,
    };
    checkpoint(
        &Action {
            name,
            actor: None,
            document: 0,
            replica,
        },
        implementation,
    )
}

struct Replay<'trace> {
    trace: &'trace Trace,
    actions: Vec<Action>,
    implementation: Implementation,
    world: Arc<World>,
    mem: Arc<MemFs>,
    keeper: Option<Store>,
    observer_process: Arc<MemFs>,
    observer: Box<dyn Coordination>,
    processes: Vec<Option<Process>>,
    seen: contract::Projection,
}

impl<'trace> Replay<'trace> {
    fn start(trace: &'trace Trace, implementation: Implementation) -> Result<Self, String> {
        let world = Arc::new(World::of(trace));
        let (mem, keeper) = super::tests::initialized_with(&world, implementation);
        world.seed_directory_debt(&mem);
        let observer_process = Arc::new(mem.another_process());
        let observer = process::factory(implementation, None)
            .open(observer_process.clone(), &world.root)
            .map_err(|error| error.to_string())?;
        let seen = contract::observe(&mem, implementation, observer.as_ref());
        let initial = contract::model(&trace.states[0]);
        if let Some(error) = contract::differ(&initial, &initial, &seen, &seen) {
            return Err(error);
        }
        if projection::observe(&world, &mem)? != projection::model(&world, &trace.states[0]) {
            return Err("the initial documents differ from the model".to_owned());
        }
        let actions = trace
            .states
            .windows(2)
            .map(|states| action::infer(&states[0], &states[1]))
            .collect::<Result<Vec<_>, _>>()?;
        let participants = trace.states[0]["pc"]
            .as_array()
            .ok_or("pc is not a sequence")?
            .len();
        Ok(Self {
            trace,
            actions,
            implementation,
            world,
            mem,
            keeper: Some(keeper),
            observer_process,
            observer,
            processes: (0..participants).map(|_| None).collect(),
            seen,
        })
    }

    fn process(&self, actor: usize) -> Result<&Process, String> {
        self.processes[actor - 1]
            .as_ref()
            .ok_or_else(|| format!("participant {actor} has no process"))
    }

    fn check_before(&self, index: usize) -> Result<(), String> {
        let expected = projection::model(&self.world, &self.trace.states[index]);
        let observed = projection::observe(&self.world, &self.mem)?;
        if observed != expected {
            return Err(format!(
                "before the step: {observed:?}, expected {expected:?}"
            ));
        }
        let expected = contract::model(&self.trace.states[index]);
        let observed = contract::observe(&self.mem, self.implementation, self.observer.as_ref());
        if let Some(error) = contract::differ(&expected, &expected, &self.seen, &observed) {
            return Err(error);
        }
        Ok(())
    }

    fn check_after(&mut self, index: usize) -> Result<(), String> {
        let expected = projection::model(&self.world, &self.trace.states[index + 1]);
        let observed = projection::observe(&self.world, &self.mem)?;
        if observed != expected {
            return Err(format!(
                "after {}: {observed:?}, expected {expected:?}",
                self.actions[index].name
            ));
        }
        let previous = contract::model(&self.trace.states[index]);
        let expected = contract::model(&self.trace.states[index + 1]);
        let observed = contract::observe(&self.mem, self.implementation, self.observer.as_ref());
        if let Some(error) = contract::differ(&previous, &expected, &self.seen, &observed) {
            return Err(error);
        }
        self.seen = observed;
        Ok(())
    }

    fn plan(&self, index: usize, actor: usize) -> Command {
        let mut document = 1;
        let mut version = 0;
        let mut modification = false;
        let mut logged = false;
        let mut picked = false;
        let mut ingested = false;
        let mut empty = false;
        let mut quiesce_before = false;
        let mut quiesce_after = false;
        for (offset, action) in self.actions.iter().enumerate().skip(index + 1) {
            if action.actor != Some(actor) {
                continue;
            }
            match action.name {
                "Release" | "ProcessEnds" => break,
                "RecoveryBegin" => return Command::BeginRecovery,
                "RecoveryStep" => return Command::RecoveryUnit,
                "RecStart" => return Command::Recover,
                "Record" => {
                    version = self.trace.states[offset + 1]["mygen"][actor - 1]
                        .as_u64()
                        .unwrap_or_default() as usize
                }
                "BeginOpen" => modification = true,
                "Begin" => logged = true,
                "Pick" => {
                    document = action.document;
                    picked = true;
                }
                "IngRen" | "IngSync" => ingested = true,
                "EndEmpty" => empty = true,
                "Quiesce" if modification => quiesce_after = true,
                "Quiesce" => quiesce_before = true,
                _ => {}
            }
        }
        if empty && !ingested {
            Command::Empty
        } else if picked || logged {
            Command::BracketedWrite {
                document,
                version,
                logged,
                quiesce_before,
                quiesce_after,
            }
        } else if modification {
            Command::Empty
        } else if quiesce_before {
            Command::Quiesce
        } else {
            Command::Acquire
        }
    }

    fn next_stop(&self, index: usize, actor: usize) -> Option<Class> {
        let mut frontier = index + 1;
        for (position, action) in self.actions.iter().enumerate().skip(index + 1) {
            if action.name == "Crash" || action.actor == Some(actor) && action.name == "ProcessEnds"
            {
                break;
            }
            if action.actor != Some(actor) {
                continue;
            }
            if action.name == "Request" {
                break;
            }
            frontier = position + 1;
            if let Some(class) = self.failure_checkpoint(position) {
                return Some(class);
            }
            let observable = projection::model(&self.world, &self.trace.states[position])
                != projection::model(&self.world, &self.trace.states[position + 1])
                || contract::model(&self.trace.states[position])
                    != contract::model(&self.trace.states[position + 1]);
            if (observable || matches!(action.name, "TakeGateS" | "TakeReaderS" | "TakeWriterS"))
                && let Some(class) = checkpoint(action, self.implementation)
            {
                return Some(class);
            }
        }
        let state = &self.trace.states[frontier];
        let phase = state["pc"][actor - 1].as_str().unwrap_or_default();
        let replica = state["wix"].as_u64().unwrap_or(1) as usize;
        let next = match phase {
            "applied" | "ingested" => Some(Class::Document(Document::LogAppend)),
            "opening" | "open" | "wrote" | "closing" => Some(Class::Contract(Contract::Count)),
            "recClear" => Some(Class::Contract(Contract::ClearFence)),
            "closed" | "told" | "held" => Some(Class::Contract(Contract::ClearRecord)),
            "recEnd" | "recWhole" => Some(
                match state
                    .get("runPhase")
                    .and_then(|phases| phases.get(actor - 1))
                    .and_then(|phase| phase.as_str())
                {
                    Some("publication") => Class::Document(Document::RecordLook),
                    Some("directories") => Class::DirectoryRead,
                    _ => Class::Document(Document::LogRead),
                },
            ),
            "recSwap1" => Some(Class::Document(Document::PtrRename(0))),
            _ => None,
        };
        phase_checkpoint(
            state["wph"].as_str().unwrap_or_default(),
            replica,
            self.implementation,
        )
        .or(next)
    }

    fn failure_checkpoint(&self, index: usize) -> Option<Class> {
        let action = &self.actions[index];
        if action.name == "EndEmpty" {
            let actor = action.actor?;
            let ingested = self.actions[..index]
                .iter()
                .rev()
                .take_while(|previous| previous.actor != Some(actor) || previous.name != "Request")
                .any(|previous| {
                    previous.actor == Some(actor) && matches!(previous.name, "IngRen" | "IngSync")
                });
            if !ingested {
                return None;
            }
        } else if !matches!(
            action.name,
            "RecStop"
                | "FailUnresolved"
                | "PrepareFails"
                | "RollbackStart"
                | "RollbackCollectionFails"
                | "OpenFails"
                | "CloseFails"
        ) {
            return None;
        }
        if matches!(action.name, "OpenFails" | "CloseFails") {
            return Some(Class::Contract(Contract::Count));
        }
        if action.name == "PrepareFails" {
            return Some(Class::Document(Document::LogAppend));
        }
        let state = &self.trace.states[index];
        if action.name == "RecStop"
            && let Some(actor) = action.actor
            && (state["pc"][actor - 1] == "recCount"
                || state["pc"][actor - 1] == "recReset" && state["log"] == "empty")
        {
            return Some(Class::Contract(Contract::Count));
        }
        if let Some(class) = phase_checkpoint(
            state["wph"].as_str().unwrap_or_default(),
            action.replica,
            self.implementation,
        ) {
            return Some(class);
        }
        let actor = action.actor?;
        Some(
            match state
                .get("runPhase")
                .and_then(|phases| phases.get(actor - 1))
                .and_then(|phase| phase.as_str())
            {
                Some("directories") => Class::DirectoryRead,
                Some("publication") => Class::Document(Document::RecordLook),
                _ => Class::Document(Document::LogRead),
            },
        )
    }

    fn step(&mut self, index: usize) -> Result<(), String> {
        self.check_before(index)?;
        let action = &self.actions[index];
        let actor = action.actor.unwrap_or_default();
        match action.name {
            "BeginAttach" => {
                self.processes[actor - 1] = Some(Process::start(
                    &self.mem,
                    self.world.clone(),
                    self.implementation,
                    actor,
                ));
                self.process(actor)?
                    .command(Command::Open, Some(Class::Contract(Contract::Gate)))?;
            }
            "Request" => self.process(actor)?.command(
                self.plan(index, actor),
                Some(Class::Contract(Contract::Exclusion)),
            )?,
            "Exit" => self.process(actor)?.command(Command::Close, None)?,
            "ProcessEnds" => {
                if let Some(process) = &mut self.processes[actor - 1] {
                    process.end();
                }
                self.processes[actor - 1] = None;
            }
            "Crash" => {
                for process in self.processes.iter_mut().flatten() {
                    process.end();
                }
                self.processes
                    .iter_mut()
                    .for_each(|process| *process = None);
                if let Some(keeper) = self.keeper.take() {
                    keeper.abandon_for_replay();
                }
                self.observer.abandon();
                self.observer_process.abandon();
                if self.implementation == Implementation::File {
                    let after = &self.trace.states[index + 1];
                    contract::settle_crash(
                        &self.mem,
                        after["owner"].as_u64().unwrap_or_default() != 0,
                        after["fence"].as_bool().expect("the fence is boolean"),
                    );
                }
                self.mem.simulate_crash();
                self.observer_process = Arc::new(self.mem.another_process());
                self.observer = process::factory(self.implementation, None)
                    .open(self.observer_process.clone(), &self.world.root)
                    .map_err(|error| error.to_string())?;
                self.observer.trust().map_err(|error| error.to_string())?;
            }
            _ if self.failure_checkpoint(index).is_some() => {
                let class = self
                    .failure_checkpoint(index)
                    .expect("the step has a failure checkpoint");
                let process = self.process(actor)?;
                if process.paused() != Some(class) {
                    process.step(Some(class), false)?;
                    self.check_before(index)?;
                }
                process.step(self.next_stop(index, actor), true)?;
            }
            _ => {
                let changed = projection::model(&self.world, &self.trace.states[index])
                    != projection::model(&self.world, &self.trace.states[index + 1])
                    || contract::model(&self.trace.states[index])
                        != contract::model(&self.trace.states[index + 1]);
                let attachment = matches!(action.name, "TakeGateS" | "TakeReaderS" | "TakeWriterS");
                if let Some(class) = checkpoint(action, self.implementation)
                    && (changed || attachment)
                {
                    let process = self.process(actor)?;
                    if process.paused() != Some(class) {
                        if process.paused().is_none() {
                            return Err(format!(
                                "{} needs {class:?}, but the call returned {:?}",
                                action.name,
                                process.returned()
                            ));
                        }
                        process.step(Some(class), false)?;
                        self.check_before(index)?;
                    }
                    if process.paused() != Some(class) {
                        return Err(format!("{} did not reach {class:?}", action.name));
                    }
                    process.step(self.next_stop(index, actor), false)?;
                } else if changed {
                    return Err(format!(
                        "{} changes the model without a bound call",
                        action.name
                    ));
                }
            }
        }
        if action.name == "Release"
            && let Some(Reply::Recovery { phase, finished }) = self.process(actor)?.returned()
            && let Some(expected) = self.trace.states[index + 1]
                .get("runPhase")
                .and_then(|phases| phases.get(actor - 1))
                .and_then(|phase| phase.as_str())
        {
            let actual = if finished {
                "none"
            } else {
                match phase {
                    crate::RecoveryPhase::Publication => "publication",
                    crate::RecoveryPhase::DirectorySyncDebt => "directories",
                    crate::RecoveryPhase::Transactions => "transaction",
                    crate::RecoveryPhase::Collection => "collection",
                    crate::RecoveryPhase::Claims => "claims",
                    crate::RecoveryPhase::Completion => "completion",
                    _ => "other",
                }
            };
            if actual != expected {
                let debt = self
                    .mem
                    .read(&self.world.directory_record())
                    .unwrap_or_default();
                return Err(format!(
                    "recovery returned phase {phase:?} ({actual}), expected {expected}; directory debt {:?}",
                    String::from_utf8_lossy(&debt)
                ));
            }
        }
        self.check_after(index)
    }
}
