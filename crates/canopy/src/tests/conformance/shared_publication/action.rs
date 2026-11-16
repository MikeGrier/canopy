// Copyright (c) Michael Grier.

use serde_json::{Map, Value};

const NO_HOLDER: usize = 0;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Action {
    pub(super) name: &'static str,
    pub(super) actor: Option<usize>,
    pub(super) document: usize,
    pub(super) replica: usize,
}

fn number(state: &Map<String, Value>, field: &str) -> Result<usize, String> {
    state
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| format!("{field} is not a model index"))
}

fn text<'state>(state: &'state Map<String, Value>, field: &str) -> Result<&'state str, String> {
    state
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{field} is not a model phase"))
}

fn changed_actor(
    before: &Map<String, Value>,
    after: &Map<String, Value>,
    field: &str,
) -> Result<Option<usize>, String> {
    let values = |state: &Map<String, Value>| -> Result<Vec<Value>, String> {
        state
            .get(field)
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| format!("{field} is not a model sequence"))
    };
    let previous = values(before)?;
    let next = values(after)?;
    if previous.len() != next.len() {
        return Err(format!("{field} changes its participant count"));
    }
    let mut changed = previous
        .iter()
        .zip(&next)
        .enumerate()
        .filter_map(|(index, (previous, next))| (previous != next).then_some(index + 1));
    let actor = changed.next();
    if changed.next().is_some() {
        return Err(format!("{field} changes more than one participant"));
    }
    Ok(actor)
}

