# canopy

A durable, replicated store for tree-structured documents.

canopy keeps a namespace of `/`-separated paths, mapped onto a directory hierarchy,
where each node holds named documents. Document bodies are opaque bytes: canopy
stores, replicates, and verifies them but never parses them. It is a library with no
transport, no protocol layer, no maintenance scheduler, and no clock of its own, and it
starts no threads.

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
  `Store` is serialized, which is what makes the read-modify-write `Store::transform`
  and `Store::plan_and_apply` lose no update; reads never take the lock, and
  `Store::snapshot` reads a state no write disturbed without excluding writers.
- **Pluggable filesystem.** All I/O goes through the [`acadia`](https://crates.io/crates/acadia)
  `Vfs` trait, so the same store runs on the real filesystem (`OsFs`) or fully in
  memory with scripted fault injection (`MemFs`, `FaultyFs`) for tests.

## Local filesystems only

**The store root must be on a local filesystem of the host that opens it. Network
filesystems are not supported** -- SMB/CIFS shares (including mapped network drives), NFS
mounts, and other remote or distributed filesystems. canopy's guarantees rest on the local
operating system's lock, rename and fsync behaviour, which network filesystems do not reliably
provide. canopy does not detect or refuse a networked root: one may appear to work while
losing writes or corrupting the store.

## Example

```rust,no_run
use std::sync::Arc;

use acadia::{OsFs, Vfs};
use canopy::{
    DocumentBody, DocumentName, GenesisConfig, NodePath, Nonce, Read, ShireId, Store,
    StoreOptions, Swap,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let vfs: Arc<dyn Vfs> = Arc::new(OsFs::new());
    let root = std::path::PathBuf::from("/var/lib/my-store"); // an existing, empty directory

    // Every write draws a nonce from the source the store is opened with. The
    // `random-nonce` feature provides `canopy::random_nonce_source()`; any source whose
    // nonces are unique per operation will do.
    let mut next = 0u64;
    let options = StoreOptions::with_nonce_source(move || {
        next += 1;
        Nonce::new(&format!("op-{next}")).expect("valid nonce")
    });

    // Create the store once; afterwards reopen it with `Store::open(vfs, root, options)`.
    let created_at_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    let config = GenesisConfig::new(ShireId::new("example")?, created_at_unix_ms);
    let store = Store::init(Arc::clone(&vfs), root.clone(), config, options)?;

    // Write a document atomically.
    let node = NodePath::parse("projects/alpha")?;
    let name = DocumentName::new("settings.json")?;
    let body = DocumentBody::new(br#"{"theme":"dark"}"#.to_vec());
    let commit = store.update_document(Swap::new(&node, name.clone(), body))?;
    assert!(commit.is_committed());

    // Read it back, verified against its content digest.
    match store.read_document(&node, &name)? {
        Read::Found(body) => println!("{} bytes", body.as_bytes().len()),
        Read::Absent => println!("not there"),
        Read::Contended(_) => println!("a writer was busy; read again"),
    }

    // Scrub regularly: it is the only pass that finds and repairs unread bad replicas.
    let (report, _nonces) = store.scrub_content()?;
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
- **Supply a nonce source.** Every write draws a `Nonce` from the source passed in
  `StoreOptions`; it names the commit-log records and staging files that crash recovery
  uses. Nonces need only be statistically unique, and a collision is detected and fails
  the operation rather than corrupting anything. Enable the `random-nonce` feature for a
  ready-made 128-bit random source.
- **Drain the repair backlog.** `Store::repair_reported_damage` rebuilds what reads have
  found damaged. canopy runs no thread to do it, so call it when the store is idle.
- **Retry contended reads.** `Read::Contended` means a writer was mid-update; it is
  not an error. `read_settled` and `read_within` provide the retry loop when you want
  one.

## Minimum supported Rust version

Rust 1.98.

## License

MIT
