// Copyright (c) Michael Grier.
// Split from tests.rs at 0b1f7d4.

//! Fault injection over a [`FaultyFs`]: the fault vocabulary on a scripted
//! schedule, and the D107 surface (matchers, gates, observation).

use super::*;

// ---------------------------------------------------------------------------
// Fault injection -- every mode surfaces as the expected typed error, over the
// mock, on a deterministic schedule.
// ---------------------------------------------------------------------------

/// Builds a faulty filesystem wrapping a mock that already has `/data`.
fn faulty(policy: FaultPolicy) -> FaultyFs<MemFs> {
    let fs = FaultyFs::new(MemFs::new(), policy);
    fs.create_dir_all(Path::new("/data")).unwrap();
    fs
}

#[test]
fn fault_missing_makes_a_present_file_read_not_found() {
    let fs =
        faulty(FaultPolicy::new().inject(FaultOp::Read, "/data/f", Fault::missing(), When::Always));
    fs.write(Path::new("/data/f"), b"present").unwrap();
    assert_kind(fs.read(Path::new("/data/f")), VfsErrorKind::NotFound);
}

#[test]
fn fault_read_error_surfaces_typed_error() {
    let fs = faulty(FaultPolicy::new().inject(
        FaultOp::Read,
        "/data/f",
        Fault::io_error(),
        When::Always,
    ));
    fs.write(Path::new("/data/f"), b"present").unwrap();
    assert_kind(fs.read(Path::new("/data/f")), VfsErrorKind::Other);
}

#[test]
fn fault_corrupt_flip_truncate_replace() {
    // Flip.
    let fs = faulty(FaultPolicy::new().inject(
        FaultOp::Read,
        "/data/f",
        Fault::corrupt(Corruption::FlipByte { offset: 0 }),
        When::Always,
    ));
    fs.write(Path::new("/data/f"), b"AAAA").unwrap();
    assert_eq!(
        fs.read(Path::new("/data/f")).unwrap(),
        vec![0xBE, 0x41, 0x41, 0x41]
    );

    // Truncate.
    let fs = faulty(FaultPolicy::new().inject(
        FaultOp::Read,
        "/data/f",
        Fault::corrupt(Corruption::Truncate { len: 2 }),
        When::Always,
    ));
    fs.write(Path::new("/data/f"), b"AAAA").unwrap();
    assert_eq!(fs.read(Path::new("/data/f")).unwrap(), b"AA");

    // Replace.
    let fs = faulty(FaultPolicy::new().inject(
        FaultOp::Read,
        "/data/f",
        Fault::corrupt(Corruption::Replace(b"zzz".to_vec())),
        When::Always,
    ));
    fs.write(Path::new("/data/f"), b"AAAA").unwrap();
    assert_eq!(fs.read(Path::new("/data/f")).unwrap(), b"zzz");
}

#[test]
fn fault_short_write_persists_prefix_then_write_zero() {
    let fs = faulty(FaultPolicy::new().inject(
        FaultOp::Write,
        "/data/f",
        Fault::short_write(2),
        When::Once,
    ));
    assert_kind(
        fs.write(Path::new("/data/f"), b"hello"),
        VfsErrorKind::WriteZero,
    );
    // The prefix was persisted; a subsequent (unfaulted) read observes it.
    assert_eq!(fs.read(Path::new("/data/f")).unwrap(), b"he");
}

#[test]
fn fault_write_error_persists_nothing() {
    let fs =
        faulty(FaultPolicy::new().inject(FaultOp::Write, "/data/f", Fault::io_error(), When::Once));
    assert_kind(
        fs.write(Path::new("/data/f"), b"hello"),
        VfsErrorKind::Other,
    );
    // Nothing was created.
    assert_kind(fs.read(Path::new("/data/f")), VfsErrorKind::NotFound);
}

#[test]
fn fault_fsync_failure_does_not_advance_durability() {
    let fs = faulty(FaultPolicy::new().inject(
        FaultOp::Fsync,
        "/data/f",
        Fault::io_error(),
        When::Always,
    ));
    fs.write(Path::new("/data/f"), b"payload").unwrap();
    assert_kind(fs.fsync(Path::new("/data/f")), VfsErrorKind::Other);
    // Durability never advanced, so a crash loses the write.
    fs.inner().simulate_crash();
    assert!(!fs.exists(Path::new("/data/f")).unwrap());
}

