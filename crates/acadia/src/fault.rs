// Copyright (c) Michael Grier.

//! Deterministic fault injection over any [`Vfs`] (VFS-4, D55/D56).
//!
//! [`FaultyFs`] wraps another [`Vfs`] (in practice the in-memory mock
//! [`MemFs`](crate::mem::MemFs)) and makes chosen operations on chosen paths
//! fail or return corrupted data, on a **scripted** schedule. There is no
//! runtime randomness: every fault is declared up front in a [`FaultPolicy`] and
//! fires on a deterministic call-count schedule ([`When`]), so a test that
//! injects a fault gets the same behavior on every run (repo quality rules).
//!
//! This is the substrate the D56 never-fault posture is validated against: the
//! store is driven through [`FaultyFs`] and asserted to surface each fault as a
//! typed [`VfsError`], never a panic, and the store's recovery-stress harness
//! scripts sequences of faults (a failed `fsync`, a torn write, a vanished file)
//! to check the store stays up and truthful.
//!
//! # Fault vocabulary
//!
//! A rule pairs a [`FaultOp`] (which operation to intercept) and a
//! [`PathMatcher`] (which paths it applies to) with a [`Fault`]:
//!
//! - [`Fault::Fail`] -- return a chosen [`VfsErrorKind`] instead of performing
//!   the operation. This expresses every "the operation failed" mode: a
//!   **missing** file ([`Fault::missing`], `NotFound`), a **read error** on a
//!   `Read` rule, a **write error** on a `Write` rule, and an **fsync** or
//!   **rename** failure on the respective rule. A `Fail` on a mutating operation
//!   performs nothing, so nothing is persisted (a failed `fsync` therefore does
//!   not advance durability).
//! - [`Fault::ShortWrite`] -- on a `Write`/`Append`, persist only a prefix of
//!   the bytes and then return `WriteZero`, modeling a torn write that leaves the
//!   file partially updated.
//! - [`Fault::Corrupt`] -- on a `Read`, return the real bytes with a
//!   [`Corruption`] applied (a flipped byte, a truncation, or a full
//!   replacement), modeling silent data corruption that a hard error would not.
//! - [`Fault::Observe`] -- leave the operation alone and instead record it into
//!   a [`FaultLog`] and/or run an action while it is in flight (D107). This is
//!   the one effect that is not a failure.
//!
//! Faults apply only where they are meaningful (a `Corrupt` on a write, or a
//! `ShortWrite` on a read, passes through unchanged); the operation is otherwise
//! delegated to the wrapped backend.
//!
//! # Naming the paths a rule applies to
//!
//! [`FaultPolicy::inject`] names one path exactly. When the path is not known in
//! full in advance -- it carries an allocated id or a nonce -- use
//! [`FaultPolicy::inject_matching`] with a [`PathMatcher`]: a file name, a file
//! name prefix (for a file written under a generated suffix), a contained
//! component, or a caller-supplied predicate (D107).
//!
//! # Choosing the moment a rule goes live
//!
//! [`When`] counts from construction. When the moment that matters is one the
//! test controls -- setup must run clean, and only the operation under test may
//! fail -- use [`FaultPolicy::inject_gated`] with a [`FaultGate`] and open it at
//! that moment. The gate composes with [`When`] rather than replacing it, so
//! "open the gate, then fail exactly once" is a gate plus [`When::Once`].

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::{DirEntry, Metadata, Vfs, VfsError, VfsErrorKind, VfsLock, VfsResult};

/// Which [`Vfs`] operation a [`FaultPolicy`] rule intercepts.
///
/// A [`FaultOp::Rename`] rule matches on the operation's **source** path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultOp {
    Read,
    ReadAt,
    Write,
    Append,
    Truncate,
    Fsync,
    Rename,
    RemoveFile,
    CreateDir,
    CreateDirAll,
    RemoveDir,
    RemoveDirAll,
    ReadDir,
    Metadata,
    LockExclusive,
    OpenOutput,
}

/// A caller-supplied path test, for what [`PathMatcher`]'s vocabulary does not
/// cover.
pub type PathPredicate = Arc<dyn Fn(&Path) -> bool + Send + Sync>;

