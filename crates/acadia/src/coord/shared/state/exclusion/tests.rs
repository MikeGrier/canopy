// Copyright (c) Michael Grier.

use super::*;

fn participants() -> (Arc<crate::MemFs>, SharedState, SharedState) {
    let fs = Arc::new(crate::MemFs::new());
    fs.create_dir_all(Path::new("/exclusion")).unwrap();
    let first = SharedState::open(fs.clone(), Path::new("/exclusion")).unwrap();
    let second = SharedState::open(fs.clone(), Path::new("/exclusion")).unwrap();
    first
        .acquire(Waiting::Wait)
        .unwrap()
        .complete_recovery()
        .unwrap();
    (fs, first, second)
}

#[test]
fn sequential_holds_advance_once_and_never_report_a_clean_holder_dead() {
    let (_, first, second) = participants();
    for index in 0..12 {
        let participant = if index % 2 == 0 { &first } else { &second };
        let before = participant.snapshot().unwrap();
        let hold = participant.acquire(Waiting::Never).unwrap();
        assert_eq!(hold.acquired().found().get(), before.generation);
        assert_eq!(hold.acquired().advanced_to().get(), before.generation + 1);
        assert!(!hold.acquired().holder_died());
        assert!(matches!(
            second.acquire(Waiting::Never),
            Err(CoordError::WouldWait)
        ));
        hold.release().unwrap();
        assert_eq!(first.snapshot().unwrap().owner, 0);
    }
}

#[test]
fn nested_unresolved_modifications_never_restore_even_parity() {
    let (_, first, second) = participants();
    let mut hold = first.acquire(Waiting::Wait).unwrap();
    let before = second.snapshot().unwrap().modification_count;
    hold.begin_modification(Purpose::Ordinary).unwrap();
    hold.begin_modification(Purpose::Ordinary).unwrap();
    assert_eq!(second.snapshot().unwrap().modification_count, before + 1);
    hold.end_modification(ModificationEnd::Unresolved).unwrap();
    assert!(second.snapshot().unwrap().fenced);
    hold.end_modification(ModificationEnd::Settled).unwrap();
    assert_eq!(second.snapshot().unwrap().modification_count, before + 1);
    assert!(matches!(
        hold.begin_modification(Purpose::Ordinary),
        Err(CoordError::RecoveryRequired)
    ));
    hold.complete_recovery().unwrap();
    assert!(!second.snapshot().unwrap().fenced);
    assert_eq!(second.snapshot().unwrap().modification_count, before + 2);
}

#[test]
fn abandoned_ownership_is_reported_and_fenced_before_replacement() {
    let (_, first, second) = participants();
    let old = first.acquire(Waiting::Wait).unwrap();
    first.abandon();
    assert!(second.discover_recovery().unwrap().holder_died());
    let mut next = second.acquire(Waiting::Never).unwrap();
    assert!(next.acquired().holder_died());
    assert!(next.acquired().fenced());
    drop(old);
    assert_eq!(
        second.snapshot().unwrap().owner,
        next.acquired().advanced_to().get()
    );
    assert!(matches!(
        next.begin_modification(Purpose::Ordinary),
        Err(CoordError::RecoveryRequired)
    ));
    next.begin_modification(Purpose::Recovery).unwrap();
    assert!(matches!(
        next.complete_recovery(),
        Err(CoordError::ModificationOpen)
    ));
    next.end_modification(ModificationEnd::Settled).unwrap();
    next.complete_recovery().unwrap();
}

#[test]
fn discovery_inspects_only_free_exclusion_and_changes_no_generation() {
    let (_, first, second) = participants();
    let before = second.snapshot().unwrap();
    let mut inspected = false;
    assert!(
        !second
            .discover_recovery_with(&mut || inspected = true)
            .unwrap()
            .required()
    );
    assert!(inspected);
    assert_eq!(second.snapshot().unwrap().generation, before.generation);
    let hold = first.acquire(Waiting::Wait).unwrap();
    inspected = false;
    assert!(
        !second
            .discover_recovery_with(&mut || inspected = true)
            .unwrap()
            .holder_died()
    );
    assert!(!inspected);
    drop(hold);
}

#[test]
fn a_hold_keeps_the_participant_live_and_an_open_drop_preserves_recovery() {
    let (_, first, second) = participants();
    let mut hold = first.acquire(Waiting::Wait).unwrap();
    drop(first);
    hold.begin_modification(Purpose::Ordinary).unwrap();
    drop(hold);
    assert!(second.snapshot().unwrap().fenced);
    assert!(!second.discover_recovery().unwrap().holder_died());
    second
        .acquire(Waiting::Wait)
        .unwrap()
        .complete_recovery()
        .unwrap();
}