#[test]
fn fault_rename_failure_leaves_source_in_place() {
    let fs = faulty(FaultPolicy::new().inject(
        FaultOp::Rename,
        "/data/a",
        Fault::io_error(),
        When::Once,
    ));
    fs.write(Path::new("/data/a"), b"payload").unwrap();
    assert_kind(
        fs.rename(Path::new("/data/a"), Path::new("/data/b")),
        VfsErrorKind::Other,
    );
    assert!(fs.exists(Path::new("/data/a")).unwrap());
    assert!(!fs.exists(Path::new("/data/b")).unwrap());
}

#[test]
fn fault_once_fires_exactly_once() {
    let fs =
        faulty(FaultPolicy::new().inject(FaultOp::Read, "/data/f", Fault::io_error(), When::Once));
    fs.write(Path::new("/data/f"), b"payload").unwrap();
    assert_kind(fs.read(Path::new("/data/f")), VfsErrorKind::Other);
    assert_eq!(fs.read(Path::new("/data/f")).unwrap(), b"payload");
}

#[test]
fn fault_on_call_fires_only_on_the_kth_call() {
    let fs = faulty(FaultPolicy::new().inject(
        FaultOp::Read,
        "/data/f",
        Fault::io_error(),
        When::OnCall(2),
    ));
    fs.write(Path::new("/data/f"), b"payload").unwrap();
    assert_eq!(fs.read(Path::new("/data/f")).unwrap(), b"payload");
    assert_kind(fs.read(Path::new("/data/f")), VfsErrorKind::Other);
    assert_eq!(fs.read(Path::new("/data/f")).unwrap(), b"payload");
}

#[test]
fn fault_targets_only_the_named_path() {
    let fs =
        faulty(FaultPolicy::new().inject(FaultOp::Read, "/data/a", Fault::missing(), When::Always));
    fs.write(Path::new("/data/a"), b"1").unwrap();
    fs.write(Path::new("/data/b"), b"2").unwrap();
    assert_kind(fs.read(Path::new("/data/a")), VfsErrorKind::NotFound);
    assert_eq!(fs.read(Path::new("/data/b")).unwrap(), b"2");
}

// ---------------------------------------------------------------------------
// The D107 surface -- matchers, gates, and observation. These run against a
// FaultyFs over *both* backends, so the guarantee is backend-independent by
// construction rather than by assertion.
// ---------------------------------------------------------------------------

/// Runs a fault scenario against a [`FaultyFs`] over each backend in turn.
///
/// `setup` is called once per backend with that backend's root, because a rule
/// usually names paths under it and because any gate or log the scenario needs
/// must be **fresh** for each pass -- shared state carried from one backend to
/// the next would make the second pass depend on the first. Whatever `setup`
/// returns beside the policy is handed back to `body` as its state.
fn on_both_faulty<S>(
    tag: &str,
    setup: impl Fn(&Path) -> (FaultPolicy, S),
    body: impl Fn(&dyn Vfs, &Path, &S),
) {
    let mem = MemFs::new();
    let mem_root = PathBuf::from("/data");
    mem.create_dir_all(&mem_root).expect("create mem root");
    let (policy, state) = setup(&mem_root);
    body(&FaultyFs::new(mem, policy), &mem_root, &state);

    let tmp = TempDir::new(tag);
    let os_root = tmp.path().to_path_buf();
    let (policy, state) = setup(&os_root);
    body(&FaultyFs::new(OsFs::new(), policy), &os_root, &state);
}

/// Writes `payload` to `path`, creating parent directories as needed.
fn seed(fs: &dyn Vfs, path: &Path, payload: &[u8]) {
    if let Some(parent) = path.parent() {
        fs.create_dir_all(parent).expect("create parent");
    }
    fs.write(path, payload).expect("seed file");
}

