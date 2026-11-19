// Copyright (c) Michael Grier.

use super::*;

#[test]
fn on_call_fires_on_the_named_call_and_never_after_the_count_saturates() {
    assert!(should_fire(When::OnCall(u32::MAX), u32::MAX - 1));
    assert!(!should_fire(When::OnCall(u32::MAX), u32::MAX));
    assert!(should_fire(When::OnCall(1), 0));
    assert!(!should_fire(When::OnCall(1), 1));
    assert!(!should_fire(When::OnCall(0), 0), "call 0 names no call");
    assert!(!should_fire(When::OnCall(0), u32::MAX));
}

#[test]
fn a_saturated_count_keeps_once_and_times_spent() {
    assert!(!should_fire(When::Once, u32::MAX));
    assert!(!should_fire(When::Times(u32::MAX), u32::MAX));
    assert!(should_fire(When::Times(u32::MAX), u32::MAX - 1));
    assert!(should_fire(When::Always, u32::MAX));
}

#[test]
fn a_predicate_may_use_the_decorator_it_is_installed_in() {
    let weak_fs = {
        let faulty: Arc<std::sync::OnceLock<Arc<FaultyFs<crate::mem::MemFs>>>> = Arc::default();
        let reached = Arc::downgrade(&faulty);
        let policy = FaultPolicy::new().inject_matching(
            FaultOp::Write,
            PathMatcher::predicate(move |path| {
                let faulty = reached.upgrade().expect("fixture lives through the call");
                let fs = faulty.get().expect("installed before the first call");
                fs.exists(&path.with_extension("flag")).unwrap_or(false)
            }),
            Fault::io_error(),
            When::Always,
        );
        let fs = faulty.get_or_init(|| Arc::new(FaultyFs::new(crate::mem::MemFs::new(), policy)));
        let weak_fs = Arc::downgrade(fs);
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = Arc::clone(fs);
        let thread = std::thread::spawn(move || {
            worker.write(Path::new("a.flag"), b"").unwrap();
            let refused = worker.write(Path::new("a.data"), b"x").is_err();
            let allowed = worker.write(Path::new("b.data"), b"x").is_ok();
            let _ = tx.send((refused, allowed));
        });
        let (refused, allowed) =
            test_timers::mpsc::recv_timeout(&rx, std::time::Duration::from_secs(10))
                .expect("a predicate re-entering its decorator deadlocked");
        thread.join().expect("predicate worker");
        assert!(refused && allowed);
        weak_fs
    };
    assert!(
        weak_fs.upgrade().is_none(),
        "the predicate retained the fixture"
    );
}

#[test]
fn read_at_replacement_never_exceeds_the_readable_slice() {
    let replacement = b"replacement is much longer";
    for (offset, len) in [(0, 0), (1, 1), (4, 3), (20, 2)] {
        let inner = crate::MemFs::new();
        inner.write(Path::new("/range"), b"abcdef").unwrap();
        let bound = inner
            .read_at(Path::new("/range"), offset, len)
            .unwrap()
            .len();
        let policy = FaultPolicy::new().inject(
            FaultOp::ReadAt,
            "/range",
            Fault::corrupt(Corruption::Replace(replacement.to_vec())),
            When::Always,
        );
        let faulty = FaultyFs::new(inner, policy);
        assert_eq!(
            faulty.read_at(Path::new("/range"), offset, len).unwrap(),
            replacement[..bound].to_vec(),
            "offset={offset}, len={len}"
        );
    }
}
