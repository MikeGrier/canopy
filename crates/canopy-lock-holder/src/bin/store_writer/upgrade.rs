// Copyright (c) Michael Grier.

use super::*;
use canopy::{MigrationUnit, SchemaLayer, SchemaUpgrade, Swap, SwapSet, UpgradeRequest};

pub(super) const UNITS: u64 = 3;
pub(super) const DOCUMENTS_PER_UNIT: usize = 64;

#[derive(Clone, Copy)]
pub(super) struct Plan {
    pub(super) source: u32,
    target: u32,
    floor: u32,
    units: u64,
}

impl Plan {
    pub(super) fn parse(rest: &[String], mode: &str) -> Result<Self, String> {
        let source = rest
            .get(2)
            .map(|value| value.parse::<u32>())
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or(if mode == "resume-upgrade" { 2 } else { 1 });
        let target = rest
            .get(3)
            .map(|value| value.parse::<u32>())
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or(source.saturating_add(1));
        let floor = rest
            .get(4)
            .map(|value| value.parse::<u32>())
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or(if mode == "upgrade-unsafe" {
                target
            } else {
                source
            });
        let units = rest
            .get(5)
            .map(|value| value.parse::<u64>())
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or(UNITS);
        Ok(Self {
            source,
            target,
            floor,
            units,
        })
    }
}

pub(super) fn unit(
    context: &MigrationUnit<'_>,
    node: &NodePath,
    prefix: &DocumentName,
    text: &str,
) -> Result<(), canopy::StoreError> {
    let content = DocumentBody::new(text.as_bytes().to_vec());
    let mut swaps = Vec::new();
    for index in 0..DOCUMENTS_PER_UNIT {
        let document = DocumentName::new(&format!(
            "{}-{}-{index}",
            prefix.as_str(),
            context.progress().next_unit
        ))
        .unwrap();
        if !matches!(context.read_document(node.clone(), &document)?, canopy::Read::Found(ref existing) if existing == &content)
        {
            swaps.push(Swap::new(node.clone(), document, content.clone()));
        }
    }
    if swaps.is_empty() {
        return Ok(());
    }
    let swaps = SwapSet::new(swaps).map_err(|_| canopy::StoreError::InvalidUpgrade {
        reason: "invalid fixture migration transaction",
    })?;
    context.apply_transaction(&swaps).map(|_| ())
}

pub(super) fn resume(
    store: &Store,
    node: &NodePath,
    document: &DocumentName,
    text: &str,
) -> Result<(), String> {
    let mut recovery = store.recovery().map_err(|error| error.to_string())?;
    while !recovery.progress().finished() {
        if recovery.progress().phase() == canopy::RecoveryPhase::UpgradeBody {
            checked(recovery.step_upgrade(|context| unit(context, node, document, text)))?;
        } else {
            checked(recovery.step())?;
        }
    }
    Ok(())
}

pub(super) fn run(
    store: &Store,
    node: &NodePath,
    document: &DocumentName,
    text: &str,
    plan: Plan,
) -> ExitCode {
    let mut upgrade: Option<SchemaUpgrade> = None;
    loop {
        let mut command = String::new();
        match std::io::stdin().lock().read_line(&mut command) {
            Ok(0) => return ExitCode::SUCCESS,
            Ok(_) => {}
            Err(error) => return fail(&error.to_string()),
        }
        let result = match command.trim() {
            "begin" => {
                if upgrade.is_some() {
                    return fail("an upgrade already exists");
                }
                match store.drain_upgrade().and_then(|drain| {
                    drain.begin_migration(
                        UpgradeRequest::new(
                            SchemaLayer::Agent,
                            plan.source,
                            plan.target,
                            plan.floor,
                        )?,
                        plan.units,
                    )
                }) {
                    Ok(started) => {
                        upgrade = Some(started);
                        Ok(())
                    }
                    Err(error) => Err(error.to_string()),
                }
            }
            "poll" => Ok(()),
            "step" => {
                let Some(upgrade) = &upgrade else {
                    return fail("begin before stepping");
                };
                checked(upgrade.step(|context| unit(context, node, document, text)))
                    .map(|progress| say(&format!("unit:{}", progress.next_unit)))
            }
            "fail-unit" => {
                let Some(upgrade) = &upgrade else {
                    return fail("begin before failing a unit");
                };
                match upgrade.step(|context| {
                    unit(context, node, document, text)?;
                    Err(canopy::StoreError::NoNonceSource)
                }) {
                    Err(canopy::StoreError::NoNonceSource) => {
                        say("body-failed");
                        return ExitCode::FAILURE;
                    }
                    Err(error) => return fail(&format!("unexpected body failure: {error}")),
                    Ok(_) => return fail("the injected body failure did not fail"),
                }
            }
            "complete" => {
                let Some(upgrade) = &upgrade else {
                    return fail("begin before completing");
                };
                checked(upgrade.complete()).map(|_| say("completed"))
            }
            _ => return fail("unknown upgrade command"),
        };
        if let Err(error) = result {
            say("upgrade-refused");
            return fail(&error);
        }
        if matches!(command.trim(), "begin" | "poll") {
            match upgrade.as_ref().expect("begin created the upgrade").poll() {
                Ok(true) => say("intent"),
                Ok(false) => say("pending"),
                Err(error) => {
                    say("upgrade-refused");
                    return fail(&error.to_string());
                }
            }
        }
    }
}