#[test]
fn matcher_exact_matches_only_that_path() {
    on_both_faulty(
        "matcher-exact",
        |root| {
            let policy = FaultPolicy::new().inject_matching(
                FaultOp::Read,
                PathMatcher::exact(root.join("target")),
                Fault::io_error(),
                When::Always,
            );
            (policy, ())
        },
        |fs, root, ()| {
            let target = root.join("target");
            let sibling = root.join("other");
            let same_name_elsewhere = root.join("sub").join("target");
            seed(fs, &target, b"payload");
            seed(fs, &sibling, b"payload");
            seed(fs, &same_name_elsewhere, b"payload");

            assert_kind(fs.read(&target), VfsErrorKind::Other);
            assert_eq!(fs.read(&sibling).unwrap(), b"payload");
            // Exact means exact: the same file name in another directory is a
            // different path.
            assert_eq!(fs.read(&same_name_elsewhere).unwrap(), b"payload");
        },
    );
}

#[test]
fn matcher_file_name_matches_that_name_in_any_directory() {
    on_both_faulty(
        "matcher-file-name",
        |_root| {
            let policy = FaultPolicy::new().inject_matching(
                FaultOp::Read,
                PathMatcher::file_name("target"),
                Fault::io_error(),
                When::Always,
            );
            (policy, ())
        },
        |fs, root, ()| {
            let at_root = root.join("target");
            let nested = root.join("a").join("b").join("target");
            let other_name = root.join("a").join("untargeted");
            // A directory component of the right name is not a file name match.
            let as_a_directory = root.join("target-dir").join("f");
            seed(fs, &at_root, b"payload");
            seed(fs, &nested, b"payload");
            seed(fs, &other_name, b"payload");
            seed(fs, &as_a_directory, b"payload");

            assert_kind(fs.read(&at_root), VfsErrorKind::Other);
            assert_kind(fs.read(&nested), VfsErrorKind::Other);
            assert_eq!(fs.read(&other_name).unwrap(), b"payload");
            assert_eq!(fs.read(&as_a_directory).unwrap(), b"payload");
        },
    );
}

#[test]
fn matcher_file_name_prefix_matches_a_generated_suffix() {
    on_both_faulty(
        "matcher-file-name-prefix",
        |_root| {
            let policy = FaultPolicy::new().inject_matching(
                FaultOp::Read,
                PathMatcher::file_name_prefix("ENGINE"),
                Fault::io_error(),
                When::Always,
            );
            (policy, ())
        },
        |fs, root, ()| {
            // The shape a staged write actually produces: the document name plus a
            // nonce the policy cannot know when it is built.
            let staged = root.join("ENGINE.tmp.N3");
            let sidecar = root.join("ENGINE.intent.N3");
            let bare = root.join("ENGINE");
            let other_document = root.join("RECORD.tmp.N3");
            // A prefix binds to the *start* of the name, not anywhere within it.
            let suffixed_elsewhere = root.join("NOT-ENGINE.tmp.N3");
            seed(fs, &staged, b"payload");
            seed(fs, &sidecar, b"payload");
            seed(fs, &bare, b"payload");
            seed(fs, &other_document, b"payload");
            seed(fs, &suffixed_elsewhere, b"payload");

            assert_kind(fs.read(&staged), VfsErrorKind::Other);
            assert_kind(fs.read(&sidecar), VfsErrorKind::Other);
            // A prefix matches the bare name too: every name starts with itself.
            assert_kind(fs.read(&bare), VfsErrorKind::Other);
            assert_eq!(fs.read(&other_document).unwrap(), b"payload");
            assert_eq!(fs.read(&suffixed_elsewhere).unwrap(), b"payload");
        },
    );
}

#[test]
fn matcher_has_component_matches_everything_beneath_it() {
    on_both_faulty(
        "matcher-has-component",
        |_root| {
            let policy = FaultPolicy::new().inject_matching(
                FaultOp::Read,
                PathMatcher::has_component("staging"),
                Fault::io_error(),
                When::Always,
            );
            (policy, ())
        },
        |fs, root, ()| {
            let direct_child = root.join("staging").join("f");
            let deeper = root.join("staging").join("x").join("y");
            // The component itself, named as the final element, still contains it.
            let live = root.join("live").join("f");
            seed(fs, &direct_child, b"payload");
            seed(fs, &deeper, b"payload");
            seed(fs, &live, b"payload");

            assert_kind(fs.read(&direct_child), VfsErrorKind::Other);
            assert_kind(fs.read(&deeper), VfsErrorKind::Other);
            assert_eq!(fs.read(&live).unwrap(), b"payload");
        },
    );
}