/// Which **paths** a rule applies to (D107).
///
/// A rule names paths through a matcher rather than a path literal, because the
/// interesting paths in a test usually carry an allocated id or a nonce that the
/// test cannot know when it builds the policy.
///
/// All matching is done on a path **normalized** to its named components under
/// the single implicit root (a prefix, a root, and `.` are dropped; `..` pops),
/// so a match never depends on how a caller spelled an equivalent path. The one
/// exception is [`PathMatcher::Predicate`], which is handed the path as the
/// caller of the operation wrote it.
///
/// Prefer the declarative variants over a predicate wherever one of them says
/// what you mean: a predicate is opaque to anything that wants to inspect a
/// policy.
#[derive(Clone)]
pub enum PathMatcher {
    /// Matches one path exactly (the default, and what [`FaultPolicy::inject`]
    /// builds).
    Exact(Vec<OsString>),
    /// Matches any path whose **final component** is this name, whatever
    /// directory it sits in.
    FileName(OsString),
    /// Matches any path whose **final component starts with** this string, which
    /// is how a rule names a file written under a generated suffix -- a staging
    /// temp, a nonce, a rotation counter.
    FileNamePrefix(OsString),
    /// Matches any path that **contains** this component anywhere, which is how
    /// a rule names everything under a directory whose full path it does not
    /// know.
    HasComponent(OsString),
    /// Matches whatever the caller's test accepts.
    Predicate(PathPredicate),
}

impl PathMatcher {
    /// Matches `path` and nothing else.
    #[must_use]
    pub fn exact(path: impl AsRef<Path>) -> Self {
        PathMatcher::Exact(normalize(path.as_ref()))
    }

    /// Matches any path whose final component is `name`.
    #[must_use]
    pub fn file_name(name: impl AsRef<OsStr>) -> Self {
        PathMatcher::FileName(name.as_ref().to_os_string())
    }

    /// Matches any path whose final component starts with `prefix`.
    #[must_use]
    pub fn file_name_prefix(prefix: impl AsRef<OsStr>) -> Self {
        PathMatcher::FileNamePrefix(prefix.as_ref().to_os_string())
    }

    /// Matches any path containing the component `name`.
    #[must_use]
    pub fn has_component(name: impl AsRef<OsStr>) -> Self {
        PathMatcher::HasComponent(name.as_ref().to_os_string())
    }

    /// Matches any path `test` accepts, for what the variants above cannot say.
    #[must_use]
    pub fn predicate(test: impl Fn(&Path) -> bool + Send + Sync + 'static) -> Self {
        PathMatcher::Predicate(Arc::new(test))
    }

    /// Whether this matcher accepts `path`.
    #[must_use]
    pub fn matches(&self, path: &Path) -> bool {
        match self {
            PathMatcher::Exact(parts) => normalize(path) == *parts,
            PathMatcher::FileName(name) => normalize(path).last() == Some(name),
            PathMatcher::FileNamePrefix(prefix) => normalize(path).last().is_some_and(|name| {
                name.as_encoded_bytes()
                    .starts_with(prefix.as_encoded_bytes())
            }),
            PathMatcher::HasComponent(name) => normalize(path).iter().any(|part| part == name),
            PathMatcher::Predicate(test) => test(path),
        }
    }
}

impl fmt::Debug for PathMatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathMatcher::Exact(parts) => f.debug_tuple("Exact").field(parts).finish(),
            PathMatcher::FileName(name) => f.debug_tuple("FileName").field(name).finish(),
            PathMatcher::FileNamePrefix(prefix) => {
                f.debug_tuple("FileNamePrefix").field(prefix).finish()
            }
            PathMatcher::HasComponent(name) => f.debug_tuple("HasComponent").field(name).finish(),
            PathMatcher::Predicate(_) => f.write_str("Predicate(<caller-supplied>)"),
        }
    }
}

