# canopy

A template repository for building and publishing a Rust crate on crates.io.

## What this template includes

- A Cargo workspace with one placeholder library crate at `crates/canopy`
- GitHub Actions CI for formatting, clippy, tests, MSRV checks, and CodeQL
- `release-please` automation for changelog, tags, and release PRs
- A publish workflow that pushes the crate to crates.io from `v*` tags
- Copilot instructions and lightweight planning/design placeholders

## Specialize this template

Before your first real release, replace the placeholder values below:

1. Replace the example author in `Cargo.toml`.
2. Replace the placeholder library code with your actual crate implementation.
3. Set the repository secrets required for releases:
   - `RELEASE_PLEASE_TOKEN`
   - `CARGO_REGISTRY_TOKEN`

## Build

Requires Rust `1.97` or newer.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

## Release

Merging conventional commits to `main` allows `release-please` to open or update
release PRs. Merging a release PR creates a `v<version>` tag, which triggers the
crates.io publish workflow.
