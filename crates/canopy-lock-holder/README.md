# canopy-lock-holder

A test fixture that holds a `canopy` store root's lock until it is killed.

**Internal crate, not published.**

## Why it exists

A `canopy` store root has one writer, enforced across processes by an OS exclusive
lock (D116). The property that matters is what the **operating system** does when
the lock's owner dies without running any destructor -- and only a separate process
that is killed outright can show that. In-process unit tests over `MemFs` cannot.

The fixture has to be a real binary so Cargo exposes its path to integration tests
(`CARGO_BIN_EXE_canopy-lock-holder`). It lives in its own crate so that `canopy`
stays library-only and this binary never ships with it.

## The binary

```text
canopy-lock-holder <root>
```

`<root>` must be an initialized store whose lock is free. On success the process
prints `locked`, flushes stdout, and sleeps until it is killed. Any failure is
reported on stderr with a non-zero exit, so a caller never mistakes "could not
acquire" for "acquired and waiting".

## Tests

`tests/root_lock.rs` spawns the binary over a real directory using `OsFs` and checks
that:

- a live holder in another process makes `Store::open` fail with
  `StoreError::RootInUse`;
- killing the holder (`TerminateProcess` on Windows, `SIGKILL` on Unix) releases the
  root without any cleanup by the child;
- the lock file the dead holder leaves behind does not block a successor;
- holders of different roots do not exclude each other;
- after the holder dies, the root reopens with a clean commit log.

## The `vfs-lock-holder` binary

```text
vfs-lock-holder <path> <shared|exclusive> <file|OFFSET+LEN>
```

Takes one `Vfs::try_lock` lock over `OsFs` and holds it, with the same handshake and
exit behavior as `canopy-lock-holder`. `tests/vfs_lock.rs` uses it to check that a
lock held in another process excludes this one, that shared holders in two processes
coexist, that a range lock covers only its range, and that killing the holder
releases its lock.

Run them with:

```sh
cargo test -p canopy-lock-holder
```
