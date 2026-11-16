// Copyright (c) Michael Grier.

//! Reading the trace corpus: one file per behaviour, in the format
//! `tools/export-trace-corpus.ps1` writes (DESIGN-NOTES "The trace corpus").

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// One action of a trace.
#[derive(Clone)]
pub(super) struct Step {
    /// The timeline's step id, `before`, or `#n` for the n-th action of a behaviour without ids.
    pub(super) id: String,
    /// The action as TLA+ text.
    pub(super) action: String,
    /// The action's operator.
    pub(super) name: String,
    /// The action's arguments: a number or a string as JSON, anything else as `{"expr": text}`.
    pub(super) args: Vec<Value>,
}

/// One behaviour of a spec, with every state it passes through.
#[derive(Clone)]
pub(super) struct Trace {
    /// The corpus file, relative to the corpus root.
    pub(super) file: String,
    /// The configuration the trace was read from.
    pub(super) configuration: String,
    /// The configuration's constants, as TLA+ text.
    pub(super) constants: BTreeMap<String, String>,
    /// The values the corpus derives from the configuration, as JSON; empty for a spec with none.
    pub(super) derived: Map<String, Value>,
    /// The step at which the code as built is expected to leave the behaviour.
    pub(super) departs: Option<String>,
    /// `states[0]` is the initial state and `states[k + 1]` the state after `steps[k]`.
    pub(super) states: Vec<Map<String, Value>>,
    pub(super) steps: Vec<Step>,
}

impl Trace {
    /// A constant of the trace's configuration, as TLA+ text.
    pub(super) fn constant(&self, name: &str) -> &str {
        self.constants
            .get(name)
            .unwrap_or_else(|| panic!("{}: no constant {name}", self.file))
    }

    /// Where step `k` is, for a report.
    pub(super) fn at(&self, k: usize) -> String {
        let step = &self.steps[k];
        format!("step {} (action {}, {})", step.id, k + 1, step.action)
    }
}

/// The corpus root.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("traces")
}

/// Every behaviour of `spec` in the corpus, in file-name order.
pub(super) fn load(spec: &str) -> Vec<Trace> {
    let dir = root().join(spec);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        .map(|entry| entry.expect("a corpus directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    files.iter().map(|path| parse(spec, path)).collect()
}

pub(super) fn load_one(spec: &str, file: &str) -> Trace {
    parse(spec, &root().join(spec).join(file))
}

fn parse(spec: &str, path: &Path) -> Trace {
    let file = format!(
        "{spec}/{}",
        path.file_name()
            .expect("a corpus file has a name")
            .to_string_lossy()
    );
    let text = std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{file}: {error}"));
    let json: Value = serde_json::from_str(&text).unwrap_or_else(|error| panic!("{file}: {error}"));
    assert_eq!(json["format"], 1, "{file}: unknown corpus format");
    assert_eq!(json["spec"], spec, "{file}: a trace of another spec");

    let constants = json["constants"]
        .as_object()
        .unwrap_or_else(|| panic!("{file}: no constants"))
        .iter()
        .map(|(name, value)| {
            let text = value.as_str().unwrap_or_else(|| panic!("{file}: {name}"));
            (name.clone(), text.to_owned())
        })
        .collect();
    let configuration = json["configuration"]
        .as_str()
        .unwrap_or_else(|| panic!("{file}: no configuration"))
        .to_owned();
    let derived = json["derived"].as_object().cloned().unwrap_or_default();
    let departs = json["departs"].as_str().map(str::to_owned);

    let mut state = json["initial"]
        .as_object()
        .unwrap_or_else(|| panic!("{file}: no initial state"))
        .clone();
    let mut states = vec![state.clone()];
    let mut steps = Vec::new();
    for step in json["steps"]
        .as_array()
        .unwrap_or_else(|| panic!("{file}: no steps"))
    {
        let text = |field: &str| {
            step[field]
                .as_str()
                .unwrap_or_else(|| panic!("{file}: a step without {field}"))
                .to_owned()
        };
        for (name, value) in step["changes"]
            .as_object()
            .unwrap_or_else(|| panic!("{file}: a step without changes"))
        {
            assert!(state.contains_key(name), "{file}: {name} is no variable");
            state.insert(name.clone(), value.clone());
        }
        states.push(state.clone());
        steps.push(Step {
            id: text("step"),
            action: text("action"),
            name: text("name"),
            args: step["args"].as_array().cloned().unwrap_or_default(),
        });
    }
    if let Some(departs) = &departs {
        assert!(
            steps.iter().any(|step| &step.id == departs),
            "{file}: departs at {departs}, which is no step"
        );
    }
    Trace {
        file,
        configuration,
        constants,
        derived,
        departs,
        states,
        steps,
    }
}
