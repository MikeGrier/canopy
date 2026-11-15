// Copyright (c) Michael Grier.

//! The modification count is the coordination state's, shared **across processes** (`D335`).
//!
//! `canopy`'s `src/tests/shared_count.rs` runs a reader and a writer over one in-memory filesystem
//! in one process. These spawn a real second process as the writer over `OsFs` and read it from
//! here, which is the only way to show that a write in another process reads as contention, and
//! that what a writer killed outright leaves behind reads as contention until a writer recovers.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use acadia::{OsFs, Vfs};
use canopy::{
    DocumentBody, DocumentName, GenesisConfig, NodePath, Nonce, Read, RecoveryNeed, Store,
    StoreError, StoreId, StoreOptions,
};

#[path = "shared_count/turns.rs"]
mod turns;

#[path = "shared_count/upgrades.rs"]
mod upgrades;

/// How long to wait for the writer to report a step.
///
/// Generous on purpose: the assertion is that the step happens, not how soon.
const STEP_DEADLINE: Duration = Duration::from_secs(30);

/// How long to wait for the OS to release a killed writer's lock before failing.
const RELEASE_DEADLINE: Duration = Duration::from_secs(10);

/// A unique scratch directory on the real filesystem, removed on drop.
struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new(tag: &str) -> TempRoot {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!("canopy-count-{}-{}-{}", std::process::id(), tag, n));
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

fn nonce_source(prefix: &'static str) -> StoreOptions {
    let mut minted = 0u64;
    StoreOptions::with_nonce_source(
        canopy::Agent::new("canopy-tests", "test", "Canopy test fixture", 1),
        move || {
            minted += 1;
            Nonce::new(&format!("{prefix}-{minted}")).unwrap()
        },
    )
}

/// Initialises a store at `root` and releases it, leaving a valid store nobody holds.
fn seed_store(root: &Path) {
    let config = GenesisConfig::new(StoreId::new("shared-count").unwrap(), 0);
    let store = Store::init(vfs(), root.to_path_buf(), config, nonce_source("seed")).expect("init");
    drop(store);
}

fn open_reader(root: &Path) -> Store {
    Store::open_read_only(
        vfs(),
        root.to_path_buf(),
        StoreOptions::without_nonce_source(canopy::Agent::new(
            "canopy-tests",
            "test",
            "Canopy test fixture",
            1,
        )),
    )
    .expect("open to read")
}

/// Opens `root` as its writer and resolves what writers left, retrying until the OS has released a
/// killed writer's lock.
fn open_writer_within_deadline(root: &Path) -> Result<Store, StoreError> {
    let deadline = Instant::now() + RELEASE_DEADLINE;
    loop {
        match Store::open(vfs(), root.to_path_buf(), nonce_source("here")) {
            Ok((store, _)) => {
                store.resolve()?.agent_stamp.expect("agent version stamp");
                return Ok(store);
            }
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => test_timers::thread::sleep(Duration::from_millis(25)),
        }
    }
}

fn node(raw: &str) -> NodePath {
    NodePath::parse(raw).unwrap()
}

fn document(raw: &str) -> DocumentName {
    DocumentName::new(raw).unwrap()
}

fn body(text: &str) -> DocumentBody {
    DocumentBody::new(text.as_bytes().to_vec())
}

fn settled(count: u64) -> bool {
    Store::undisturbed(count, count)
}

/// A spawned writer process, killed on drop so a failing assertion never leaks it.
struct Writer {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
}

impl Writer {
    fn spawn(root: &Path, node: &str, document: &str, text: &str, pause: bool) -> Writer {
        Self::spawn_mode(
            root,
            node,
            document,
            text,
            if pause { "pause" } else { "write" },
        )
    }

    fn spawn_mode(root: &Path, node: &str, document: &str, text: &str, mode: &str) -> Writer {
        Self::spawn_version(root, node, document, text, mode, "test")
    }

    fn spawn_version(
        root: &Path,
        node: &str,
        document: &str,
        text: &str,
        mode: &str,
        version: &str,
    ) -> Writer {
        Self::spawn_extra(root, node, document, text, mode, version, &[])
    }

