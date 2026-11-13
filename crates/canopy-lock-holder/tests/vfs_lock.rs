// Copyright (c) Michael Grier.

//! `Vfs::try_lock` across processes: what one process holds excludes another, and the
//! operating system releases a killed holder's lock without any cleanup by the holder.
//!
//! The in-process suite in `acadia` shows how guards contend inside one process and what
//! `MemFs` simulates for a death. Only a real second process that is killed outright shows what
//! the platform does, so every test here spawns `vfs-lock-holder` over `OsFs`.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use acadia::{LockMode, LockSpan, OsFs, Vfs, VfsErrorKind, VfsLock, VfsResult};

/// How long to wait for the OS to release a killed holder's lock before failing. Generous,
/// because Windows releases a dead process's byte-range locks when it gets to them.
const RELEASE_DEADLINE: Duration = Duration::from_secs(10);

/// How long to wait for the child's `locked` handshake.
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(30);

/// A unique scratch directory on the real filesystem, removed on drop.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> TempDir {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!("vfs-lock-{}-{}-{}", std::process::id(), tag, n));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch dir");
        TempDir { path }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A spawned holder process, killed on drop so a failing assertion never leaks it.
struct Holder {
    child: Child,
    /// The holder exits when this closes.
    stdin: Option<ChildStdin>,
}

