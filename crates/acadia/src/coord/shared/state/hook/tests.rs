// Copyright (c) Michael Grier.

use super::*;
use crate::MemFs;
use crate::coord::shared::SharedFactory;
use crate::coord::{Coordination, CoordinationFactory, ModificationEnd, Purpose, Waiting};

const ROOT: &str = "/store";

/// Records each step it is told of, with the words as they stood before the step acted.
struct Recording {
    vfs: Arc<dyn Vfs>,
    seen: Mutex<Vec<(SharedStep, Option<SharedWords>)>>,
}

impl StepHook for Recording {
    fn before(&self, step: SharedStep) {
        let words = inspect(self.vfs.as_ref(), Path::new(ROOT)).expect("the words are readable");
        self.seen.lock().unwrap().push((step, words));
    }
}

impl Recording {
    fn take(&self) -> Vec<SharedStep> {
        std::mem::take(&mut *self.seen.lock().unwrap())
            .into_iter()
            .map(|(step, _)| step)
            .collect()
    }

    fn take_words(&self) -> Vec<(SharedStep, SharedWords)> {
        std::mem::take(&mut *self.seen.lock().unwrap())
            .into_iter()
            .map(|(step, words)| (step, words.expect("a step finds the words made")))
            .collect()
    }
}

fn store() -> MemFs {
    let fs = MemFs::new();
    fs.create_dir_all(Path::new(ROOT)).unwrap();
    fs
}

fn hooked(fs: &MemFs) -> (Arc<Recording>, Box<dyn Coordination>) {
    let vfs: Arc<dyn Vfs> = Arc::new(fs.another_process());
    let recording = Arc::new(Recording {
        vfs: Arc::new(fs.another_process()),
        seen: Mutex::new(Vec::new()),
    });
    let participant = SharedFactory::new()
        .with_hook(recording.clone())
        .open(vfs, Path::new(ROOT))
        .unwrap();
    (recording, participant)
}

/// A participant whose first holder has recovered the state and let go.
fn recovered(fs: &MemFs) -> (Arc<Recording>, Box<dyn Coordination>) {
    let (recording, participant) = hooked(fs);
    let mut hold = participant.acquire(Waiting::Wait).unwrap();
    hold.complete_recovery().unwrap();
    hold.release().unwrap();
    recording.take();
    (recording, participant)
}

fn words(fs: &MemFs) -> SharedWords {
    inspect(fs, Path::new(ROOT))
        .unwrap()
        .expect("a participant made the words")
}

#[test]
fn no_words_are_found_before_a_participant_makes_them() {
    let fs = store();
    assert_eq!(inspect(&fs, Path::new(ROOT)).unwrap(), None);
    let (_, participant) = hooked(&fs);
    assert_eq!(inspect(&fs, Path::new(ROOT)).unwrap(), None);
    drop(participant.acquire(Waiting::Wait).unwrap());
    assert!(inspect(&fs, Path::new(ROOT)).unwrap().is_some());
}

#[test]
fn new_state_is_untrusted_and_fenced_until_recovered() {
    let fs = store();
    let (recording, participant) = hooked(&fs);
    let mut hold = participant.acquire(Waiting::Wait).unwrap();
    assert_eq!(recording.take(), [SharedStep::SetFence, SharedStep::Record]);
    let found = words(&fs);
    assert_ne!(found.trust, Trust::Vouched);
    assert!(found.fence);
    assert_eq!(found.owner, found.generation);
    hold.complete_recovery().unwrap();
    assert_eq!(
        recording.take(),
        [SharedStep::PublishRecovered, SharedStep::ClearFence]
    );
    let found = words(&fs);
    assert_eq!(found.trust, Trust::Vouched);
    assert!(!found.fence);
    assert!(found.count.is_multiple_of(2));
}

#[test]
fn a_clean_release_clears_the_record() {
    let fs = store();
    let (recording, participant) = recovered(&fs);
    let hold = participant.acquire(Waiting::Wait).unwrap();
    assert_eq!(recording.take(), [SharedStep::Record]);
    assert_ne!(words(&fs).owner, 0);
    hold.release().unwrap();
    assert_eq!(recording.take(), [SharedStep::ClearRecord]);
    assert_eq!(words(&fs).owner, 0);
}

#[test]
fn a_settled_operation_raises_and_closes_the_count() {
    let fs = store();
    let (recording, participant) = recovered(&fs);
    let mut hold = participant.acquire(Waiting::Wait).unwrap();
    let before = words(&fs).count;
    recording.take();
    hold.begin_modification(Purpose::Ordinary).unwrap();
    assert_eq!(
        recording.take(),
        [SharedStep::BeginRaise, SharedStep::PublishRaise]
    );
    assert_eq!(words(&fs).count, before + 1);
    hold.end_modification(ModificationEnd::Settled).unwrap();
    assert_eq!(
        recording.take(),
        [SharedStep::BeginClose, SharedStep::PublishClose]
    );
    assert_eq!(words(&fs).count, before + 2);
}

