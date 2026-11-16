// Copyright (c) Michael Grier.

//! What the Creation model's root variables say, read off the root's files.

use std::path::{Path, PathBuf};

use acadia::coord::file::FileFactory;
use acadia::{MemFs, Vfs};
use serde_json::{Map, Value};

/// The root the processes create.
pub(super) const ROOT: &str = "/store";

/// The reservation's name under the root, which both implementations use.
pub(super) const RESERVATION: &str = ".coord-reservation";

/// What `Intrude` puts under the root: a name creation does not make.
pub(super) const INTRUDER: &str = "intruder";

/// The reservation's encoding, as the root holds it. Changing any value is a breaking change to
/// the format, which both implementations write.
mod format {
    pub const MAGIC: [u8; 4] = *b"ACRV";
    pub const VERSION: u8 = 1;
    pub const NONCE_LEN: usize = 16;
    pub const INPUTS_LEN_WIDTH: usize = 8;
}

pub(super) fn root() -> &'static Path {
    Path::new(ROOT)
}

/// Artifact `k`, counting from 1; the last is the marker.
pub(super) fn artifact(k: usize) -> String {
    format!("a{k}")
}

pub(super) fn artifact_path(k: usize) -> PathBuf {
    root().join(artifact(k))
}

/// The model's number for an implementation: 1 is the file-based one, 2 the shared one.
fn model_implementation(id: &str) -> u64 {
    if id == FileFactory::IMPLEMENTATION.as_str() {
        1
    } else if id == acadia::coord::shared::IMPLEMENTATION.as_str() {
        2
    } else {
        0
    }
}

/// The model's reservation: `st` is `none`, `partial` or `whole`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Reservation {
    pub(super) st: String,
    pub(super) implementation: u64,
    pub(super) inputs: u64,
    pub(super) nonce: u64,
}

impl Reservation {
    pub(super) fn none() -> Reservation {
        Reservation {
            st: "none".to_owned(),
            implementation: 0,
            inputs: 0,
            nonce: 0,
        }
    }

    fn partial() -> Reservation {
        Reservation {
            st: "partial".to_owned(),
            ..Reservation::none()
        }
    }
}

/// The reservation `bytes` encode, or `None` for bytes no creator writes whole.
fn decode(bytes: &[u8]) -> Option<Reservation> {
    let mut rest = bytes;
    let mut take = |n: usize| {
        if rest.len() < n {
            return None;
        }
        let (head, tail) = rest.split_at(n);
        rest = tail;
        Some(head)
    };
    if take(format::MAGIC.len())? != format::MAGIC || take(1)? != [format::VERSION] {
        return None;
    }
    let id_len = usize::from(take(1)?[0]);
    let id = std::str::from_utf8(take(id_len)?).ok()?.to_owned();
    let nonce = take(format::NONCE_LEN)?;
    let nonce = nonce
        .iter()
        .all(|&byte| byte == nonce[0])
        .then(|| u64::from(nonce[0]))?;
    let len = u64::from_le_bytes(take(format::INPUTS_LEN_WIDTH)?.try_into().ok()?);
    let inputs = number(take(usize::try_from(len).ok()?)?)?;
    let whole = Reservation {
        st: "whole".to_owned(),
        implementation: model_implementation(&id),
        inputs,
        nonce,
    };
    rest.is_empty().then_some(whole)
}

/// The creation inputs a file holds: the model's number for them, written as text.
fn number(bytes: &[u8]) -> Option<u64> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

/// The model's root variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Projection {
    pub(super) res: Reservation,
    pub(super) rdur: bool,
    pub(super) aliv: Vec<bool>,
    /// The inputs each artifact was made from: 0 for none, and 99 for bytes no creator writes.
    pub(super) ainp: Vec<u64>,
    pub(super) adur: Vec<bool>,
    pub(super) foreign: bool,
}

fn field<'v>(state: &'v Map<String, Value>, name: &str) -> &'v Value {
    state
        .get(name)
        .unwrap_or_else(|| panic!("no variable {name}"))
}

fn flag(value: &Value) -> bool {
    value.as_bool().expect("a boolean")
}

fn count(value: &Value) -> u64 {
    value.as_u64().expect("a number")
}

fn list<T>(value: &Value, each: fn(&Value) -> T) -> Vec<T> {
    value
        .as_array()
        .expect("a sequence")
        .iter()
        .map(each)
        .collect()
}

/// The root variables of a model state.
pub(super) fn model(state: &Map<String, Value>) -> Projection {
    let res = field(state, "res");
    Projection {
        res: Reservation {
            st: res["st"].as_str().expect("a reservation state").to_owned(),
            implementation: count(&res["impl"]),
            inputs: count(&res["inp"]),
            nonce: count(&res["nonce"]),
        },
        rdur: flag(field(state, "rdur")),
        aliv: list(field(state, "aliv"), flag),
        ainp: list(field(state, "ainp"), count),
        adur: list(field(state, "adur"), flag),
        foreign: flag(field(state, "foreign")),
    }
}

/// What the root of `mem` holds now, with `artifacts` artifacts, read as the model's variables;
/// a file is durable if a crash now keeps it as it is.
pub(super) fn observe(mem: &MemFs, artifacts: usize) -> Projection {
    let durable = mem.fork();
    durable.simulate_crash();
    let reservation = root().join(RESERVATION);
    let mut projection = Projection {
        res: mem.read(&reservation).map_or_else(
            |_| Reservation::none(),
            |bytes| decode(&bytes).unwrap_or_else(Reservation::partial),
        ),
        rdur: durable
            .read(&reservation)
            .is_ok_and(|bytes| decode(&bytes).is_some()),
        aliv: Vec::with_capacity(artifacts),
        ainp: Vec::with_capacity(artifacts),
        adur: Vec::with_capacity(artifacts),
        foreign: mem
            .exists(&root().join(INTRUDER))
            .expect("the root is readable"),
    };
    for k in 1..=artifacts {
        let path = artifact_path(k);
        let live = mem.read(&path).ok();
        projection.aliv.push(live.is_some());
        projection.ainp.push(
            live.as_deref()
                .map_or(0, |bytes| number(bytes).unwrap_or(99)),
        );
        projection
            .adur
            .push(live.is_some() && durable.read(&path).ok() == live);
    }
    projection
}