#[test]
fn cold_epochs_never_reuse_an_acquired_generation_and_live_ranges_can_renew() {
    let fs = Arc::new(crate::MemFs::new());
    fs.create_dir_all(Path::new("/generation-epochs")).unwrap();
    let first = SharedState::open(fs.clone(), Path::new("/generation-epochs")).unwrap();
    let hold = first.acquire(Waiting::Never).unwrap();
    let previous = hold.acquired().advanced_to();
    drop(hold);
    drop(first);
    let next = SharedState::open(fs, Path::new("/generation-epochs")).unwrap();
    let hold = next.acquire(Waiting::Never).unwrap();
    assert!(hold.acquired().found() > previous);
    drop(hold);
    let limit = next
        .words
        .word(Word::GenerationLimit)
        .load(Ordering::Acquire);
    next.words
        .word(Word::Generation)
        .store(limit, Ordering::Release);
    let renewed = next.acquire(Waiting::Never).unwrap();
    assert_eq!(renewed.acquired().found().get(), limit);
    assert_eq!(renewed.acquired().advanced_to().get(), limit + 1);
    assert!(
        next.words
            .word(Word::GenerationLimit)
            .load(Ordering::Acquire)
            > limit
    );
}

#[test]
fn an_unreadable_generation_reservation_cannot_be_reused_by_a_new_epoch() {
    let fs = Arc::new(crate::MemFs::new());
    fs.create_dir_all(Path::new("/generation-corrupt")).unwrap();
    drop(SharedState::open(fs.clone(), Path::new("/generation-corrupt")).unwrap());
    fs.write(
        &Path::new("/generation-corrupt")
            .join(DIRECTORY)
            .join("generation-range"),
        b"bad",
    )
    .unwrap();
    assert!(matches!(
        SharedState::open(fs, Path::new("/generation-corrupt")),
        Err(CoordError::Malformed { .. })
    ));
}

#[test]
fn generation_and_count_exhaustion_do_not_wrap() {
    let (_, first, second) = participants();
    first
        .words
        .word(Word::Generation)
        .store(u64::MAX, Ordering::Release);
    assert!(first.acquire(Waiting::Never).is_err());
    assert_eq!(second.snapshot().unwrap().generation, u64::MAX);
    assert_eq!(second.snapshot().unwrap().owner, 0);
    first
        .words
        .word(Word::Generation)
        .store(10, Ordering::Release);
    let mut hold = first.acquire(Waiting::Wait).unwrap();
    first
        .words
        .word(Word::Count)
        .store(u64::MAX - 1, Ordering::Release);
    assert!(matches!(
        hold.begin_modification(Purpose::Ordinary),
        Err(CoordError::Exhausted { .. })
    ));
    assert_eq!(second.snapshot().unwrap().modification_count, u64::MAX - 1);
    first
        .words
        .word(Word::Count)
        .store(u64::MAX, Ordering::Release);
    assert_eq!(second.snapshot().unwrap().modification_count, u64::MAX);
    assert!(matches!(
        hold.complete_recovery(),
        Err(CoordError::Exhausted { .. })
    ));
}

#[test]
fn new_epochs_refuse_ordinary_work_and_empty_ends_change_nothing() {
    let fs = Arc::new(crate::MemFs::new());
    fs.create_dir_all(Path::new("/new-epoch")).unwrap();
    let state = SharedState::open(fs, Path::new("/new-epoch")).unwrap();
    let mut hold = state.acquire(Waiting::Never).unwrap();
    assert!(hold.acquired().recovery_required());
    let before = state.snapshot().unwrap().modification_count;
    assert!(matches!(
        hold.begin_modification(Purpose::Ordinary),
        Err(CoordError::RecoveryRequired)
    ));
    assert!(matches!(
        hold.end_modification(ModificationEnd::Settled),
        Err(CoordError::NoModificationOpen)
    ));
    assert_eq!(state.snapshot().unwrap().modification_count, before);
    hold.begin_modification(Purpose::Recovery).unwrap();
    hold.end_modification(ModificationEnd::Settled).unwrap();
    assert_eq!(state.snapshot().unwrap().modification_count, before);
    hold.complete_recovery().unwrap();
    assert!(
        state
            .snapshot()
            .unwrap()
            .modification_count
            .is_multiple_of(2)
    );
    hold.begin_modification(Purpose::Ordinary).unwrap();
    hold.end_modification(ModificationEnd::Settled).unwrap();
}

#[test]
fn a_panicking_discovery_inspector_releases_its_probe_without_bookkeeping() {
    let (_, first, second) = participants();
    let before = first.snapshot().unwrap();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = first.discover_recovery_with(&mut || panic!("inspection stopped"));
        }))
        .is_err()
    );
    assert_eq!(second.snapshot().unwrap().generation, before.generation);
    assert_eq!(second.snapshot().unwrap().owner, before.owner);
    assert!(second.acquire(Waiting::Never).is_ok());
}

#[test]
fn a_modeled_restart_invalidates_old_holds_before_any_new_effect() {
    let (fs, first, second) = participants();
    let mut old = first.acquire(Waiting::Wait).unwrap();
    fs.simulate_crash();
    assert!(matches!(
        old.begin_modification(Purpose::Recovery),
        Err(CoordError::Abandoned)
    ));
    assert!(matches!(
        second.acquire(Waiting::Never),
        Err(CoordError::Abandoned)
    ));
    first.abandon();
    second.abandon();
    let replacement = SharedState::open(fs, Path::new("/exclusion")).unwrap();
    let mut current = replacement.acquire(Waiting::Wait).unwrap();
    current.complete_recovery().unwrap();
    drop(old);
    assert_eq!(
        replacement.snapshot().unwrap().owner,
        current.acquired().advanced_to().get()
    );
}
