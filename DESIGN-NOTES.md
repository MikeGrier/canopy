# Design notes

Workspace-level decisions. Crate-specific decisions live in each crate's own notes:
[crates/canopy/DESIGN-NOTES.md](crates/canopy/DESIGN-NOTES.md) and
[crates/acadia/DESIGN-NOTES.md](crates/acadia/DESIGN-NOTES.md).

## Lockstep versioning

Every publishable crate (`canopy`, `acadia`) takes its version from
`[workspace.package]`, and `release-please` bumps that one value. `acadia` is
published because `canopy`'s public API takes an `Arc<dyn acadia::Vfs>`; its version
requirement in `[workspace.dependencies]` carries the `x-release-please-version`
marker so it moves with the release. Test-support crates (`komorebi`, `test-timers`,
`canopy-lock-holder`) are `publish = false` and keep version `0.0.0`.
