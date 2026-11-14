// Copyright (c) Michael Grier.

//! The modification count: published whole, raised only by the outermost modification, odd while a
//! modification is open or the state cannot be vouched for (canopy's DESIGN-NOTES.md, "The
//! modification count, the generation and reads, modelled in `Coordination.tla`").

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::super::TempDir;
use super::{Backend, implementations, on_every_implementation, participant, participant_on};
use crate::coord::file::FileCoordination;
use crate::coord::file::count::raising::Modifications;
use crate::coord::file::restart::Standing;
use crate::coord::{CoordError, Coordination, ModCount, ModificationCount, ModificationEnd};
use crate::{
    Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, MemFs, OsFs, PathMatcher, Vfs, When,
};

const STAGED: &str = "count.staging.";

/// Publishes a vouched-for `value`, as a completed recovery leaves the count.
fn vouched(participant: &FileCoordination, value: u64) {
    participant.record_boot(Standing::Vouched).expect("vouch");
    participant
        .publish_count(value, Standing::Vouched)
        .expect("publish");
}

fn sample(participant: &FileCoordination) -> ModCount {
    participant.sample().expect("sample")
}

fn staged() -> PathMatcher {
    PathMatcher::FileNamePrefix(STAGED.into())
}

fn fenced(participant: &FileCoordination) -> bool {
    participant
        .vfs()
        .exists(&participant.layout().fence())
        .expect("exists")
}

#[test]
fn a_count_never_published_samples_odd() {
    on_every_implementation("coord-count-new", &|implementation, backend| {
        let sampled = sample(&participant(implementation, backend));
        assert!(!sampled.is_settled(), "{}: {sampled:?}", backend.name());
    });
}

#[test]
fn a_vouched_count_samples_its_value_in_every_participant() {
    on_every_implementation("coord-count-vouched", &|implementation, backend| {
        let publisher = participant(implementation, backend);
        vouched(&publisher, 42);
        for observer in [&publisher, &participant(implementation, backend)] {
            assert_eq!(sample(observer), ModCount::new(42), "{}", backend.name());
        }
    });
}

#[test]
fn a_count_no_recovery_vouched_for_samples_odd() {
    on_every_implementation("coord-count-untrusted", &|implementation, backend| {
        let publisher = participant(implementation, backend);
        publisher
            .publish_count(42, Standing::Untrusted)
            .expect("publish");
        assert_eq!(sample(&publisher), ModCount::new(43));
    });
}

#[test]
fn a_modification_holds_the_count_odd_from_its_beginning_to_its_end() {
    on_every_implementation("coord-count-raise", &|implementation, backend| {
        let holder = participant(implementation, backend);
        let sampler = participant(implementation, backend);
        vouched(&holder, 10);
        let mut open = Modifications::default();
        holder.begin_modification(&mut open).expect("begin");
        assert_eq!(sample(&sampler), ModCount::new(11));
        holder
            .end_modification(&mut open, ModificationEnd::Settled)
            .expect("end");
        assert_eq!(sample(&sampler), ModCount::new(12), "{}", backend.name());
        assert!(!fenced(&holder));
    });
}

#[test]
fn only_the_outermost_modification_raises_the_count() {
    on_every_implementation("coord-count-nested", &|implementation, backend| {
        let holder = participant(implementation, backend);
        vouched(&holder, 0);
        let mut open = Modifications::default();
        for _ in 0..3 {
            holder.begin_modification(&mut open).expect("begin");
        }
        for _ in 0..2 {
            holder
                .end_modification(&mut open, ModificationEnd::Settled)
                .expect("end");
            assert_eq!(
                sample(&holder),
                ModCount::new(1),
                "a nested end lowers nothing"
            );
        }
        holder
            .end_modification(&mut open, ModificationEnd::Settled)
            .expect("end");
        assert_eq!(sample(&holder), ModCount::new(2));
    });
}

#[test]
fn an_unresolved_modification_leaves_the_count_odd_and_sets_the_fence() {
    on_every_implementation("coord-count-unresolved", &|implementation, backend| {
        let holder = participant(implementation, backend);
        vouched(&holder, 6);
        let mut open = Modifications::default();
        holder.begin_modification(&mut open).expect("begin");
        holder
            .end_modification(&mut open, ModificationEnd::Unresolved)
            .expect("end");
        assert_eq!(sample(&holder), ModCount::new(7));
        assert!(fenced(&holder));
    });
}