#[test]
fn matcher_predicate_matches_what_the_caller_accepts() {
    on_both_faulty(
        "matcher-predicate",
        |_root| {
            // The shape none of the declarative variants can say: a file name
            // suffix, which is how a nonce-carrying temporary file is named.
            let policy = FaultPolicy::new().inject_matching(
                FaultOp::Read,
                PathMatcher::predicate(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.ends_with(".tmp"))
                }),
                Fault::io_error(),
                When::Always,
            );
            (policy, ())
        },
        |fs, root, ()| {
            let temporary = root.join("ticket-4f2a91.tmp");
            let published = root.join("ticket-4f2a91.json");
            seed(fs, &temporary, b"payload");
            seed(fs, &published, b"payload");

            assert_kind(fs.read(&temporary), VfsErrorKind::Other);
            assert_eq!(fs.read(&published).unwrap(), b"payload");
        },
    );
}

#[test]
fn gate_opens_and_closes_mid_run() {
    on_both_faulty(
        "gate-mid-run",
        |_root| {
            let gate = FaultGate::closed();
            let policy = FaultPolicy::new().inject_gated(
                FaultOp::Read,
                PathMatcher::file_name("f"),
                Fault::io_error(),
                When::Always,
                gate.clone(),
            );
            (policy, gate)
        },
        |fs, root, gate| {
            let path = root.join("f");
            seed(fs, &path, b"payload");

            // Setup runs clean while the gate is shut.
            assert!(!gate.is_open());
            assert_eq!(fs.read(&path).unwrap(), b"payload");

            gate.open();
            assert_kind(fs.read(&path), VfsErrorKind::Other);

            gate.close();
            assert_eq!(fs.read(&path).unwrap(), b"payload");
        },
    );
}

#[test]
fn gate_composes_with_when_once() {
    on_both_faulty(
        "gate-once",
        |_root| {
            let gate = FaultGate::closed();
            let policy = FaultPolicy::new().inject_gated(
                FaultOp::Read,
                PathMatcher::file_name("f"),
                Fault::io_error(),
                When::Once,
                gate.clone(),
            );
            (policy, gate)
        },
        |fs, root, gate| {
            let path = root.join("f");
            seed(fs, &path, b"payload");

            // Calls made while the gate is shut consume no part of the schedule;
            // were it otherwise, the single firing would be spent here and the
            // rule could never fire after opening.
            for _ in 0..3 {
                assert_eq!(fs.read(&path).unwrap(), b"payload");
            }

            gate.open();
            assert_kind(fs.read(&path), VfsErrorKind::Other);
            assert_eq!(fs.read(&path).unwrap(), b"payload");
        },
    );
}

#[test]
fn gate_composes_with_when_times() {
    on_both_faulty(
        "gate-times",
        |_root| {
            let gate = FaultGate::closed();
            let policy = FaultPolicy::new().inject_gated(
                FaultOp::Read,
                PathMatcher::file_name("f"),
                Fault::io_error(),
                When::Times(2),
                gate.clone(),
            );
            (policy, gate)
        },
        |fs, root, gate| {
            let path = root.join("f");
            seed(fs, &path, b"payload");
            assert_eq!(fs.read(&path).unwrap(), b"payload");

            gate.open();
            assert_kind(fs.read(&path), VfsErrorKind::Other);
            assert_kind(fs.read(&path), VfsErrorKind::Other);
            assert_eq!(fs.read(&path).unwrap(), b"payload");
        },
    );
}

#[test]
fn gate_composes_with_when_on_call() {
    on_both_faulty(
        "gate-on-call",
        |_root| {
            let gate = FaultGate::closed();
            let policy = FaultPolicy::new().inject_gated(
                FaultOp::Read,
                PathMatcher::file_name("f"),
                Fault::io_error(),
                When::OnCall(2),
                gate.clone(),
            );
            (policy, gate)
        },
        |fs, root, gate| {
            let path = root.join("f");
            seed(fs, &path, b"payload");
            assert_eq!(fs.read(&path).unwrap(), b"payload");

            gate.open();
            // The k-th call is counted from the first call after the gate opened.
            assert_eq!(fs.read(&path).unwrap(), b"payload");
            assert_kind(fs.read(&path), VfsErrorKind::Other);
            assert_eq!(fs.read(&path).unwrap(), b"payload");
        },
    );
}