/// A runtime switch deciding whether a rule applies yet (D107).
///
/// [`When`] is counted from construction, which cannot express "start failing
/// **now**, once setup is durable" -- a moment the test controls rather than a
/// call count it can predict. A gate is that moment. It is cheap to clone (a
/// handle on shared state), so the policy keeps one clone and the test keeps
/// another.
///
/// The gate **composes with** [`When`] rather than replacing it: while the gate
/// is closed the rule does not apply at all, so a closed gate consumes no part
/// of the schedule and "open the gate, then fail exactly once" fails the first
/// matching call *after* opening.
#[derive(Debug, Clone, Default)]
pub struct FaultGate {
    open: Arc<AtomicBool>,
}

impl FaultGate {
    /// A gate that starts closed: the rule does not apply until
    /// [`open`](FaultGate::open) is called.
    #[must_use]
    pub fn closed() -> Self {
        Self::default()
    }

    /// A gate that starts open.
    #[must_use]
    pub fn opened() -> Self {
        let gate = Self::default();
        gate.open();
        gate
    }

    /// Opens the gate: rules holding it begin to apply.
    pub fn open(&self) {
        self.open.store(true, Ordering::SeqCst);
    }

    /// Closes the gate: rules holding it stop applying.
    pub fn close(&self) {
        self.open.store(false, Ordering::SeqCst);
    }

    /// Whether the gate is currently open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }
}

/// The deterministic schedule on which a rule fires, counted over the calls the
/// rule applies to -- those matching its [`FaultOp`] and path while its
/// [`FaultGate`], if it has one, is open.
#[derive(Debug, Clone, Copy)]
pub enum When {
    /// Fire on every matching call.
    Always,
    /// Fire on the first matching call only.
    Once,
    /// Fire on the first `n` matching calls.
    Times(u32),
    /// Fire only on the `k`-th matching call (1-based).
    OnCall(u32),
}

/// A corruption applied to the bytes returned by a faulted [`Vfs::read`].
#[derive(Debug, Clone)]
pub enum Corruption {
    /// Invert every bit of the byte at `offset` (no-op if out of range).
    FlipByte {
        /// The byte offset to flip.
        offset: usize,
    },
    /// Cut the returned bytes down to `len` (no-op if already shorter).
    Truncate {
        /// The length to truncate to.
        len: usize,
    },
    /// Replace the returned bytes entirely.
    Replace(Vec<u8>),
}

/// One intercepted operation, as recorded by a [`FaultLog`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultRecord {
    /// The operation that was intercepted.
    pub op: FaultOp,
    /// The path it was acting on (the **source** path, for a rename).
    pub path: PathBuf,
}

/// A record of the operations an observing rule saw, in the order they happened.
///
/// Cheap to clone (a handle on shared state): the policy holds one clone and the
/// test keeps another to assert against.
#[derive(Debug, Clone, Default)]
pub struct FaultLog {
    records: Arc<Mutex<Vec<FaultRecord>>>,
}

impl FaultLog {
    /// An empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The operations recorded so far, oldest first.
    #[must_use]
    pub fn records(&self) -> Vec<FaultRecord> {
        self.lock().clone()
    }

    /// How many operations have been recorded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether nothing has been recorded yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> MutexGuard<'_, Vec<FaultRecord>> {
        self.records.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn record(&self, op: FaultOp, path: &Path) {
        self.lock().push(FaultRecord {
            op,
            path: path.to_path_buf(),
        });
    }
}

/// A caller-supplied action run while an observed operation is in flight.
pub type FaultAction = Arc<dyn Fn(FaultOp, &Path) + Send + Sync>;

/// What an observing rule does: record the operation, run an action, or both.
///
/// # The action runs inside the storage operation
///
/// The action is called **before the operation is delegated**, from inside the
/// [`Vfs`] call, so whatever it does is visible to the operation it interrupted.
/// The store is mid-operation at that moment, which means the action **may
/// re-enter the store** -- that is the whole point (it is how a test opens a
/// deterministic race window that is otherwise unreachable) and it is also the
/// hazard: an action that re-enters is running against a store part-way through
/// another operation, and it can deadlock a caller's own locks.
///
/// This decorator's rule state is not held while the action runs, so re-entering
/// the same [`FaultyFs`] is safe.
#[derive(Clone, Default)]
pub struct Observation {
    log: Option<FaultLog>,
    action: Option<FaultAction>,
}