#[test]
fn a_nested_operation_announces_nothing() {
    let fs = store();
    let (recording, participant) = recovered(&fs);
    let mut hold = participant.acquire(Waiting::Wait).unwrap();
    hold.begin_modification(Purpose::Ordinary).unwrap();
    recording.take();
    hold.begin_modification(Purpose::Ordinary).unwrap();
    hold.end_modification(ModificationEnd::Settled).unwrap();
    assert_eq!(recording.take(), []);
    hold.end_modification(ModificationEnd::Settled).unwrap();
    assert_eq!(
        recording.take(),
        [SharedStep::BeginClose, SharedStep::PublishClose]
    );
}

#[test]
fn an_unresolved_operation_sets_the_fence_and_leaves_the_count_odd() {
    let fs = store();
    let (recording, participant) = recovered(&fs);
    let mut hold = participant.acquire(Waiting::Wait).unwrap();
    hold.begin_modification(Purpose::Ordinary).unwrap();
    recording.take();
    hold.end_modification(ModificationEnd::Unresolved).unwrap();
    assert_eq!(recording.take(), [SharedStep::Unresolved]);
    let found = words(&fs);
    assert!(found.fence);
    assert!(!found.count.is_multiple_of(2));
}

#[test]
fn a_holder_found_dead_is_fenced_before_the_record_is_replaced() {
    let fs = store();
    // The state lives while any participant does, so the dead holder's evidence outlives it.
    let (_, _keeper) = recovered(&fs);
    let first = SharedFactory::new()
        .open(Arc::new(fs.another_process()), Path::new(ROOT))
        .unwrap();
    let hold = first.acquire(Waiting::Wait).unwrap();
    first.abandon();
    drop(hold);
    let (recording, second) = hooked(&fs);
    drop(second.acquire(Waiting::Wait).unwrap());
    let seen = recording.take_words();
    let steps: Vec<SharedStep> = seen.iter().map(|(step, _)| *step).collect();
    assert_eq!(
        steps,
        [
            SharedStep::SetFence,
            SharedStep::Record,
            SharedStep::ClearRecord
        ]
    );
    let (_, at_fence) = seen[0];
    assert!(!at_fence.fence, "the fence is told of before it is set");
    let (_, at_record) = seen[1];
    assert!(
        at_record.fence,
        "the record is replaced only once the fence is set"
    );
}

#[test]
fn each_step_is_told_of_before_it_acts() {
    let fs = store();
    let (recording, participant) = recovered(&fs);
    let mut hold = participant.acquire(Waiting::Wait).unwrap();
    hold.begin_modification(Purpose::Ordinary).unwrap();
    hold.end_modification(ModificationEnd::Settled).unwrap();
    hold.release().unwrap();
    let seen = recording.take_words();
    let at = |wanted: SharedStep| {
        seen.iter()
            .find(|(step, _)| *step == wanted)
            .map(|(_, words)| *words)
            .unwrap_or_else(|| panic!("{wanted:?} was not told of"))
    };
    let raise = at(SharedStep::PublishRaise);
    assert!(
        raise.count.is_multiple_of(2),
        "the raise is not yet published"
    );
    let close = at(SharedStep::PublishClose);
    assert_eq!(
        close.count,
        raise.count + 1,
        "the close is not yet published"
    );
    assert_ne!(
        at(SharedStep::ClearRecord).owner,
        0,
        "the record is not yet cleared"
    );
}

#[test]
fn a_host_restart_leaves_no_words_until_a_participant_makes_them_again() {
    let fs = store();
    let (_, participant) = recovered(&fs);
    drop(participant);
    fs.simulate_crash();
    assert_eq!(inspect(&fs, Path::new(ROOT)).unwrap(), None);
    let (recording, participant) = hooked(&fs);
    let hold = participant.acquire(Waiting::Wait).unwrap();
    assert_eq!(recording.take(), [SharedStep::SetFence, SharedStep::Record]);
    assert_ne!(words(&fs).trust, Trust::Vouched);
    drop(hold);
}

#[test]
fn a_participant_without_a_hook_is_read_alike() {
    let fs = store();
    let participant = SharedFactory::new()
        .open(Arc::new(fs.another_process()), Path::new(ROOT))
        .unwrap();
    let mut hold = participant.acquire(Waiting::Wait).unwrap();
    hold.complete_recovery().unwrap();
    let found = words(&fs);
    assert_eq!(found.trust, Trust::Vouched);
    assert_eq!(found.owner, found.generation);
    hold.release().unwrap();
    assert_eq!(words(&fs).owner, 0);
}
