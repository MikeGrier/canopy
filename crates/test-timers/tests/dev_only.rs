// Copyright (c) Michael Grier.

//! `test-timers` is a dev-dependency and nothing else (`D485`).
//!
//! A crate that took it as a normal dependency would have, in its production code, a way to reach a
//! timer that carries no reason of its own, which is the one thing `D484` exists to prevent. This
//! test asks Cargo itself -- `cargo metadata` -- for every workspace member's dependencies, and
//! refuses any edge onto `test-timers` whose kind is not `dev`.
//!
//! Cargo is asked rather than the manifests being scanned, because Cargo is what decides what a
//! manifest means: renames (`package = "test-timers"`), `workspace = true` inheritance, dotted keys,
//! quoting and target tables are all resolved by it, and a text scanner would have to re-implement
//! every one of them to be trusted.

use std::process::Command;

use serde_json::Value;

const NAME: &str = "test-timers";

/// Every dependency edge in `metadata` onto `test-timers` that is not a dev-dependency, as
/// `"<package> (<kind>[, renamed <alias>])"`.
fn misplaced(metadata: &Value) -> Vec<String> {
    let mut found = Vec::new();
    for package in metadata["packages"].as_array().into_iter().flatten() {
        let owner = package["name"].as_str().unwrap_or("<unnamed>");
        for dependency in package["dependencies"].as_array().into_iter().flatten() {
            // `name` is the package depended on, even when the manifest renames it.
            if dependency["name"].as_str() != Some(NAME) {
                continue;
            }
            // Cargo reports a normal dependency's kind as `null`.
            let kind = dependency["kind"].as_str().unwrap_or("normal");
            if kind == "dev" {
                continue;
            }
            match dependency["rename"].as_str() {
                Some(alias) => found.push(format!("{owner} ({kind}, renamed {alias})")),
                None => found.push(format!("{owner} ({kind})")),
            }
        }
    }
    found
}

#[test]
fn no_workspace_crate_takes_it_as_a_normal_dependency() {
    let output = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout).expect("cargo metadata JSON");

    // The check is only worth anything if it saw the workspace; a metadata call that resolved
    // some other manifest would pass vacuously.
    let members = metadata["packages"].as_array().map_or(0, Vec::len);
    assert!(
        members > 1,
        "cargo metadata saw {members} workspace packages"
    );

    let offences = misplaced(&metadata);
    assert!(
        offences.is_empty(),
        "test-timers is a dev-dependency only (D485): {offences:#?}"
    );
}

/// A package `owner` whose single dependency edge onto `name` has `kind` and `rename`.
fn one_edge(owner: &str, name: &str, kind: Value, rename: Value) -> Value {
    serde_json::json!({
        "packages": [{
            "name": owner,
            "dependencies": [{ "name": name, "kind": kind, "rename": rename }],
        }],
    })
}

#[test]
fn a_dev_dependency_is_accepted() {
    let metadata = one_edge("x", NAME, Value::from("dev"), Value::Null);
    assert!(misplaced(&metadata).is_empty());
}

#[test]
fn a_renamed_dev_dependency_is_accepted() {
    let metadata = one_edge("x", NAME, Value::from("dev"), Value::from("timers"));
    assert!(misplaced(&metadata).is_empty());
}

#[test]
fn a_normal_dependency_is_refused() {
    let metadata = one_edge("x", NAME, Value::Null, Value::Null);
    assert_eq!(misplaced(&metadata), ["x (normal)"]);
}

#[test]
fn a_build_dependency_is_refused() {
    let metadata = one_edge("x", NAME, Value::from("build"), Value::Null);
    assert_eq!(misplaced(&metadata), ["x (build)"]);
}

#[test]
fn a_renamed_normal_dependency_is_refused_and_named() {
    let metadata = one_edge("x", NAME, Value::Null, Value::from("timers"));
    assert_eq!(misplaced(&metadata), ["x (normal, renamed timers)"]);
}

#[test]
fn a_similarly_named_crate_is_not_mistaken_for_it() {
    let metadata = one_edge("x", "test-timers-extra", Value::Null, Value::Null);
    assert!(misplaced(&metadata).is_empty());
}

#[test]
fn a_rename_onto_its_name_from_another_crate_is_not_mistaken_for_it() {
    // An alias of `test-timers` for some other package is not a dependency on this crate.
    let metadata = one_edge("x", "other", Value::Null, Value::from(NAME));
    assert!(misplaced(&metadata).is_empty());
}

#[test]
fn every_offending_package_is_reported() {
    let metadata = serde_json::json!({
        "packages": [
            { "name": "a", "dependencies": [{ "name": NAME, "kind": null }] },
            { "name": "b", "dependencies": [{ "name": NAME, "kind": "dev" }] },
            { "name": "c", "dependencies": [{ "name": NAME, "kind": "build" }] },
        ],
    });
    assert_eq!(misplaced(&metadata), ["a (normal)", "c (build)"]);
}

#[test]
fn metadata_without_packages_reports_nothing() {
    assert!(misplaced(&serde_json::json!({})).is_empty());
}

#[test]
fn a_package_listing_it_twice_reports_each_non_dev_edge() {
    // A crate may list it under both a normal and a dev table, e.g. per target.
    let metadata = serde_json::json!({
        "packages": [{
            "name": "x",
            "dependencies": [
                { "name": NAME, "kind": "dev" },
                { "name": NAME, "kind": null },
            ],
        }],
    });
    assert_eq!(misplaced(&metadata), ["x (normal)"]);
}