pub(super) fn infer(
    before: &Map<String, Value>,
    after: &Map<String, Value>,
) -> Result<Action, String> {
    let document = number(before, "wdoc")?;
    let replica = number(before, "wix")?;
    if number(after, "crashes")? > number(before, "crashes")? {
        return Ok(Action {
            name: "Crash",
            actor: None,
            document,
            replica,
        });
    }
    let actor = changed_actor(before, after, "pc")?
        .or(changed_actor(before, after, "att")?)
        .or_else(|| {
            number(before, "lockHolder")
                .ok()
                .filter(|holder| *holder != NO_HOLDER)
        })
        .or_else(|| {
            number(after, "lockHolder")
                .ok()
                .filter(|holder| *holder != NO_HOLDER)
        });
    if before == after {
        return Ok(Action {
            name: "Terminal",
            actor,
            document,
            replica,
        });
    }
    let actor = actor.ok_or("a changing step has no participant")?;
    let phase_of = |state: &Map<String, Value>, field: &str| -> Result<String, String> {
        state
            .get(field)
            .and_then(Value::as_array)
            .and_then(|values| values.get(actor - 1))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("{field} has no phase for participant {actor}"))
    };
    let previous = phase_of(before, "pc")?;
    let next = phase_of(after, "pc")?;
    let work = text(before, "wph")?;
    let next_work = text(after, "wph")?;
    let completed_unit = before
        .get("runPhase")
        .and_then(|phases| phases.get(actor - 1))
        .and_then(Value::as_str)
        .is_some_and(|phase| {
            matches!(
                (phase, work),
                ("none", "unitStart")
                    | ("directories", "dirDone")
                    | ("publication", "rvDone")
                    | ("transaction", "unitTxnDone")
                    | ("collection", "txDone")
                    | ("claims", "claimsDone")
            )
        });
    let result = |name| {
        Ok(Action {
            name,
            actor: Some(actor),
            document,
            replica,
        })
    };
    if number(after, "deaths")? > number(before, "deaths")? {
        return result("ProcessEnds");
    }
    if previous == "open" && next == "wrote" {
        return result("EndEmpty");
    }
    if next == "told" && previous != "told" && previous != "locked" {
        return result(if previous.starts_with("rec") {
            if before.get("runPhase") != after.get("runPhase") || completed_unit {
                "RecoveryUnitEnd"
            } else {
                "RecStop"
            }
        } else if work.starts_with("rbColl") {
            "RollbackCollectionFails"
        } else if previous == "ingested" {
            "PrepareFails"
        } else if previous == "closing" {
            "CloseFails"
        } else {
            "FailUnresolved"
        });
    }
    if matches!(previous.as_str(), "held" | "told") && next.starts_with("rec") {
        return result(if next_work == "unitStart" {
            "RecoveryBegin"
        } else if before.contains_key("runPhase") {
            "RecoveryStep"
        } else {
            "RecStart"
        });
    }
    if work == "none" && next_work == "ingRen" {
        return Ok(Action {
            name: "Pick",
            actor: Some(actor),
            document: number(after, "wdoc")?,
            replica,
        });
    }
    if matches!(previous.as_str(), "begun" | "ingested")
        && matches!(next_work, "rbCollRead" | "rbDone")
    {
        return result("RollbackStart");
    }
    let work_action = match work {
        "ingRen" => Some("IngRen"),
        "ingSync" => Some("IngSync"),
        "recWrite" => Some("RecWrite"),
        "recSync" => Some("RecSync"),
        "ptrRen" => Some("PtrRen"),
        "ptrSync" => Some("PtrSync"),
        "recClr" => Some("RecClr"),
        "recClrSync" => Some("RecClrSync"),
        "collRead" => Some("CollRead"),
        "collSync" => Some("CollSync"),
        "collRem" => Some("CollRem"),
        "collFsync" => Some("CollFsync"),
        "rvRecSync" => Some("RvRecSync"),
        "rvRen" => Some("RvRen"),
        "rvSync" => Some("RvSync"),
        "rvClr" => Some("RvClr"),
        "rvClrSync" => Some("RvClrSync"),
        "rvCollRead" => Some("RvCollRead"),
        "rvCollSync" => Some("RvCollSync"),
        "rvCollRem" => Some("RvCollRem"),
        "rvCollFsync" => Some("RvCollFsync"),
        "rsRen" => Some("RsRen"),
        "rsSync" => Some("RsSync"),
        "txRen" => Some("TxRen"),
        "txSync" => Some("TxSync"),
        "rbCollRead" => Some("RbCollRead"),
        "rbCollSync" => Some("RbCollSync"),
        "rbCollRem" => Some("RbCollRem"),
        "rbCollFsync" => Some("RbCollFsync"),
        "dirSync" => Some("DirectorySync"),
        "dirClear" => Some("DirectoryClear"),
        "dirClearSync" => Some("DirectoryClearSync"),
        _ => None,
    };
    let document_changed = [
        "ptrLive", "ptrDur", "blobLive", "blobDur", "record", "wkeep", "wdoc", "wix",
    ]
    .iter()
    .any(|field| before.get(*field) != after.get(*field));
    if let Some(name) = work_action
        && number(before, "lockHolder")? == actor
        && (work != next_work || document_changed)
    {
        return result(name);
    }
    let name = match (previous.as_str(), next.as_str()) {
        ("new" | "detached", "attaching") => "BeginAttach",
        ("attaching", "idle") => "Admit",
        ("idle" | "detached", "exited") => "Exit",
        ("idle", "waiting") => "Request",
        ("waiting", "locked") => "Lock",
        ("locked", "held" | "told") => "Record",
        ("held", "opening") => "BeginOpen",
        ("opening", "open") => "EndOpen",
        ("opening", "closed") => "OpenFails",
        ("open", "begun") => "Begin",
        ("begun", "ingested") => "Ingest",
        ("ingested", "prepared") => "Prepare",
        ("applied", "ended") => "End",
        ("ended", "wrote") => "Sweep",
        ("open", "wrote") => "EndEmpty",
        ("begun" | "ingested", "wrote") => "RollbackBeforePrepare",
        ("wrote", "closing") => "BeginClose",
        ("closing", "closed") => "EndClose",
        ("held" | "closed" | "told", "releasing") => "Release",
        ("releasing", "idle") => "Unlock",
        ("recWhole", "recEnd") => "RecWhole",
        ("recSwap1", "recSwap2") => "RecSwap1",
        ("recSwap2", "recEnd") => "RecSwap2",
        ("recEnd", "recSweep" | "recReset") => "RecEnd",
        ("recSweep", "recCount" | "recReset") => "RecSweep",
        ("recReset", "recSweep" | "recCount") => "RecReset",
        ("recCount", "recClear") => "RecCount",
        ("recClear", "closed") => "RecClear",
        _ => match (
            phase_of(before, "att")?.as_str(),
            phase_of(after, "att")?.as_str(),
        ) {
            ("wGate", "hasGate") => "TakeGateS",
            ("hasGate", "gateRd") => "TakeReaderS",
            ("gateRd", "rd") => "DropGate",
            ("rd", "tiers") => "TakeWriterS",
            _ if before.get("fence") != after.get("fence") => "SetFence",
            _ if before.get("log") != after.get("log") => "Quiesce",
            _ if work == "none" && matches!(next_work, "rvRecSync" | "rsRen" | "rvDone") => {
                "RvBegin"
            }
            _ if work == "none" && next_work == "txRen" => "TxBegin",
            _ => {
                return Err(format!(
                    "unrecognized transition for {actor}: {previous}/{work} -> {next}/{next_work}"
                ));
            }
        },
    };
    result(name)
}