impl Holder {
    /// Spawns the fixture and returns once it reports the lock is held.
    fn spawn(path: &Path, mode: &str, span: &str) -> Holder {
        let mut child = Command::new(env!("CARGO_BIN_EXE_vfs-lock-holder"))
            .arg(path)
            .arg(mode)
            .arg(span)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn the holder");

        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
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

    /// Kills the holder without letting it run any destructor (`TerminateProcess` on Windows,
    /// `SIGKILL` on Unix), then reaps it. Whatever frees the lock is the operating system.
    fn kill(mut self) {
        self.child.kill().expect("kill the holder");
        self.child.wait().expect("reap the holder");
    }

    /// Closes the holder's stdin and waits for it to exit on its own.
    fn close_stdin(mut self) {
        drop(self.stdin.take());
        let deadline = Instant::now() + RELEASE_DEADLINE;
        while self.child.try_wait().expect("poll the holder").is_none() {
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

fn try_lock(path: &Path, mode: LockMode, span: LockSpan) -> VfsResult<Box<dyn VfsLock>> {
    OsFs::new().try_lock(path, mode, span)
}

fn assert_locked(result: VfsResult<Box<dyn VfsLock>>) {
    match result {
        Err(e) => assert_eq!(e.kind(), VfsErrorKind::Locked, "{e}"),
        Ok(held) => panic!("expected the other process's lock to refuse this one, got {held:?}"),
    }
}

/// Takes the lock, retrying until it is granted or [`RELEASE_DEADLINE`] passes. The retry is
/// for the kernel's release of a dead holder's lock, not for flakiness.
fn lock_within_deadline(path: &Path, mode: LockMode, span: LockSpan) -> Box<dyn VfsLock> {
    let deadline = Instant::now() + RELEASE_DEADLINE;
    loop {
        match try_lock(path, mode, span) {
            Ok(held) => return held,
            Err(e) if Instant::now() >= deadline => {
                panic!("the dead holder's lock was never released: {e}")
            }
            Err(e) => {
                assert_eq!(e.kind(), VfsErrorKind::Locked, "{e}");
                test_timers::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

#[test]
fn a_live_holders_whole_file_lock_excludes_this_process() {
    let dir = TempDir::new("whole");
    let path = dir.file("x.lock");
    let holder = Holder::spawn(&path, "exclusive", "file");

    assert_locked(try_lock(&path, LockMode::Exclusive, LockSpan::File));
    assert_locked(try_lock(
        &path,
        LockMode::Shared,
        LockSpan::Range { offset: 0, len: 1 },
    ));

    holder.kill();
}

#[test]
fn a_killed_holders_whole_file_lock_is_released() {
    let dir = TempDir::new("killed-whole");
    let path = dir.file("x.lock");
    let holder = Holder::spawn(&path, "exclusive", "file");
    assert_locked(try_lock(&path, LockMode::Exclusive, LockSpan::File));

    holder.kill();

    drop(lock_within_deadline(
        &path,
        LockMode::Exclusive,
        LockSpan::File,
    ));
}

#[test]
fn a_killed_holders_range_lock_is_released_and_covered_only_its_range() {
    let dir = TempDir::new("killed-range");
    let path = dir.file("ranges");
    let held = LockSpan::Range {
        offset: 100,
        len: 8,
    };
    let holder = Holder::spawn(&path, "exclusive", "100+8");

    assert_locked(try_lock(&path, LockMode::Shared, held));
    let beside = try_lock(
        &path,
        LockMode::Exclusive,
        LockSpan::Range {
            offset: 108,
            len: 8,
        },
    )
    .expect("the range next to the holder's is free");

    holder.kill();

    drop(lock_within_deadline(&path, LockMode::Exclusive, held));
    drop(beside);
}

/// Starts a waiting `lock` on its own thread, which reports on the returned channel once the
/// lock is granted.
fn waiter(
    path: &Path,
    mode: LockMode,
    span: LockSpan,
) -> std::sync::mpsc::Receiver<VfsResult<Box<dyn VfsLock>>> {
    let (granted, receive) = std::sync::mpsc::channel();
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        let _ = granted.send(OsFs::new().lock(&path, mode, span));
    });
    receive
}

/// Asserts the waiter has not been granted within a short while, so it is really waiting.
fn assert_still_waiting(waiting: &std::sync::mpsc::Receiver<VfsResult<Box<dyn VfsLock>>>) {
    assert!(
        test_timers::mpsc::recv_timeout(waiting, Duration::from_millis(200)).is_err(),
        "the waiter was granted a lock another process holds"
    );
}

#[test]
fn a_waiter_is_woken_by_the_holders_death() {
    let dir = TempDir::new("wait-death");
    let path = dir.file("writer.lock");
    let holder = Holder::spawn(&path, "exclusive", "file");
    let waiting = waiter(&path, LockMode::Exclusive, LockSpan::File);
    assert_still_waiting(&waiting);

    holder.kill();

    let taken = test_timers::mpsc::recv_timeout(&waiting, RELEASE_DEADLINE)
        .expect("the dead holder's lock was released to the waiter")
        .expect("the waiting lock is granted");
    assert_locked(try_lock(&path, LockMode::Shared, LockSpan::File));
    drop(taken);
}

#[test]
fn a_range_waiter_is_woken_by_the_holders_death_and_a_waiter_beside_it_never_waits() {
    let dir = TempDir::new("wait-range");
    let path = dir.file("ranges");
    let held = LockSpan::Range { offset: 0, len: 8 };
    let holder = Holder::spawn(&path, "exclusive", "0+8");
    let beside = OsFs::new()
        .lock(
            &path,
            LockMode::Exclusive,
            LockSpan::Range { offset: 8, len: 8 },
        )
        .expect("bytes nobody holds are granted at once");
    let waiting = waiter(&path, LockMode::Shared, held);
    assert_still_waiting(&waiting);

    holder.kill();

    drop(
        test_timers::mpsc::recv_timeout(&waiting, RELEASE_DEADLINE)
            .expect("the dead holder's range was released to the waiter")
            .expect("the waiting lock is granted"),
    );
    drop(beside);
}

#[test]
fn a_waiter_in_another_process_is_woken_when_this_one_lets_go() {
    let dir = TempDir::new("wait-other");
    let path = dir.file("writer.lock");
    let held = try_lock(&path, LockMode::Exclusive, LockSpan::File).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_vfs-lock-holder"))
        .arg(&path)
        .arg("exclusive")
        .arg("file")
        .arg("wait")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the waiting holder");
    let stdout = child.stdout.take().expect("piped stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let read = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(read.map(|_| line));
    });
    assert!(
        test_timers::mpsc::recv_timeout(&rx, Duration::from_millis(200)).is_err(),
        "the other process was granted a lock this one holds"
    );

    drop(held);

    let line = test_timers::mpsc::recv_timeout(&rx, HANDSHAKE_DEADLINE)
        .expect("the waiting process was let in")
        .expect("read its handshake");
    assert_eq!(line.trim(), "locked");
    assert_locked(try_lock(&path, LockMode::Shared, LockSpan::File));
    let holder = Holder { child, stdin: None };
    holder.kill();
}

#[test]
fn shared_holders_in_two_processes_coexist_and_exclude_an_exclusive_one() {
    let dir = TempDir::new("shared");
    let path = dir.file("shared.lock");
    let holder = Holder::spawn(&path, "shared", "file");

    let shared = try_lock(&path, LockMode::Shared, LockSpan::File)
        .expect("a second shared holder in another process");
    assert_locked(try_lock(&path, LockMode::Exclusive, LockSpan::File));

    holder.kill();
    assert_locked(try_lock(&path, LockMode::Exclusive, LockSpan::File));
    drop(shared);
    drop(lock_within_deadline(
        &path,
        LockMode::Exclusive,
        LockSpan::File,
    ));
}

#[test]
fn a_lock_this_process_holds_excludes_a_holder_in_another() {
    let dir = TempDir::new("parent-holds");
    let path = dir.file("x.lock");
    let held = try_lock(
        &path,
        LockMode::Exclusive,
        LockSpan::Range { offset: 0, len: 4 },
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_vfs-lock-holder"))
        .arg(&path)
        .arg("shared")
        .arg("2+1")
        .stdin(Stdio::null())
        .output()
        .expect("run the holder");
    assert!(!output.status.success(), "the holder must be refused");
    assert!(
        output.stdout.is_empty(),
        "a refused holder never reports the lock"
    );
    drop(held);
}

#[test]
fn a_holder_whose_stdin_closes_exits_and_releases_its_lock() {
    let dir = TempDir::new("stdin");
    let path = dir.file("x.lock");
    let holder = Holder::spawn(&path, "exclusive", "file");
    holder.close_stdin();
    drop(lock_within_deadline(
        &path,
        LockMode::Exclusive,
        LockSpan::File,
    ));
}

#[test]
fn a_malformed_request_is_refused_without_a_handshake() {
    let dir = TempDir::new("malformed");
    for args in [
        vec!["only-a-path"],
        vec!["x.lock", "sideways", "file"],
        vec!["x.lock", "shared", "not-a-span"],
        vec!["x.lock", "shared", "5+0"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_vfs-lock-holder"))
            .current_dir(&dir.path)
            .args(&args)
            .stdin(Stdio::null())
            .output()
            .expect("run the holder");
        assert!(!output.status.success(), "{args:?} must be refused");
        assert!(output.stdout.is_empty(), "{args:?} must not report a lock");
    }
}