impl Observation {
    /// Records every observed operation into `log`.
    #[must_use]
    pub fn recording(log: &FaultLog) -> Self {
        Observation::default().and_record(log)
    }

    /// Runs `action` while each observed operation is in flight.
    #[must_use]
    pub fn running(action: impl Fn(FaultOp, &Path) + Send + Sync + 'static) -> Self {
        Observation::default().and_run(action)
    }

    /// Also records into `log`.
    #[must_use]
    pub fn and_record(mut self, log: &FaultLog) -> Self {
        self.log = Some(log.clone());
        self
    }

    /// Also runs `action`.
    #[must_use]
    pub fn and_run(mut self, action: impl Fn(FaultOp, &Path) + Send + Sync + 'static) -> Self {
        self.action = Some(Arc::new(action));
        self
    }

    /// Records the call, then runs the action. Called with no rule state held.
    fn observe(&self, op: FaultOp, path: &Path) {
        if let Some(log) = &self.log {
            log.record(op, path);
        }
        if let Some(action) = &self.action {
            action(op, path);
        }
    }
}

impl fmt::Debug for Observation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Observation")
            .field("log", &self.log)
            .field("action", &self.action.as_ref().map(|_| "<caller-supplied>"))
            .finish()
    }
}

/// What a fired rule does to the intercepted operation.
#[derive(Debug, Clone)]
pub enum Fault {
    /// Return this error instead of performing the operation; nothing is
    /// persisted.
    Fail(VfsErrorKind),
    /// On a write/append, persist only the first `persist` bytes and then return
    /// `WriteZero` (a torn write).
    ShortWrite {
        /// How many leading bytes to actually persist.
        persist: usize,
    },
    /// On a read, apply this [`Corruption`] to the returned bytes.
    Corrupt(Corruption),
    /// Leave the operation alone and instead observe it: record it, run an
    /// action while it is in flight, or both.
    ///
    /// This is the one effect that is not a failure. It applies to **every**
    /// [`FaultOp`], since there is nothing about an operation that makes it
    /// unobservable.
    Observe(Observation),
}

impl Fault {
    /// A [`Fault::Fail`] with `NotFound`: the path behaves as if it does not
    /// exist.
    #[must_use]
    pub fn missing() -> Self {
        Fault::Fail(VfsErrorKind::NotFound)
    }

    /// A [`Fault::Fail`] with the generic `Other` I/O category.
    #[must_use]
    pub fn io_error() -> Self {
        Fault::Fail(VfsErrorKind::Other)
    }

    /// A [`Fault::Fail`] with a chosen category.
    #[must_use]
    pub fn error(kind: VfsErrorKind) -> Self {
        Fault::Fail(kind)
    }

    /// A torn write that persists only `persist` leading bytes.
    #[must_use]
    pub fn short_write(persist: usize) -> Self {
        Fault::ShortWrite { persist }
    }

    /// A read that returns corrupted bytes.
    #[must_use]
    pub fn corrupt(corruption: Corruption) -> Self {
        Fault::Corrupt(corruption)
    }

    /// Records the operation into `log` and lets it proceed unchanged.
    #[must_use]
    pub fn record_into(log: &FaultLog) -> Self {
        Fault::Observe(Observation::recording(log))
    }

    /// Runs `action` while the operation is in flight, then lets it proceed
    /// unchanged.
    ///
    /// The action may re-enter the store; see [`Observation`] for what that
    /// means and what it costs.
    #[must_use]
    pub fn observe_with(action: impl Fn(FaultOp, &Path) + Send + Sync + 'static) -> Self {
        Fault::Observe(Observation::running(action))
    }
}

/// One scripted rule: fire `fault` on calls to `op` at any path `matcher`
/// accepts, per `when`.
#[derive(Debug, Clone)]
struct Rule {
    op: FaultOp,
    matcher: PathMatcher,
    gate: Option<FaultGate>,
    when: When,
    fault: Fault,
    matched: u32,
}