#[test]
fn gate_composes_with_when_always() {
    on_both_faulty(
        "gate-always",
        |_root| {
            let gate = FaultGate::opened();
            let policy = FaultPolicy::new().inject_gated(
                FaultOp::Read,
                PathMatcher::file_name("f"),
                Fault::io_error(),
                When::Always,
                gate.clone(),
            );
            (policy, gate)
        },
        |fs, root, gate| {
            let path = root.join("f");
            seed(fs, &path, b"payload");

            assert_kind(fs.read(&path), VfsErrorKind::Other);
            assert_kind(fs.read(&path), VfsErrorKind::Other);

            gate.close();
            assert_eq!(fs.read(&path).unwrap(), b"payload");
        },
    );
}

#[test]
fn a_closed_gate_claims_no_call_so_a_later_rule_decides_it() {
    on_both_faulty(
        "gate-claims-nothing",
        |_root| {
            let gate = FaultGate::closed();
            let policy = FaultPolicy::new()
                .inject_gated(
                    FaultOp::Read,
                    PathMatcher::file_name("f"),
                    Fault::missing(),
                    When::Always,
                    gate.clone(),
                )
                .inject_matching(
                    FaultOp::Read,
                    PathMatcher::file_name("f"),
                    Fault::io_error(),
                    When::Always,
                );
            (policy, gate)
        },
        |fs, root, gate| {
            let path = root.join("f");
            seed(fs, &path, b"payload");

            // The gated rule is first in insertion order but not live, so it does
            // not swallow the call: the second rule decides it.
            assert_kind(fs.read(&path), VfsErrorKind::Other);

            gate.open();
            assert_kind(fs.read(&path), VfsErrorKind::NotFound);
        },
    );
}

#[test]
fn observation_records_calls_without_disturbing_them() {
    on_both_faulty(
        "observe-record",
        |_root| {
            let log = FaultLog::new();
            let policy = FaultPolicy::new().inject_matching(
                FaultOp::Read,
                PathMatcher::file_name("watched"),
                Fault::record_into(&log),
                When::Always,
            );
            (policy, log)
        },
        |fs, root, log| {
            let watched = root.join("watched");
            let ignored = root.join("ignored");
            seed(fs, &watched, b"payload");
            seed(fs, &ignored, b"payload");
            assert!(log.is_empty());

            // The operation is completely undisturbed.
            assert_eq!(fs.read(&watched).unwrap(), b"payload");
            assert_eq!(fs.read(&ignored).unwrap(), b"payload");
            assert_eq!(fs.read(&watched).unwrap(), b"payload");

            let records = log.records();
            assert_eq!(records.len(), 2);
            assert_eq!(log.len(), 2);
            assert!(records.iter().all(|r| r.op == FaultOp::Read));
            assert!(records.iter().all(|r| r.path == watched));
        },
    );
}

#[test]
fn observation_runs_an_action_and_can_record_at_the_same_time() {
    on_both_faulty(
        "observe-record-and-run",
        |_root| {
            let log = FaultLog::new();
            let seen = Arc::new(AtomicU64::new(0));
            let counter = Arc::clone(&seen);
            let policy = FaultPolicy::new().inject_matching(
                FaultOp::Write,
                PathMatcher::file_name("watched"),
                Fault::Observe(Observation::recording(&log).and_run(move |op, _path| {
                    assert_eq!(op, FaultOp::Write);
                    counter.fetch_add(1, Ordering::SeqCst);
                })),
                When::Always,
            );
            (policy, (log, seen))
        },
        |fs, root, (log, seen)| {
            let watched = root.join("watched");
            fs.write(&watched, b"first").expect("write is undisturbed");
            fs.write(&watched, b"second").expect("write is undisturbed");

            assert_eq!(fs.read(&watched).unwrap(), b"second");
            assert_eq!(seen.load(Ordering::SeqCst), 2);
            assert_eq!(log.len(), 2);
        },
    );
}

#[test]
fn observation_obeys_the_schedule_like_any_other_effect() {
    on_both_faulty(
        "observe-schedule",
        |_root| {
            let log = FaultLog::new();
            let policy = FaultPolicy::new().inject_matching(
                FaultOp::Read,
                PathMatcher::file_name("watched"),
                Fault::record_into(&log),
                When::Once,
            );
            (policy, log)
        },
        |fs, root, log| {
            let watched = root.join("watched");
            seed(fs, &watched, b"payload");
            for _ in 0..3 {
                assert_eq!(fs.read(&watched).unwrap(), b"payload");
            }
            assert_eq!(log.len(), 1);
        },
    );
}