#[test]
fn ending_a_modification_none_began_is_refused() {
    on_every_implementation("coord-count-none", &|implementation, backend| {
        let holder = participant(implementation, backend);
        vouched(&holder, 2);
        let mut open = Modifications::default();
        assert!(matches!(
            holder.end_modification(&mut open, ModificationEnd::Settled),
            Err(CoordError::NoModificationOpen)
        ));
        assert_eq!(sample(&holder), ModCount::new(2));
    });
}

#[test]
fn a_count_a_dead_holder_left_odd_stays_odd_through_a_modification() {
    on_every_implementation("coord-count-left-odd", &|implementation, backend| {
        let dead = participant(implementation, backend);
        vouched(&dead, 8);
        dead.begin_modification(&mut Modifications::default())
            .expect("begin");
        dead.abandon();
        let next = participant(implementation, backend);
        assert_eq!(sample(&next), ModCount::new(9), "{}", backend.name());
        let mut open = Modifications::default();
        next.begin_modification(&mut open).expect("begin");
        next.end_modification(&mut open, ModificationEnd::Settled)
            .expect("end");
        assert_eq!(sample(&next), ModCount::new(9), "only recovery lowers it");
    });
}

/// A participant over `mem` whose filesystem fails or observes the count's publication.
fn with_policy(
    implementation: &crate::coord::file::FileFactory,
    mem: &MemFs,
    policy: FaultPolicy,
) -> FileCoordination {
    participant_on(
        implementation,
        Arc::new(FaultyFs::new(mem.another_process(), policy)),
        &PathBuf::from(Backend::MEM_ROOT),
    )
}

fn plain(implementation: &crate::coord::file::FileFactory, mem: &MemFs) -> FileCoordination {
    participant_on(
        implementation,
        Arc::new(mem.another_process()),
        &PathBuf::from(Backend::MEM_ROOT),
    )
}

#[test]
fn a_sample_during_a_publication_sees_the_previous_count_whole() {
    for implementation in implementations() {
        let mem = Backend::mem();
        let sampler = plain(&implementation, &mem);
        vouched(&sampler, 20);
        let seen = Arc::new(Mutex::new(Vec::new()));
        for op in [FaultOp::Write, FaultOp::Rename] {
            let (log, observer) = (Arc::clone(&seen), plain(&implementation, &mem));
            let policy = FaultPolicy::new().inject_matching(
                op,
                staged(),
                Fault::observe_with(move |_, _| log.lock().unwrap().push(sample(&observer))),
                When::Always,
            );
            let holder = with_policy(&implementation, &mem, policy);
            let mut open = Modifications::default();
            holder.begin_modification(&mut open).expect("begin");
            holder
                .end_modification(&mut open, ModificationEnd::Settled)
                .expect("end");
        }
        let seen = seen.lock().unwrap().clone();
        assert_eq!(
            seen,
            [20, 21, 22, 23].map(ModCount::new),
            "every sample is a whole published value, the one before the rename"
        );
    }
}

#[test]
fn a_holder_killed_before_publishing_its_raise_leaves_the_count_as_it_was() {
    for implementation in implementations() {
        let mem = Backend::mem();
        let sampler = plain(&implementation, &mem);
        vouched(&sampler, 30);
        let policy = FaultPolicy::new().inject_matching(
            FaultOp::Rename,
            staged(),
            Fault::io_error(),
            When::Always,
        );
        let holder = with_policy(&implementation, &mem, policy);
        let mut open = Modifications::default();
        assert!(holder.begin_modification(&mut open).is_err());
        holder.abandon();
        assert_eq!(sample(&sampler), ModCount::new(30));
        let staged_left = sampler
            .vfs()
            .read_dir(&sampler.layout().state_dir())
            .expect("list")
            .into_iter()
            .any(|entry| entry.file_name.to_string_lossy().starts_with(STAGED));
        assert!(!staged_left, "a failed publication removes what it staged");
    }
}

