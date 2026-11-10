# acadia

A specified filesystem contract, with a real backend, an in-memory backend that
models durability, and a deterministic fault injector.

acadia defines `Vfs`, the complete set of filesystem operations a storage engine
needs, and owns its semantics instead of inheriting whatever `std::fs` does on a
given platform. Code written against `Arc<dyn Vfs>` can run on disk in production
and fully in memory, with scripted failures, in tests. It is the filesystem layer
under [`canopy`](https://crates.io/crates/canopy).

## The contract

- **Path-oriented and object-safe.** Every operation names a path and returns an
  owned result, so a `Vfs` works as `Arc<dyn Vfs>` and can be shared across threads.
- **Errors are values.** Every fallible operation returns `VfsResult<T>`. Failures
  are reported as a `VfsError` with a crate-owned `VfsErrorKind` (for example
  `NotFound`), never as a panic.
- **Durability is explicit.** `write`, `append`, and `truncate` change contents but
  promise nothing about durability; bytes are durable only once `fsync` returns.
- **Rename is atomic, not durable.** `rename` replaces the destination atomically,
  and the rename itself becomes durable only after the containing directory is
  `fsync`'d.
- **Exclusive locks are owned.** `lock_exclusive` returns a guard whose lifetime is
  the lock, so the OS releases it if the owning process dies.

## Backends

| Type | Purpose |
|---|---|
| `OsFs` | Passthrough to the operating system; it maps `std::io::Error` onto `VfsErrorKind`. |
| `MemFs` | A complete in-memory filesystem with separate *live* and *durable* states. `fsync` of a file makes its bytes durable; `fsync` of a directory makes its entries durable. `simulate_crash` discards everything not yet durable. |
| `FaultyFs<V>` | Wraps any backend and applies a `FaultPolicy`: fail an operation, tear a write, corrupt a read, or observe a call, on chosen paths at chosen moments. No randomness -- every fault fires on a declared schedule. |

`read_root` additionally provides `ReadRoot`/`OsReadRoot`, read access anchored to a
directory handle for paths that come from untrusted input.

## Example

```rust
use std::path::Path;

use acadia::{Fault, FaultOp, FaultPolicy, FaultyFs, MemFs, Vfs, VfsErrorKind, When};

let fs = MemFs::new();
let dir = Path::new("/data");
let file = dir.join("record");
fs.create_dir_all(dir).unwrap();
fs.fsync(dir).unwrap(); // make the directory itself durable

// Written but not yet durable: a crash loses it.
fs.write(&file, b"v1").unwrap();
fs.simulate_crash();
assert!(!fs.exists(&file).unwrap());

// Durable once the file and its directory entry are fsync'd.
fs.write(&file, b"v1").unwrap();
fs.fsync(&file).unwrap();
fs.fsync(dir).unwrap();
fs.simulate_crash();
assert_eq!(fs.read(&file).unwrap(), b"v1");

// Scripted fault: the first fsync of `record` fails.
let policy = FaultPolicy::new().inject(
    FaultOp::Fsync,
    &file,
    Fault::Fail(VfsErrorKind::Other),
    When::Once,
);
let faulty = FaultyFs::new(fs, policy);
assert!(faulty.fsync(&file).is_err());
assert!(faulty.fsync(&file).is_ok());
```

Faults can also target paths by pattern (`FaultPolicy::inject_matching` with a
`PathMatcher`) and be held back until the test opens a `FaultGate`
(`FaultPolicy::inject_gated`), so setup runs clean and only the operation under test
fails.

## Platform notes

On Unix, `OsFs::lock_exclusive` uses `flock`; on Windows the exclusion comes from the
file's share mode.

## Minimum supported Rust version

Rust 1.98.

## License

MIT
