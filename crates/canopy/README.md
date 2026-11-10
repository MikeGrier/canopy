# canopy

A durable, replicated store for tree-structured documents.

canopy keeps a namespace of `/`-separated paths, mapped onto a directory hierarchy,
where each node holds named documents. Document bodies are opaque bytes: canopy
stores, replicates, and verifies them but never parses them. It is a library with no
transport, no protocol layer, no maintenance scheduler, and no clock of its own. `Store`
itself starts no threads; the optional `WriteMonitor` runs one writer thread that you spawn
explicitly.

## Features

- **Atomic writes.** `Store::update_document` replaces one document all-or-nothing;
  `Store::apply_transaction` applies a `SwapSet` across many documents
  all-or-nothing, journaled through an append-only commit log and replayed on open
  after a crash.
- **Compare-and-set.** `Swap::conditional` commits only if the current body has an
  expected `ContentDigest`; `Swap::conditional_absent` commits only if the document
  does not yet exist.
- **N-copy durability.** Content lives in content-addressed blobs and the namespace
  in separately replicated pointer trees. Reads take a quorum of pointer replicas and
  verify each blob against the digest in its own name, so a bad copy is identified
  rather than outvoted.
- **Self-repair.** Damage seen by reads is queued for repair, and
  `Store::scrub_content` walks every document and rebuilds bad replicas from a
  verified copy.
- **Single writer.** A store root is guarded by an OS-level exclusive lock, so a
  second process cannot open it while the first holds it, and the lock is released
  by the OS if the holder dies. Within a process, every write through any clone of a
  `Store` -- directly or through a `WriteMonitor` -- is serialized, and the monitor
  adds a read-modify-write `transform`; reads never take the lock.
- **Pluggable filesystem.** All I/O goes through the [`acadia`](https://crates.io/crates/acadia)
  `Vfs` trait, so the same store runs on the real filesystem (`OsFs`) or fully in
  memory with scripted fault injection (`MemFs`, `FaultyFs`) for tests.

## Example

```rust,no_run
use std::sync::Arc;

use acadia::{OsFs, Vfs};
use canopy::{
    DocumentBody, DocumentName, GenesisConfig, NodePath, Nonce, Read, ShireId, Store, Swap,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let vfs: Arc<dyn Vfs> = Arc::new(OsFs::new());
    let root = std::path::PathBuf::from("/var/lib/my-store"); // an existing, empty directory

    // Create the store once; afterwards reopen it with `Store::open(vfs, root)`.
    let created_at_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    let config = GenesisConfig::new(ShireId::new("example")?, created_at_unix_ms);
    let store = Store::init(Arc::clone(&vfs), root.clone(), config)?;

    // Write a document atomically. canopy mints no ids: the caller supplies the nonce.
    let node = NodePath::parse("projects/alpha")?;
    let name = DocumentName::new("settings.json")?;
    let body = DocumentBody::new(br#"{"theme":"dark"}"#.to_vec());
    let outcome = store.update_document(Swap::new(&node, name.clone(), body), &Nonce::new("txn-0001")?)?;
    assert!(outcome.is_committed());

    // Read it back, verified against its content digest.
    match store.read_document(&node, &name)? {
        Read::Found(body) => println!("{} bytes", body.as_bytes().len()),
        Read::Absent => println!("not there"),
        Read::Contended(_) => println!("a writer was busy; read again"),
    }

    // Scrub regularly: it is the only pass that finds and repairs unread bad replicas.
    let mut next = 0u64;
    let report = store.scrub_content(|| {
        next += 1;
        Nonce::new(&format!("scrub-{next}")).expect("valid nonce")
    })?;
    println!("scrub clean: {}", report.is_clean());
    Ok(())
}
```

## Names

Path components, document names, and nonces are validated and normalized: ASCII
letters (folded to uppercase), digits, `-` and `_`, with an interior `.` also allowed
in document names. There is no relative addressing: `.` and `..` are rejected, so a
path always names exactly one node.

## What the embedder must do

- **Schedule scrubs.** Reads only probe what they touch, and the repair backlog is
  bounded on purpose. A bad replica under a document nobody reads is rebuilt by
  `scrub_content` or by nothing, and every read keeps succeeding until the last good
  copy is gone. canopy has no scheduler, so choosing and keeping the cadence is the
  embedder's job.
- **Supply nonces.** canopy has no random number generator. Every write and
  transaction takes a caller-supplied `Nonce` that must be unique per operation; it
  names the commit-log records and staging files that crash recovery uses.
- **Retry contended reads.** `Read::Contended` means a writer was mid-update; it is
  not an error. `read_settled` and `read_within` provide the retry loop when you want
  one.

## Minimum supported Rust version

Rust 1.98.

## License

MIT
