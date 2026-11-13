# canopy

A durable, replicated store for tree-structured documents, written in Rust.

## Crates

| Crate | Published | Purpose |
|---|---|---|
| [`canopy`](crates/canopy/README.md) | yes | The document store: atomic writes and transactions, quorum reads, self-repair. |
| [`acadia`](crates/acadia/README.md) | yes | The filesystem contract canopy runs on, with OS, in-memory, and fault-injecting backends. |
| [`komorebi`](crates/komorebi/README.md) | no | Logging setup and test-time event capture. |
| [`test-timers`](crates/test-timers/README.md) | no | The one sanctioned way for tests to reach a timer. |
| [`canopy-lock-holder`](crates/canopy-lock-holder/README.md) | no | Test fixture for canopy's cross-process root lock. |

## Build

Rust `1.98`, pinned by [rust-toolchain.toml](rust-toolchain.toml).

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

## Release

Merging conventional commits to `main` lets `release-please` open or update a release
PR. Publishable crates share the workspace version and are released in lockstep:
merging a release PR creates a `v<version>` tag, which triggers the crates.io publish
workflow to publish every member not marked `publish = false`. `acadia` is published
because `canopy`'s public API exposes its `Vfs` trait.

Releases need two repository secrets: `RELEASE_PLEASE_TOKEN` and
`CARGO_REGISTRY_TOKEN`.

## License

MIT -- see [LICENSE](LICENSE).