#[test]
fn an_observing_action_may_re_enter_the_same_decorator() {
    // The rule state must not be held while the action runs, or the first
    // re-entrant call would deadlock -- which is the case the facility exists to
    // support (a test mutating durable state from inside another operation).
    let handle: Arc<OnceLock<Weak<FaultyFs<MemFs>>>> = Arc::new(OnceLock::new());
    let from_action = Arc::clone(&handle);

    let policy = FaultPolicy::new().inject_matching(
        FaultOp::Read,
        PathMatcher::file_name("trigger"),
        Fault::observe_with(move |_op, _path| {
            let fs = from_action
                .get()
                .and_then(Weak::upgrade)
                .expect("decorator handle");
            fs.write(
                Path::new("/data/side-effect"),
                b"written from inside a read",
            )
            .expect("re-entrant write");
        }),
        When::Once,
    );

    let fs = Arc::new(faulty(policy));
    handle.set(Arc::downgrade(&fs)).expect("install handle");

    let trigger = Path::new("/data/trigger");
    fs.write(trigger, b"payload").unwrap();

    // The read completes normally, and the action's write landed while it was in
    // flight.
    assert_eq!(fs.read(trigger).unwrap(), b"payload");
    assert_eq!(
        fs.read(Path::new("/data/side-effect")).unwrap(),
        b"written from inside a read"
    );
}

/// The unacquirable-lock branch is injectable (`D116`). This is not incidental
/// coverage: reaching this branch through the seam is the stated reason locking
/// was put *in* the `Vfs` rather than beside it, so if this stops being possible
/// the argument for the design has been lost.
#[test]
fn an_unacquirable_lock_is_injectable() {
    let fs = faulty(FaultPolicy::new().inject(
        FaultOp::LockExclusive,
        "/data/.lock",
        Fault::Fail(VfsErrorKind::Locked),
        When::Always,
    ));

    let refused = fs
        .lock_exclusive(Path::new("/data/.lock"))
        .expect_err("the injected fault refuses the lock");
    assert_eq!(refused.kind(), VfsErrorKind::Locked);
    assert_eq!(refused.op(), "lock_exclusive");
}

/// The decorator forwards to the backend instead of inheriting the trait's
/// `Unsupported` default -- a wrapper that silently downgraded a capable backend
/// would make every wrapped store unlockable.
#[test]
fn the_decorator_forwards_locking_to_its_backend() {
    let fs = faulty(FaultPolicy::new());

    let held = fs
        .lock_exclusive(Path::new("/data/.lock"))
        .expect("the wrapped MemFs grants the lock");
    let refused = fs
        .lock_exclusive(Path::new("/data/.lock"))
        .expect_err("and the backend's exclusion is visible through the wrapper");
    assert_eq!(refused.kind(), VfsErrorKind::Locked);
    drop(held);
}

/// A rule for one path does not refuse another, so a test can fail exactly the
/// root it means to.
#[test]
fn an_injected_lock_fault_is_confined_to_its_path() {
    let fs = faulty(FaultPolicy::new().inject(
        FaultOp::LockExclusive,
        "/data/.lock",
        Fault::Fail(VfsErrorKind::Locked),
        When::Always,
    ));

    assert!(fs.lock_exclusive(Path::new("/data/.lock")).is_err());
    drop(
        fs.lock_exclusive(Path::new("/data/other.lock"))
            .expect("an unrelated path is unaffected"),
    );
}

/// The open of a child's output handle is injectable (`EO-17`). The stated reason
/// the open went *into* the `Vfs` is that it is the one place a handle is created,
/// so it must be a place a test can make fail.
#[test]
fn an_unopenable_output_is_injectable_and_confined_to_its_path() {
    let fs = faulty(FaultPolicy::new().inject(
        FaultOp::OpenOutput,
        "/data/exec/stdout",
        Fault::Fail(VfsErrorKind::StorageFull),
        When::Always,
    ));

    let refused = fs
        .open_output(Path::new("/data/exec/stdout"))
        .expect_err("the injected fault refuses the open");
    assert_eq!(refused.kind(), VfsErrorKind::StorageFull);
    assert_eq!(refused.op(), "open_output");
    // An unrelated path reaches the backend, which here has no OS files: the
    // refusal is the backend's, not an injected one.
    let other = fs
        .open_output(Path::new("/data/exec/stderr"))
        .expect_err("memory has no OS handles");
    assert_eq!(other.kind(), VfsErrorKind::Unsupported);
}

