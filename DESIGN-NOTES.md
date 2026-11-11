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

## Code comments are for the reader at HEAD

A code comment serves a reader long after the change that wrote it: someone holding the file at
its current revision, without the repository history, the review that prompted the code, or any
memory of its earlier form. So a comment states what is true of the code as it stands and what
that reader cannot see by reading it -- invariants, silent ordering and durability requirements,
constraints that live in another module, process or format, and the reason for an unobvious
algorithm or data structure -- and cites the design decision behind it where there is one.

How the code came to be this way is history, and history belongs to version control: the commit
message records the change and its motive, and the checklist archive records the work. A comment
that narrates a change makes the future reader rebuild a past state they never saw before they
can understand the present one, and it goes stale the moment the code moves again. The
actionable rules, including the check to run on every comment, are in
[.github/copilot-instructions.md](.github/copilot-instructions.md) under "Comments describe the
code as it is, never how it got there".
