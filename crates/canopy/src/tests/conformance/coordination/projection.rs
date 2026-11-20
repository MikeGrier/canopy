// Copyright (c) Michael Grier.

//! What the coordination model's contract state says, read off each implementation.

use std::path::Path;

use acadia::coord::shared::inspect;
use acadia::coord::{Coordination, Trust};
use acadia::{MemFs, Vfs};
use serde_json::{Map, Value};

use super::process::Implementation;

/// The root the processes coordinate.
const ROOT: &str = if cfg!(windows) { "C:/store" } else { "/store" };

pub(in crate::tests::conformance) fn root() -> &'static Path {
    Path::new(ROOT)
}

/// The contract's state as the model has it. The ownership record is only present or absent:
/// each implementation names the holder by what the model does not, a process id or a
/// generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tests::conformance) struct Projection {
    pub(super) trusted: bool,
    pub(super) fence: bool,
    pub(super) owner: bool,
    pub(super) count_odd: bool,
    pub(super) generation: u64,
}

pub(in crate::tests::conformance) fn model(state: &Map<String, Value>) -> Projection {
    let number = |name: &str| state[name].as_u64().unwrap_or(0);
    let flag = |name: &str| state[name].as_bool().expect("a boolean");
    Projection {
        trusted: flag("trusted"),
        fence: flag("fence"),
        owner: number("owner") != 0,
        count_odd: !number("count").is_multiple_of(2),
        generation: number("gen"),
    }
}

/// The contract's state the implementation keeps under the root of `mem`, read through
/// `observer`, a participant of the file-based implementation that takes nothing.
pub(in crate::tests::conformance) fn observe(
    mem: &MemFs,
    implementation: Implementation,
    observer: &dyn Coordination,
) -> Projection {
    match implementation {
        Implementation::File => {
            let state = root().join(".coord");
            let exists = |name: &str| {
                mem.exists(&state.join(name))
                    .expect("the state is readable")
            };
            let generation = mem
                .read(&state.join("generation"))
                .ok()
                .and_then(|bytes| {
                    let at = bytes.len().checked_sub(size_of::<u64>())?;
                    Some(u64::from_le_bytes(bytes[at..].try_into().ok()?))
                })
                .unwrap_or(0);
            Projection {
                trusted: observer.trust().expect("the boot record is readable") == Trust::Vouched,
                fence: exists("fence"),
                owner: exists("owner"),
                count_odd: !observer
                    .sample()
                    .expect("the count is readable")
                    .get()
                    .is_multiple_of(2),
                generation,
            }
        }
        Implementation::Shared => match inspect(mem, root()).expect("the words are readable") {
            Some(words) => Projection {
                trusted: words.trust == Trust::Vouched,
                fence: words.fence,
                owner: words.owner != 0,
                count_odd: !words.count.is_multiple_of(2),
                generation: words.generation,
            },
            None => Projection {
                trusted: false,
                fence: true,
                owner: false,
                count_odd: true,
                generation: 0,
            },
        },
    }
}

/// Where the code's state after a step is not the model's, given both before it. State that
/// cannot be vouched for is compared only as that: the contract tells every acquirer of it that
/// recovery is required, whatever its record, fence and count say. The generation is compared by
/// whether it advanced, since each implementation numbers it differently.
pub(in crate::tests::conformance) fn differ(
    model_before: &Projection,
    model: &Projection,
    code_before: &Projection,
    code: &Projection,
) -> Option<String> {
    if model.trusted != code.trusted {
        return Some(format!(
            "the state is {} where the model's is {}",
            trusted(code.trusted),
            trusted(model.trusted)
        ));
    }
    if !model.trusted {
        return None;
    }
    let mut differences = Vec::new();
    if model.fence != code.fence {
        differences.push(format!(
            "the fence is {} where the model's is {}",
            model_flag(code.fence),
            model_flag(model.fence)
        ));
    }
    if model.owner != code.owner {
        differences.push(format!(
            "the ownership record is {} where the model's is {}",
            present(code.owner),
            present(model.owner)
        ));
    }
    if model.count_odd != code.count_odd {
        differences.push(format!(
            "the count is {} where the model's is {}",
            parity(code.count_odd),
            parity(model.count_odd)
        ));
    }
    let model_advanced = model.generation != model_before.generation;
    let code_advanced = code.generation != code_before.generation;
    if model_before.trusted && model_advanced != code_advanced {
        differences.push(format!(
            "the generation {} where the model's {}",
            advanced(code_advanced),
            advanced(model_advanced)
        ));
    }
    (!differences.is_empty()).then(|| differences.join("; "))
}

fn trusted(flag: bool) -> &'static str {
    if flag { "trusted" } else { "untrusted" }
}

fn model_flag(flag: bool) -> &'static str {
    if flag { "set" } else { "clear" }
}

fn present(flag: bool) -> &'static str {
    if flag { "present" } else { "absent" }
}

fn parity(odd: bool) -> &'static str {
    if odd { "odd" } else { "even" }
}

fn advanced(flag: bool) -> &'static str {
    if flag { "advanced" } else { "stayed" }
}

/// Makes what the model's crash keeps of a file-based record durable, and removes what it loses,
/// before the power goes: the model chooses which survive.
pub(in crate::tests::conformance) fn settle_crash(mem: &MemFs, kept_owner: bool, kept_fence: bool) {
    let state = root().join(".coord");
    for (name, kept) in [("owner", kept_owner), ("fence", kept_fence)] {
        let path = state.join(name);
        if !mem.exists(&path).expect("the state is readable") {
            continue;
        }
        if kept {
            mem.fsync(&path).expect("a record is made durable");
        } else {
            mem.remove_file(&path).expect("a record is removed");
        }
    }
    if mem.exists(&state).expect("the state is readable") {
        mem.fsync(&state)
            .expect("the state's names are made durable");
    }
}