impl Rule {
    /// Whether this rule applies to a call on `path`: the operation matches, the
    /// gate (if any) is open, and the matcher accepts the path.
    fn applies(&self, op: FaultOp, path: &Path) -> bool {
        self.op == op
            && self.gate.as_ref().is_none_or(FaultGate::is_open)
            && self.matcher.matches(path)
    }
}

/// A declarative, deterministic set of [`Fault`] rules.
///
/// Built up with [`FaultPolicy::inject`] and handed to [`FaultyFs::new`]. Rules
/// are considered in insertion order and the **first** rule matching an
/// operation's `op` and path decides that call (later rules on the same target
/// are not consulted for that call), so a single rule with a [`When`] schedule
/// expresses a per-path sequence (fail once, fail twice, fail on the third
/// call).
#[derive(Debug, Clone, Default)]
pub struct FaultPolicy {
    rules: Vec<Rule>,
}

impl FaultPolicy {
    /// An empty policy that injects nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a rule: fire `fault` on `op` at exactly `path`, on the schedule
    /// `when`.
    ///
    /// Use [`FaultPolicy::inject_matching`] when the path is not known in full
    /// in advance.
    #[must_use]
    pub fn inject(self, op: FaultOp, path: impl AsRef<Path>, fault: Fault, when: When) -> Self {
        self.inject_matching(op, PathMatcher::exact(path), fault, when)
    }

    /// Adds a rule: fire `fault` on `op` at any path `matcher` accepts, on the
    /// schedule `when`.
    #[must_use]
    pub fn inject_matching(
        self,
        op: FaultOp,
        matcher: PathMatcher,
        fault: Fault,
        when: When,
    ) -> Self {
        self.push(op, matcher, None, fault, when)
    }

    /// Adds a rule that applies only while `gate` is open, and then on the
    /// schedule `when`.
    ///
    /// While the gate is closed the rule does not apply at all: it claims no
    /// call and consumes no part of `when`, so pairing a gate with
    /// [`When::Once`] means "fail the first matching call after the gate opens".
    #[must_use]
    pub fn inject_gated(
        self,
        op: FaultOp,
        matcher: PathMatcher,
        fault: Fault,
        when: When,
        gate: FaultGate,
    ) -> Self {
        self.push(op, matcher, Some(gate), fault, when)
    }

    fn push(
        mut self,
        op: FaultOp,
        matcher: PathMatcher,
        gate: Option<FaultGate>,
        fault: Fault,
        when: When,
    ) -> Self {
        self.rules.push(Rule {
            op,
            matcher,
            gate,
            when,
            fault,
            matched: 0,
        });
        self
    }
}

/// A [`Vfs`] decorator that injects the faults declared in a [`FaultPolicy`].
///
/// Wrap any backend: `FaultyFs::new(MemFs::new(), policy)`. All operations that
/// are not faulted are delegated unchanged to the wrapped backend, reachable via
/// [`FaultyFs::inner`] (e.g. to call
/// [`MemFs::simulate_crash`](crate::mem::MemFs::simulate_crash)).
#[derive(Debug)]
pub struct FaultyFs<V> {
    inner: V,
    rules: Mutex<Vec<Rule>>,
}

impl<V> FaultyFs<V> {
    /// Wraps `inner`, injecting the faults in `policy`.
    pub fn new(inner: V, policy: FaultPolicy) -> Self {
        Self {
            inner,
            rules: Mutex::new(policy.rules),
        }
    }

    /// The wrapped backend.
    pub fn inner(&self) -> &V {
        &self.inner
    }

    /// Locks the rule state, recovering the guard on poison (D56: poisoning is
    /// not a fault to propagate).
    fn lock(&self) -> MutexGuard<'_, Vec<Rule>> {
        self.rules.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Returns the [`Fault`] to apply to this call, if the first rule applying
    /// to `op` and `path` fires on this call.
    ///
    /// An observing rule is discharged here rather than returned: it runs its
    /// observation and reports no fault, so the operation proceeds unchanged.
    /// The rule state is **unlocked** before the observation runs, so an action
    /// that re-enters this same decorator does not deadlock.
    fn check(&self, op: FaultOp, path: &Path) -> Option<Fault> {
        match self.select(op, path) {
            Some(Fault::Observe(observation)) => {
                observation.observe(op, path);
                None
            }
            other => other,
        }
    }

