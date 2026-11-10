// Copyright (c) Michael Grier.

//! `LOCK-M1`/`D116`: a store root has one writer, enforced **across processes**.
//!
//! The unit tests in `canopy`'s `src/tests/lock.rs` run over `MemFs`, whose exclusion scope is a
//! single process. They therefore say nothing at all about the property this decision
//! exists for. These do: every test here spawns a real second process over `OsFs`.
//!
//! The one that matters most is [`a_killed_holder_releases_the_root`]. It is the reason
//! `D116` chose an OS lock over a marker file recording a pid, and if it ever cannot be
//! written, the mechanism should be reconsidered rather than the test skipped.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use acadia::{OsFs, Vfs};
use canopy::{
    DocumentBody, DocumentName, GenesisConfig, NodePath, Nonce, Read, ShireId, Store, StoreError,
    Swap,
};

/// How long to wait for the OS to release a killed holder's lock before failing.
///
/// Generous on purpose: process teardown is not instant, and the assertion is "the kernel
/// releases it", not "the kernel releases it within N milliseconds". A tight bound here
/// would buy nothing and produce a flaky test on a loaded machine.
const RELEASE_DEADLINE: Duration = Duration::from_secs(10);

/// How long to wait for the child's `locked` handshake.
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(30);

/// A unique scratch directory on the real filesystem, removed on drop.
struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new(tag: &str) -> TempRoot {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!("canopy-lock-{}-{}-{}", std::process::id(), tag, n));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch root");
        TempRoot { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn vfs() -> Arc<dyn Vfs> {
    Arc::new(OsFs::new())
}

fn config() -> GenesisConfig {
    GenesisConfig::new(ShireId::new("lock-tests").unwrap(), 0)
        .with_pointer_replica_count(1)
        .unwrap()
}

/// Initialises a store at `root` and releases it, leaving a valid store nobody holds.
fn seed_store(root: &Path) {
    let store = Store::init(vfs(), root.to_path_buf(), config()).expect("init store");
    drop(store);
}

/// A spawned holder process, killed on drop so a failing assertion never leaks it.
struct Holder {
    child: Child,
    /// The holder exits when this closes, so a test process that dies without killing it
    /// does not leave it holding a scratch root.
    stdin: Option<ChildStdin>,
}

impl Holder {
    /// Spawns the fixture over `root` and returns once it reports the lock is held.
    fn spawn(root: &Path) -> Holder {
        let mut child = Command::new(env!("CARGO_BIN_EXE_canopy-lock-holder"))
            .arg(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn the holder");

        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        // Read the handshake on a worker so a child that dies without printing cannot hang
        // the suite: the recv timeout below is the bound.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let read = BufReader::new(stdout).read_line(&mut line);
            let _ = tx.send(read.map(|_| line));
        });

        match test_timers::mpsc::recv_timeout(&rx, HANDSHAKE_DEADLINE) {
            Ok(Ok(line)) if line.trim() == "locked" => Holder { child, stdin },
            other => {
                let _ = child.kill();
                let output = child.wait_with_output().ok();
                let stderr = output
                    .map(|o| String::from_utf8_lossy(&o.stderr).into_owned())
                    .unwrap_or_default();
                panic!("holder never reported the lock ({other:?}); its stderr was: {stderr}");
            }
        }
    }

    /// Kills the holder **without** letting it run any destructor, then reaps it.
    ///
    /// This is what makes the test meaningful: on Windows this is `TerminateProcess` and on
    /// Unix `SIGKILL`, so nothing in the child releases anything. Whatever frees the lock is
    /// the operating system.
    fn kill(mut self) {
        self.child.kill().expect("kill the holder");
        self.child.wait().expect("reap the holder");
        // `Drop` then closes stdin; its kill of the reaped child is a harmless no-op.
    }

    /// Closes the holder's stdin and waits for it to exit on its own, as it does when the
    /// test process that spawned it dies.
    /// Bounded by [`RELEASE_DEADLINE`]; a holder that ignores EOF fails the test instead of
    /// hanging it, and is killed by `Drop`.
    fn close_stdin(mut self) -> std::process::ExitStatus {
        drop(self.stdin.take());
        let deadline = Instant::now() + RELEASE_DEADLINE;
        loop {
            if let Some(status) = self.child.try_wait().expect("poll the holder") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "the holder did not exit after its stdin closed"
            );
            test_timers::thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Opens `root`, retrying until it succeeds or [`RELEASE_DEADLINE`] passes.
///
/// The retry is for the *kernel*, not for flakiness: a killed process's handles are closed
/// as it is torn down, which is prompt but not synchronous with `wait` returning.
fn open_within_deadline(root: &Path) -> Result<Store, StoreError> {
    let deadline = Instant::now() + RELEASE_DEADLINE;
    loop {
        match Store::open(vfs(), root.to_path_buf()) {
            Ok(store) => return Ok(store),
            Err(e) if Instant::now() >= deadline => return Err(e),
            Err(_) => test_timers::thread::sleep(Duration::from_millis(25)),
        }
    }
}

/// A live holder in another process excludes this one. This is the property `D116` is
/// about, and the only test in the tree that actually demonstrates it.
#[test]
fn a_live_holder_in_another_process_excludes_this_one() {
    let root = TempRoot::new("contend");
    seed_store(root.path());
    let holder = Holder::spawn(root.path());

    let refused =
        Store::open(vfs(), root.path().to_path_buf()).expect_err("another process holds this root");
    match refused {
        StoreError::RootInUse { root: reported } => assert_eq!(reported, root.path()),
        other => panic!("expected RootInUse, got {other:?}"),
    }

    holder.kill();
}

/// **The test that justifies the mechanism.** The holder is killed outright, so it runs no
/// destructor, closes nothing, and gets no chance to tidy up -- and the root becomes
/// available anyway, because the kernel owns the release.
///
/// A marker file recording a pid would fail here: it would still be sitting in the root,
/// and the only way to recover would be to check whether that pid is alive, which races
/// with pid reuse.
#[test]
fn a_killed_holder_releases_the_root() {
    let root = TempRoot::new("killed");
    seed_store(root.path());
    let holder = Holder::spawn(root.path());

    // Confirm the lock really was held, so a passing test cannot mean "the child never
    // acquired anything".
    assert!(
        matches!(
            Store::open(vfs(), root.path().to_path_buf()),
            Err(StoreError::RootInUse { .. })
        ),
        "the holder must actually hold the lock before we kill it"
    );

    holder.kill();

    let recovered = open_within_deadline(root.path())
        .expect("the OS released the dead holder's lock without any cleanup by the child");
    drop(recovered);
}

/// The lock file the dead holder left behind is inert. Nothing reaps it, and nothing needs
/// to -- if its mere presence blocked a successor, every crash would wedge the store.
#[test]
fn the_dead_holders_lock_file_does_not_wedge_the_root() {
    let root = TempRoot::new("debris");
    seed_store(root.path());
    let holder = Holder::spawn(root.path());
    holder.kill();

    let lock_file = root.path().join(".lock");
    assert!(
        lock_file.is_file(),
        "the dead holder's lock file is still on disk at {lock_file:?}"
    );

    let reopened = open_within_deadline(root.path()).expect("the leftover file wedges nothing");
    drop(reopened);
}

/// Exclusion is per-root, so two processes over two roots do not interfere. Without this,
/// "one writer" would mean "one writer per machine", which is not what was decided.
#[test]
fn holders_of_different_roots_do_not_exclude_each_other() {
    let first = TempRoot::new("root-a");
    let second = TempRoot::new("root-b");
    seed_store(first.path());
    seed_store(second.path());

    let holder = Holder::spawn(first.path());

    let other = Store::open(vfs(), second.path().to_path_buf())
        .expect("a different root is a different lock");
    drop(other);

    holder.kill();
}

/// Once the holder is gone the root is fully usable, not merely openable: recovery runs and
/// writes land. A lock that were somehow still half-held would surface here rather than in
/// production.
#[test]
fn the_root_is_writable_again_after_the_holder_dies() {
    let root = TempRoot::new("writable");
    seed_store(root.path());
    let holder = Holder::spawn(root.path());
    holder.kill();

    let store = open_within_deadline(root.path()).expect("reopen after the holder died");
    assert!(
        store.commit_log_is_empty().expect("read the log"),
        "a store reopened after a kill starts from a clean log"
    );

    let node = NodePath::parse("after/kill").unwrap();
    let name = DocumentName::new("doc").unwrap();
    let body = DocumentBody::new(b"written after the holder died".to_vec());
    let outcome = store
        .update_document(
            Swap::new(&node, name.clone(), body.clone()),
            &Nonce::new("after-kill").unwrap(),
        )
        .expect("write after the holder died");
    assert!(outcome.is_committed());
    drop(store);

    let reopened = open_within_deadline(root.path()).expect("reopen after writing");
    assert_eq!(
        reopened.read_document(&node, &name).expect("read back"),
        Read::Found(body),
        "the write survives a reopen"
    );
}

/// A holder whose parent goes away exits by itself and releases the root. Closing stdin is
/// what the OS does to the pipe when the parent dies, so without this a crashed test run
/// would leave holders behind, each keeping a scratch root locked.
#[test]
fn a_holder_whose_stdin_closes_exits_and_releases_the_root() {
    let root = TempRoot::new("orphan");
    seed_store(root.path());
    let holder = Holder::spawn(root.path());

    let status = holder.close_stdin();
    assert!(
        status.success(),
        "the holder exits cleanly on EOF: {status:?}"
    );

    let reopened = open_within_deadline(root.path()).expect("the exited holder released it");
    drop(reopened);
}