    fn spawn_extra(
        root: &Path,
        node: &str,
        document: &str,
        text: &str,
        mode: &str,
        version: &str,
        extra: &[String],
    ) -> Writer {
        let mut command = Command::new(env!("CARGO_BIN_EXE_canopy-store-writer"));
        command
            .arg(root)
            .args([node, document, text])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        command.args([mode, version]);
        command.args(extra);
        let mut child = command.spawn().expect("spawn the writer");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let (sender, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Writer {
            child,
            stdin,
            lines,
        }
    }

    /// Waits for the writer to report `step`.
    fn expect(&self, step: &str) {
        match test_timers::mpsc::recv_timeout(&self.lines, STEP_DEADLINE) {
            Ok(line) if line == step => {}
            other => panic!("the writer did not report {step:?}: {other:?}"),
        }
    }

    fn next(&self) -> String {
        test_timers::mpsc::recv_timeout(&self.lines, STEP_DEADLINE).expect("fixture progress")
    }

    fn command(&mut self, command: &str) {
        let stdin = self.stdin.as_mut().expect("stdin is open");
        writeln!(stdin, "{command}").expect("send command");
        stdin.flush().expect("flush command");
    }

    /// Lets a writer paused part-way through its write go on.
    fn go(&mut self) {
        let stdin = self.stdin.as_mut().expect("stdin is open");
        writeln!(stdin, "go").expect("send go");
        stdin.flush().expect("flush go");
    }

    /// Kills the writer **without** letting it run any destructor, then reaps it: on Windows this
    /// is `TerminateProcess` and on Unix `SIGKILL`, so nothing in the child clears or releases
    /// anything, and whatever it left is what a reader finds.
    fn kill(mut self) {
        self.child.kill().expect("kill the writer");
        self.child.wait().expect("reap the writer");
    }

    /// Closes the writer's stdin, which is how a finished writer is let go, and waits for it.
    fn finish(mut self) {
        assert!(self.close_stdin().success(), "the fixture reported failure");
    }

    fn expect_failure(mut self) {
        assert!(
            !self.close_stdin().success(),
            "the fixture unexpectedly succeeded"
        );
    }

    fn close_stdin(&mut self) -> std::process::ExitStatus {
        drop(self.stdin.take());
        let deadline = Instant::now() + RELEASE_DEADLINE;
        while self.child.try_wait().expect("poll the writer").is_none() {
            assert!(Instant::now() < deadline, "the writer did not exit");
            test_timers::thread::sleep(Duration::from_millis(25));
        }
        self.child.wait().unwrap()
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_reader_here_reads_what_a_writer_in_another_process_wrote() {
    let root = TempRoot::new("reads");
    seed_store(root.path());
    let reader = open_reader(root.path());
    let before = reader.modifications().unwrap();
    assert!(settled(before));

    let writer = Writer::spawn(root.path(), "alpha", "one", "from another process", false);
    writer.expect("opened");
    writer.expect("wrote");

    assert_eq!(
        reader
            .read_document(node("alpha"), &document("one"))
            .unwrap(),
        Read::Found(body("from another process"))
    );
    assert_eq!(reader.modifications().unwrap(), before + 2);
    writer.finish();
}

#[test]
fn a_writer_killed_before_its_first_effect_still_requires_explicit_recovery() {
    let root = TempRoot::new("before-effect");
    seed_store(root.path());
    let reader = open_reader(root.path());
    let count = reader.modifications().unwrap();
    let writer = Writer::spawn_mode(root.path(), "alpha", "one", "not written", "pause-before");
    writer.expect("opened");
    writer.expect("paused");
    assert_eq!(reader.modifications().unwrap(), count);
    assert!(!reader.recovery_need().unwrap().holder_died());
    writer.kill();
    let (survivor, found) =
        Store::open(vfs(), root.path().to_path_buf(), nonce_source("survivor")).unwrap();
    assert!(found.holder_died());
    assert!(matches!(
        survivor.write_document_replicated(node("alpha"), &document("one"), &body("refused")),
        Err(StoreError::RecoveryRequired { .. })
    ));
    survivor.recover().unwrap().agent_stamp.unwrap();
    assert_eq!(
        reader
            .read_document(node("alpha"), &document("one"))
            .unwrap(),
        Read::Absent
    );
    survivor
        .write_document_replicated(node("alpha"), &document("one"), &body("after recovery"))
        .unwrap()
        .agent_stamp
        .unwrap();
}

#[test]
fn a_reader_cannot_write_but_another_attached_writer_takes_a_turn() {
    let root = TempRoot::new("refused");
    seed_store(root.path());
    let writer = Writer::spawn(root.path(), "alpha", "one", "held", false);
    writer.expect("opened");
    writer.expect("wrote");

    let reader = open_reader(root.path());
    assert!(matches!(
        reader.write_document_replicated(node("beta"), &document("two"), &body("no")),
        Err(StoreError::ReadOnly { .. })
    ));
    let (second, _) =
        Store::open(vfs(), root.path().to_path_buf(), nonce_source("second")).unwrap();
    second
        .write_document_replicated(node("beta"), &document("two"), &body("another writer"))
        .unwrap()
        .agent_stamp
        .unwrap();
    assert_eq!(
        reader
            .read_document(node("beta"), &document("two"))
            .unwrap(),
        Read::Found(body("another writer"))
    );
    writer.finish();
}

#[test]
fn a_write_in_another_process_reads_as_contention_here_until_it_ends() {
    let root = TempRoot::new("contended");
    seed_store(root.path());
    let reader = open_reader(root.path());
    let before = reader.modifications().unwrap();

    let mut writer = Writer::spawn(root.path(), "alpha", "one", "midway", true);
    writer.expect("opened");
    writer.expect("paused");

    let during = reader.modifications().unwrap();
    assert_eq!(during, before + 1, "the whole write is one odd span");
    assert!(!settled(during));
    assert!(!Store::undisturbed(during, reader.modifications().unwrap()));
    assert!(matches!(
        reader.snapshot(|_| ()),
        Err(StoreError::SnapshotContended { .. })
    ));

    writer.go();
    writer.expect("wrote");
    let after = reader.modifications().unwrap();
    assert_eq!(after, before + 2);
    assert!(settled(after));
    assert!(reader.snapshot(|_| ()).is_ok());
    assert_eq!(
        reader
            .read_document(node("alpha"), &document("one"))
            .unwrap(),
        Read::Found(body("midway"))
    );
    writer.finish();
}

#[test]
fn what_a_writer_killed_mid_write_left_reads_as_contention_here_until_a_writer_recovers() {
    let root = TempRoot::new("killed");
    seed_store(root.path());
    let reader = open_reader(root.path());
    let before = reader.modifications().unwrap();

    let writer = Writer::spawn(root.path(), "alpha", "one", "never finished", true);
    writer.expect("opened");
    writer.expect("paused");
    writer.kill();

    // Nothing recovered anything: the count the dead writer raised is still the one published,
    // and a reader here never takes the partial state it left for a settled one.
    let left = reader.modifications().unwrap();
    assert_eq!(left, before + 1);
    assert!(!settled(left));
    for _ in 0..3 {
        assert!(matches!(
            reader.snapshot(|_| ()),
            Err(StoreError::SnapshotContended { .. })
        ));
    }

    // A writer opening the root recovers it, and the count settles.
    let recovered = open_writer_within_deadline(root.path()).expect("the root is free once dead");
    let after = reader.modifications().unwrap();
    assert!(settled(after), "recovered: {after}");
    assert!(after > left);
    assert!(reader.snapshot(|_| ()).is_ok());

    recovered
        .write_document_replicated(node("beta"), &document("two"), &body("after the kill"))
        .expect("the recovered root is writable")
        .agent_stamp
        .expect("agent version stamp");
    assert_eq!(
        reader
            .read_document(node("beta"), &document("two"))
            .unwrap(),
        Read::Found(body("after the kill"))
    );
}

#[test]
fn a_writer_process_that_follows_a_killed_one_recovers_the_root_and_writes() {
    let root = TempRoot::new("successor");
    seed_store(root.path());
    let reader = open_reader(root.path());

    let first = Writer::spawn(root.path(), "alpha", "one", "killed", true);
    first.expect("opened");
    first.expect("paused");
    first.kill();
    assert!(!settled(reader.modifications().unwrap()));

    let mut next = None;
    let deadline = Instant::now() + RELEASE_DEADLINE;
    while next.is_none() {
        let writer = Writer::spawn(root.path(), "beta", "two", "from the successor", false);
        // A successor that opened before the OS released the dead lock exits on its own with the
        // failure on stderr and never reports opening, so it is retried until it does.
        match test_timers::mpsc::recv_timeout(&writer.lines, STEP_DEADLINE) {
            Ok(line) if line == "opened" => next = Some(writer),
            _ => {
                assert!(
                    Instant::now() < deadline,
                    "no successor ever opened the root"
                );
                test_timers::thread::sleep(Duration::from_millis(25));
            }
        }
    }
    let next = next.unwrap();
    next.expect("wrote");

    let after = reader.modifications().unwrap();
    assert!(settled(after), "the successor recovered and wrote: {after}");
    assert_eq!(
        reader
            .read_document(node("beta"), &document("two"))
            .unwrap(),
        Read::Found(body("from the successor"))
    );
    next.finish();
}

#[test]
fn successive_writer_processes_each_move_the_count_by_two() {
    let root = TempRoot::new("successive");
    seed_store(root.path());
    let reader = open_reader(root.path());
    let start = reader.modifications().unwrap();
    for i in 0..4u64 {
        let writer = Writer::spawn(root.path(), "alpha", "one", &format!("v{i}"), false);
        writer.expect("opened");
        writer.expect("wrote");
        assert_eq!(reader.modifications().unwrap(), start + 2 * (i + 1));
        writer.finish();
        assert_eq!(reader.modifications().unwrap(), start + 2 * (i + 1));
    }
}

#[test]
fn many_readers_here_sample_one_count_while_a_writer_in_another_process_works() {
    let root = TempRoot::new("many");
    seed_store(root.path());
    let readers: Vec<Store> = (0..6).map(|_| open_reader(root.path())).collect();
    let mut writer = Writer::spawn(root.path(), "alpha", "one", "shared", true);
    writer.expect("opened");
    writer.expect("paused");
    let samples: Vec<u64> = readers.iter().map(|r| r.modifications().unwrap()).collect();
    assert!(samples.iter().all(|s| *s == samples[0] && !settled(*s)));
    writer.go();
    writer.expect("wrote");
    let samples: Vec<u64> = readers.iter().map(|r| r.modifications().unwrap()).collect();
    assert!(samples.iter().all(|s| *s == samples[0] && settled(*s)));
    writer.finish();
}

/// Opens `root` as its writer, resolving nothing, retrying until the OS has released a killed
/// writer's lock.
fn open_within_deadline(root: &Path) -> Result<(Store, RecoveryNeed), StoreError> {
    let deadline = Instant::now() + RELEASE_DEADLINE;
    loop {
        match Store::open(vfs(), root.to_path_buf(), nonce_source("opener")) {
            Ok(opened) => return Ok(opened),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => test_timers::thread::sleep(Duration::from_millis(25)),
        }
    }
}

#[test]
fn a_writer_in_another_process_is_not_reported_as_abandoned_while_it_lives() {
    let root = TempRoot::new("live-discovery");
    seed_store(root.path());
    let reader = open_reader(root.path());
    assert!(!reader.recovery_need().unwrap().required());

    let mut writer = Writer::spawn(root.path(), "alpha", "one", "midway", true);
    writer.expect("opened");
    writer.expect("paused");
    let during = reader.recovery_need().unwrap();
    assert!(
        !during.required(),
        "a live writer's transaction is not abandoned work: {during:?}"
    );

    writer.go();
    writer.expect("wrote");
    assert!(!reader.recovery_need().unwrap().required());
    writer.finish();
    assert!(!reader.recovery_need().unwrap().required());
}

#[test]
fn a_writer_killed_mid_write_is_reported_here_and_recovered_by_a_writer_that_opens() {
    let root = TempRoot::new("dead-discovery");
    seed_store(root.path());
    let reader = open_reader(root.path());

    let writer = Writer::spawn(root.path(), "alpha", "one", "never finished", true);
    writer.expect("opened");
    writer.expect("paused");
    writer.kill();

    // The OS releases a dead process's lock promptly but not synchronously with its reaping, so the
    // record reads as a live holder's until it has.
    let deadline = Instant::now() + RELEASE_DEADLINE;
    let reported = loop {
        let need = reader.recovery_need().unwrap();
        if need.holder_died() || Instant::now() >= deadline {
            break need;
        }
        test_timers::thread::sleep(Duration::from_millis(25));
    };
    assert!(
        reported.holder_died() && reported.required(),
        "a dead writer's work is reported: {reported:?}"
    );

    let (attached, found) = open_within_deadline(root.path()).expect("the root is free once dead");
    assert!(
        found.holder_died() && !found.fenced() && found.unresolved_work() == Some(true),
        "opening was told: {found:?}"
    );
    assert!(attached.recovery_required().unwrap());
    assert!(!attached.recovery_need().unwrap().fenced());
    let mut run = attached.recovery().expect("begin");
    assert!(attached.recovery_need().unwrap().fenced());
    let mut steps = 0;
    while !run.progress().finished() {
        run.step()
            .expect("step")
            .agent_stamp
            .expect("agent version stamp");
        steps += 1;
    }
    assert!(steps >= 1);

    assert!(!reader.recovery_need().unwrap().required());
    assert!(settled(reader.modifications().unwrap()));
    attached
        .write_document_replicated(node("beta"), &document("two"), &body("after the kill"))
        .expect("the recovered root is writable")
        .agent_stamp
        .expect("agent version stamp");
}