#[test]
fn a_failed_opening_publication_opens_no_modification() {
    for implementation in implementations() {
        for op in [FaultOp::Write, FaultOp::Rename] {
            let mem = Backend::mem();
            let policy =
                FaultPolicy::new().inject_matching(op, staged(), Fault::io_error(), When::Once);
            let holder = with_policy(&implementation, &mem, policy);
            let setup = plain(&implementation, &mem);
            vouched(&setup, 4);
            let mut open = Modifications::default();
            assert!(holder.begin_modification(&mut open).is_err(), "{op:?}");
            assert_eq!(sample(&holder), ModCount::new(4));
            assert!(matches!(
                holder.end_modification(&mut open, ModificationEnd::Settled),
                Err(CoordError::NoModificationOpen)
            ));
        }
    }
}

#[test]
fn a_failed_closing_publication_leaves_the_count_odd_and_sets_the_fence() {
    for implementation in implementations() {
        for op in [FaultOp::Write, FaultOp::Rename] {
            let mem = Backend::mem();
            let gate = FaultGate::closed();
            let policy = FaultPolicy::new().inject_gated(
                op,
                staged(),
                Fault::io_error(),
                When::Always,
                gate.clone(),
            );
            let holder = with_policy(&implementation, &mem, policy);
            vouched(&plain(&implementation, &mem), 14);
            let mut open = Modifications::default();
            holder.begin_modification(&mut open).expect("begin");
            gate.open();
            assert!(
                holder
                    .end_modification(&mut open, ModificationEnd::Settled)
                    .is_err(),
                "{op:?}"
            );
            assert_eq!(sample(&holder), ModCount::new(15));
            assert!(fenced(&holder), "{op:?}");
        }
    }
}

#[test]
fn a_sample_after_a_restart_reads_odd() {
    for implementation in implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        let publisher = plain(&implementation, &mem);
        vouched(&publisher, 50);
        let layout = publisher.layout();
        for path in [layout.count(), layout.state_dir(), root.clone()] {
            mem.fsync(&path).expect("sync");
        }
        assert_eq!(sample(&publisher), ModCount::new(50));
        mem.simulate_crash();
        assert_eq!(
            sample(&publisher),
            ModCount::new(51),
            "the record survived, from the boot before"
        );
    }
}

#[test]
fn a_record_that_is_not_one_samples_odd() {
    on_every_implementation("coord-count-garbage", &|implementation, backend| {
        let publisher = participant(implementation, backend);
        vouched(&publisher, 2);
        let count = publisher.layout().count();
        let good = publisher.vfs().read(&count).expect("the record");
        let mut wrong_standing = good.clone();
        wrong_standing[4] = 9;
        for garbage in [
            Vec::new(),
            good[..8].to_vec(),
            b"ACCX\x01".to_vec(),
            wrong_standing,
        ] {
            publisher.vfs().write(&count, &garbage).expect("garble");
            assert!(!sample(&publisher).is_settled(), "{garbage:?}");
        }
    });
}

#[test]
fn an_abandoned_participant_neither_samples_nor_publishes() {
    on_every_implementation("coord-count-dead", &|implementation, backend| {
        let participant = participant(implementation, backend);
        participant.abandon();
        assert!(matches!(participant.sample(), Err(CoordError::Abandoned)));
        assert!(matches!(
            participant.publish_count(2, Standing::Vouched),
            Err(CoordError::Abandoned)
        ));
        assert!(matches!(
            participant.begin_modification(&mut Modifications::default()),
            Err(CoordError::Abandoned)
        ));
    });
}

#[test]
fn a_reader_with_the_published_count_open_reads_it_whole_while_it_is_replaced() {
    use std::io::Read;
    for implementation in implementations() {
        let dir = TempDir::new("coord-count-open");
        let vfs: Arc<dyn Vfs> = Arc::new(OsFs::new());
        let holder = participant_on(&implementation, Arc::clone(&vfs), dir.path());
        vouched(&holder, 60);
        let before = vfs.read(&holder.layout().count()).expect("the record");
        // The seam opens and closes a file within one call, so a reader holding it open across a
        // replacement is reached only through the operating system.
        let mut open_reader = std::fs::File::open(holder.layout().count()).expect("open");
        let mut modifications = Modifications::default();
        holder
            .begin_modification(&mut modifications)
            .expect("the record is replaced while a reader has it open");
        let mut read = Vec::new();
        open_reader.read_to_end(&mut read).expect("read");
        assert_eq!(read, before, "the open reader reads the record it opened");
        assert_eq!(sample(&holder), ModCount::new(61));
    }
}
