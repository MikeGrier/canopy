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
    let faulty: Arc<std::sync::OnceLock<Arc<FaultyFs<crate::mem::MemFs>>>> = Arc::default();
    let reached = Arc::clone(&faulty);
    let policy = FaultPolicy::new().inject_matching(
        FaultOp::Write,
        PathMatcher::predicate(move |path| {
            let fs = reached.get().expect("installed before the first call");
            fs.exists(&path.with_extension("flag")).unwrap_or(false)
        }),
        Fault::io_error(),
        When::Always,
    );
    let fs = faulty.get_or_init(|| Arc::new(FaultyFs::new(crate::mem::MemFs::new(), policy)));
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = Arc::clone(fs);
    std::thread::spawn(move || {
        worker.write(Path::new("a.flag"), b"").unwrap();
        let refused = worker.write(Path::new("a.data"), b"x").is_err();
        let allowed = worker.write(Path::new("b.data"), b"x").is_ok();
        let _ = tx.send((refused, allowed));
    });
    let (refused, allowed) =
        test_timers::mpsc::recv_timeout(&rx, std::time::Duration::from_secs(10))
            .expect("a predicate re-entering its decorator deadlocked");
    assert!(refused && allowed);
}
