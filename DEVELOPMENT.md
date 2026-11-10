# Development

## Commands

Run the standard checks from the workspace root:

```sh
cargo fmt --all --check
cargo build --workspace --all-targets --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

The toolchain is pinned by [rust-toolchain.toml](rust-toolchain.toml); `rustup`
installs it on first use.

## Release setup

Before the first release:

1. Create the `RELEASE_PLEASE_TOKEN` repository secret so release tags trigger
   downstream workflows.
2. Create the `CARGO_REGISTRY_TOKEN` repository secret with crates.io publish
   permission.