    /// Picks the fault the policy scripts for this call, holding the rule state
    /// only for as long as the decision takes.
    fn select(&self, op: FaultOp, path: &Path) -> Option<Fault> {
        let mut rules = self.lock();
        for rule in rules.iter_mut() {
            if rule.applies(op, path) {
                let fire = should_fire(rule.when, rule.matched);
                rule.matched = rule.matched.saturating_add(1);
                return if fire { Some(rule.fault.clone()) } else { None };
            }
        }
        None
    }
}

/// Whether a rule fires given its schedule and how many matching calls (this one
/// excluded) have already occurred.
fn should_fire(when: When, matched: u32) -> bool {
    match when {
        When::Always => true,
        When::Once => matched == 0,
        When::Times(n) => matched < n,
        When::OnCall(k) => matched.saturating_add(1) == k,
    }
}

/// Normalizes a path to its named components under the single implicit root,
/// matching how paths are compared against rule targets.
fn normalize(path: &Path) -> Vec<OsString> {
    let mut parts = Vec::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(s) => parts.push(s.to_os_string()),
        }
    }
    parts
}

/// Builds the [`VfsError`] returned for an injected failure.
fn fault_err(kind: VfsErrorKind, op: &'static str, path: &Path) -> VfsError {
    VfsError::new(kind, op, "injected fault").with_path(path)
}

/// Applies `corruption` to `bytes` and returns the result.
fn apply_corruption(mut bytes: Vec<u8>, corruption: &Corruption) -> Vec<u8> {
    match corruption {
        Corruption::FlipByte { offset } => {
            if let Some(b) = bytes.get_mut(*offset) {
                *b ^= 0xFF;
            }
        }
        Corruption::Truncate { len } => bytes.truncate(*len),
        Corruption::Replace(new) => bytes = new.clone(),
    }
    bytes
}

impl<V: Vfs> Vfs for FaultyFs<V> {
    fn read(&self, path: &Path) -> VfsResult<Vec<u8>> {
        match self.check(FaultOp::Read, path) {
            Some(Fault::Fail(kind)) => Err(fault_err(kind, "read", path)),
            Some(Fault::Corrupt(corruption)) => {
                Ok(apply_corruption(self.inner.read(path)?, &corruption))
            }
            _ => self.inner.read(path),
        }
    }

    fn read_at(&self, path: &Path, offset: u64, len: usize) -> VfsResult<Vec<u8>> {
        match self.check(FaultOp::ReadAt, path) {
            Some(Fault::Fail(kind)) => Err(fault_err(kind, "read_at", path)),
            Some(Fault::Corrupt(corruption)) => Ok(apply_corruption(
                self.inner.read_at(path, offset, len)?,
                &corruption,
            )),
            _ => self.inner.read_at(path, offset, len),
        }
    }

    fn write(&self, path: &Path, contents: &[u8]) -> VfsResult<()> {
        match self.check(FaultOp::Write, path) {
            Some(Fault::Fail(kind)) => Err(fault_err(kind, "write", path)),
            Some(Fault::ShortWrite { persist }) => {
                let n = persist.min(contents.len());
                self.inner.write(path, &contents[..n])?;
                Err(fault_err(VfsErrorKind::WriteZero, "write", path))
            }
            _ => self.inner.write(path, contents),
        }
    }

    fn append(&self, path: &Path, contents: &[u8]) -> VfsResult<()> {
        match self.check(FaultOp::Append, path) {
            Some(Fault::Fail(kind)) => Err(fault_err(kind, "append", path)),
            Some(Fault::ShortWrite { persist }) => {
                let n = persist.min(contents.len());
                self.inner.append(path, &contents[..n])?;
                Err(fault_err(VfsErrorKind::WriteZero, "append", path))
            }
            _ => self.inner.append(path, contents),
        }
    }

