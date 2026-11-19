// Copyright (c) Michael Grier.

//! Driving the processes through a Creation trace, one step at a time.

use acadia::{MemFs, Vfs};
use serde_json::{Map, Value};

use super::super::corpus::Trace;
use super::process::{Class, Part, Process, expected};
use super::projection::{self, INTRUDER, RESERVATION, artifact_path, root};

/// Where the code left a trace, and how.
#[derive(Debug)]
pub(super) struct Departure {
    /// The step after which the code's root, or a process's ending, is not the model's.
    pub(super) step: usize,
    pub(super) detail: String,
}

/// Follows `trace` with the code: every step of a process is that process's next stretch of
/// calls, and after each step the root must be the model's.
pub(super) fn replay(trace: &Trace) -> Result<(), Departure> {
    let mut replay = Replay::new(trace);
    let initial = projection::observe(&replay.mem, replay.artifacts);
    assert_eq!(
        initial,
        projection::model(&trace.states[0]),
        "{}: the root does not start as the model's",
        trace.file
    );
    for k in 0..trace.steps.len() {
        replay
            .step(k)
            .map_err(|detail| Departure { step: k, detail })?;
    }
    Ok(())
}

struct Replay<'t> {
    trace: &'t Trace,
    mem: MemFs,
    artifacts: usize,
    parts: Vec<Part>,
    /// Each process once it has started, by its number less one.
    processes: Vec<Option<Process>>,
}

/// The scenario's sequences, by process.
fn scenario<'v>(trace: &'v Trace, name: &str) -> &'v [Value] {
    trace.derived["scenario"][name]
        .as_array()
        .unwrap_or_else(|| panic!("{}: the scenario has no {name}", trace.file))
}

fn number(value: &Value) -> u64 {
    value.as_u64().expect("a number")
}

/// Process `p`'s value of the model variable `name` in `state`.
fn of<'s>(state: &'s Map<String, Value>, name: &str, p: usize) -> &'s Value {
    &state[name][p - 1]
}

impl<'t> Replay<'t> {
    fn new(trace: &'t Trace) -> Replay<'t> {
        let roles = scenario(trace, "roles");
        let impls = scenario(trace, "impls");
        let ins = scenario(trace, "ins");
        let parts: Vec<Part> = (0..roles.len())
            .map(|i| Part {
                number: i as u64 + 1,
                creator: roles[i] == "creator",
                implementation: number(&impls[i]),
                inputs: number(&ins[i]),
            })
            .collect();
        let mem = MemFs::new();
        mem.create_dir_all(root()).expect("the root is made");
        mem.fsync(root()).expect("the root is durable");
        Replay {
            trace,
            mem,
            artifacts: trace.constant("NArt").parse().expect("NArt is a number"),
            processes: (0..parts.len()).map(|_| None).collect(),
            parts,
        }
    }

    fn step(&mut self, k: usize) -> Result<(), String> {
        let trace = self.trace;
        let step = &trace.steps[k];
        let after = &trace.states[k + 1];
        match step.name.as_str() {
            "Terminate" => self.terminate(process(&step.args)),
            "Crash" => self.crash(after)?,
            "Intrude" => self
                .mem
                .write(&root().join(INTRUDER), b"intruder")
                .map_err(|error| format!("the intrusion failed: {error}"))?,
            "Terminal" => {}
            _ => {
                let p = process(&step.args);
                self.advance(p, k)
                    .map_err(|detail| format!("process {p}: {detail}"))?;
            }
        }
        let code = projection::observe(&self.mem, self.artifacts);
        let model = projection::model(after);
        if code != model {
            return Err(format!(
                "the root holds {code:?} where the model's holds {model:?}"
            ));
        }
        Ok(())
    }

    /// Lets process `p` take step `k`, starting it at its first.
    fn advance(&mut self, p: usize, k: usize) -> Result<(), String> {
        let trace = self.trace;
        let before = &trace.states[k];
        let after = &trace.states[k + 1];
        let part = &self.parts[p - 1];
        let slot = &mut self.processes[p - 1];
        let process = match slot {
            Some(process) => process,
            None => {
                let first = stop(before, p, part).expect("a process starts from \"new\"");
                slot.insert(Process::start(&self.mem, part, self.artifacts, first)?)
            }
        };
        process.step(stop(after, p, part))?;
        let model = of(after, "out", p).as_str().expect("an outcome");
        match (of(after, "pc", p) == "done", process.ended()) {
            (true, Some(code)) if code == expected(model) => Ok(()),
            (true, Some(code)) => Err(format!("it ended {code} where the model's ended {model}")),
            (true, None) => Err(format!("it went on where the model's ended {model}")),
            (false, Some(code)) => Err(format!("it ended {code} where the model's goes on")),
            (false, None) => Ok(()),
        }
    }

    fn terminate(&mut self, p: usize) {
        let process = self.processes[p - 1]
            .as_mut()
            .expect("a process dies once it has started");
        process.condemn();
        process.join();
    }

    /// Kills every process, then loses what the model's crash loses, and makes what it keeps
    /// durable, before the power goes: the model chooses which of the files that are not durable
    /// survive.
    fn crash(&mut self, after: &Map<String, Value>) -> Result<(), String> {
        for process in self.processes.iter_mut().flatten() {
            process.condemn();
            process.join();
        }
        let kept = projection::model(after);
        self.survive(&root().join(RESERVATION), kept.res.st != "none")?;
        for k in 1..=self.artifacts {
            self.survive(&artifact_path(k), kept.aliv[k - 1])?;
        }
        self.survive(&root().join(INTRUDER), kept.foreign)?;
        self.mem
            .fsync(root())
            .map_err(|error| format!("the crash's sync failed: {error}"))?;
        self.mem = self.mem.simulate_restart();
        Ok(())
    }

    fn survive(&self, path: &std::path::Path, kept: bool) -> Result<(), String> {
        let exists = self.mem.exists(path).expect("the root is readable");
        let made = match (exists, kept) {
            (true, true) => self.mem.fsync(path),
            (true, false) => self.mem.remove_file(path),
            (false, true) => {
                return Err(format!(
                    "the model's crash keeps {}, which the root does not hold",
                    path.display()
                ));
            }
            (false, false) => Ok(()),
        };
        made.map_err(|error| format!("the crash could not settle {}: {error}", path.display()))
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
    }
}

/// The process a step's first argument names.
fn process(args: &[Value]) -> usize {
    args.first()
        .and_then(Value::as_u64)
        .and_then(|p| usize::try_from(p).ok())
        .expect("a process's step names the process")
}

/// The class of call the step process `p` takes next from `state` begins with, or `None` when it
/// takes none.
fn stop(state: &Map<String, Value>, p: usize, part: &Part) -> Option<Class> {
    match of(state, "pc", p).as_str().expect("a process state") {
        "new" if part.creator => Some(Class::LookRoot),
        "new" | "wantX" => Some(Class::CreationLock),
        "holdsX" => Some(Class::LookRoot),
        "syncRes" if of(state, "path", p) == "resume" => Some(Class::SyncReservation),
        "syncRes" | "syncArt" => Some(Class::SyncRoot),
        "writeArt" => Some(Class::Artifact(
            of(state, "ak", p)
                .as_u64()
                .and_then(|k| usize::try_from(k).ok())
                .expect("an artifact"),
        )),
        _ => None,
    }
}