/// The decorator forwards to its backend instead of inheriting `Unsupported`, so
/// wrapping the real filesystem for fault injection still opens real files.
#[test]
fn the_decorator_forwards_the_output_open_to_its_backend() {
    let root = std::env::temp_dir().join(format!("acadia-open-output-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let fs = FaultyFs::new(crate::OsFs::new(), FaultPolicy::new());

    let mut file = fs.open_output(&root.join("stdout")).expect("forwarded");
    std::io::Write::write_all(&mut file, b"through").unwrap();
    drop(file);

    assert_eq!(std::fs::read(root.join("stdout")).unwrap(), b"through");
    let _ = std::fs::remove_dir_all(&root);
}

/// A rule that observes the open sees it and lets it proceed, so a test can
/// count how many handles a run opened.
#[test]
fn an_observed_output_open_is_recorded_and_proceeds() {
    let log = FaultLog::new();
    let fs = faulty(FaultPolicy::new().inject(
        FaultOp::OpenOutput,
        "/data/exec/stdout",
        Fault::record_into(&log),
        When::Always,
    ));

    let _ = fs.open_output(Path::new("/data/exec/stdout"));

    let records = log.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].op, FaultOp::OpenOutput);
}

/// The ranged read is injectable on its own, so a test can fail the tail of a growing file without
/// failing a whole-file read of it (`EO-18`).
#[test]
fn a_ranged_read_is_injectable_and_independent_of_the_whole_file_read() {
    let fs = faulty(FaultPolicy::new().inject(
        FaultOp::ReadAt,
        "/data/stdout",
        Fault::Fail(VfsErrorKind::Other),
        When::Always,
    ));
    fs.write(Path::new("/data/stdout"), b"0123456789").unwrap();

    let refused = fs
        .read_at(Path::new("/data/stdout"), 2, 4)
        .expect_err("the injected fault refuses the read");

    assert_eq!(refused.kind(), VfsErrorKind::Other);
    assert_eq!(refused.op(), "read_at");
    assert_eq!(fs.read(Path::new("/data/stdout")).unwrap(), b"0123456789");
}

#[test]
fn a_ranged_read_can_be_scripted_to_fail_only_on_its_nth_call() {
    let fs = faulty(FaultPolicy::new().inject(
        FaultOp::ReadAt,
        "/data/stdout",
        Fault::io_error(),
        When::OnCall(2),
    ));
    fs.write(Path::new("/data/stdout"), b"0123456789").unwrap();

    assert_eq!(fs.read_at(Path::new("/data/stdout"), 0, 3).unwrap(), b"012");
    assert!(fs.read_at(Path::new("/data/stdout"), 3, 3).is_err());
    assert_eq!(fs.read_at(Path::new("/data/stdout"), 3, 3).unwrap(), b"345");
}

#[test]
fn a_ranged_read_can_be_corrupted_and_truncated() {
    let fs = faulty(FaultPolicy::new().inject(
        FaultOp::ReadAt,
        "/data/stdout",
        Fault::corrupt(Corruption::Truncate { len: 2 }),
        When::Always,
    ));
    fs.write(Path::new("/data/stdout"), b"0123456789").unwrap();

    assert_eq!(fs.read_at(Path::new("/data/stdout"), 4, 5).unwrap(), b"45");
}

#[test]
fn the_decorator_forwards_the_ranged_read_and_what_it_wraps_being_the_os() {
    let fs = faulty(FaultPolicy::new());
    fs.write(Path::new("/data/f"), b"abcdef").unwrap();

    assert_eq!(fs.read_at(Path::new("/data/f"), 2, 2).unwrap(), b"cd");
    assert!(!fs.is_os_filesystem(), "memory, wrapped, is still memory");
    assert!(FaultyFs::new(crate::OsFs::new(), FaultPolicy::new()).is_os_filesystem());
}