    fn truncate(&self, path: &Path, len: u64) -> VfsResult<()> {
        match self.check(FaultOp::Truncate, path) {
            Some(Fault::Fail(kind)) => Err(fault_err(kind, "truncate", path)),
            _ => self.inner.truncate(path, len),
        }
    }

    fn fsync(&self, path: &Path) -> VfsResult<()> {
        match self.check(FaultOp::Fsync, path) {
            Some(Fault::Fail(kind)) => Err(fault_err(kind, "fsync", path)),
            _ => self.inner.fsync(path),
        }
    }

    fn rename(&self, from: &Path, to: &Path) -> VfsResult<()> {
        match self.check(FaultOp::Rename, from) {
            Some(Fault::Fail(kind)) => Err(fault_err(kind, "rename", from)),
            _ => self.inner.rename(from, to),
        }
    }

    fn remove_file(&self, path: &Path) -> VfsResult<()> {
        match self.check(FaultOp::RemoveFile, path) {
            Some(Fault::Fail(kind)) => Err(fault_err(kind, "remove_file", path)),
            _ => self.inner.remove_file(path),
        }
    }

    fn create_dir(&self, path: &Path) -> VfsResult<()> {
        match self.check(FaultOp::CreateDir, path) {
            Some(Fault::Fail(kind)) => Err(fault_err(kind, "create_dir", path)),
            _ => self.inner.create_dir(path),
        }
    }

    fn create_dir_all(&self, path: &Path) -> VfsResult<()> {
        match self.check(FaultOp::CreateDirAll, path) {
            Some(Fault::Fail(kind)) => Err(fault_err(kind, "create_dir_all", path)),
            _ => self.inner.create_dir_all(path),
        }
    }

    fn remove_dir(&self, path: &Path) -> VfsResult<()> {
        match self.check(FaultOp::RemoveDir, path) {
            Some(Fault::Fail(kind)) => Err(fault_err(kind, "remove_dir", path)),
            _ => self.inner.remove_dir(path),
        }
    }

    fn remove_dir_all(&self, path: &Path) -> VfsResult<()> {
        match self.check(FaultOp::RemoveDirAll, path) {
            Some(Fault::Fail(kind)) => Err(fault_err(kind, "remove_dir_all", path)),
            _ => self.inner.remove_dir_all(path),
        }
    }

    fn read_dir(&self, path: &Path) -> VfsResult<Vec<DirEntry>> {
        match self.check(FaultOp::ReadDir, path) {
            Some(Fault::Fail(kind)) => Err(fault_err(kind, "read_dir", path)),
            _ => self.inner.read_dir(path),
        }
    }

    fn metadata(&self, path: &Path) -> VfsResult<Metadata> {
        match self.check(FaultOp::Metadata, path) {
            Some(Fault::Fail(kind)) => Err(fault_err(kind, "metadata", path)),
            _ => self.inner.metadata(path),
        }
    }

    // Forwarded explicitly rather than inherited: the default reports
    // `Unsupported`, which would silently downgrade a wrapped backend that does
    // support exclusion.
    fn lock_exclusive(&self, path: &Path) -> VfsResult<Box<dyn VfsLock>> {
        match self.check(FaultOp::LockExclusive, path) {
            Some(Fault::Fail(kind)) => Err(fault_err(kind, "lock_exclusive", path)),
            _ => self.inner.lock_exclusive(path),
        }
    }

    // Forwarded explicitly for the same reason as `lock_exclusive`: the default is
    // `Unsupported`. Only the open is intercepted -- the handle it returns is the
    // caller's, and a write made through it never comes back through this decorator.
    fn open_output(&self, path: &Path) -> VfsResult<std::fs::File> {
        match self.check(FaultOp::OpenOutput, path) {
            Some(Fault::Fail(kind)) => Err(fault_err(kind, "open_output", path)),
            _ => self.inner.open_output(path),
        }
    }

    // What a wrapper is does not change what it wraps.
    fn is_os_filesystem(&self) -> bool {
        self.inner.is_os_filesystem()
    }
}
