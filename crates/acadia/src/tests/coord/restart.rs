// Copyright (c) Michael Grier.

//! Restart recognition: state is trusted only within the boot it was made in, and only once a
//! recovery has vouched for it.

use std::path::PathBuf;
use std::sync::Arc;

use super::{Backend, implementations, on_every_implementation, participant, participant_on};
use crate::coord::file::restart::Standing;
use crate::coord::{CoordError, Coordination, RestartRecognition, Trust, Untrusted};
use crate::{
    Fault, FaultOp, FaultPolicy, FaultyFs, MemFs, OsFs, PathMatcher, Vfs, VfsErrorKind, When,
};

fn untrusted(why: Untrusted) -> Trust {
    Trust::Untrusted(why)
}

#[test]
fn state_never_made_is_untrusted() {
    on_every_implementation("coord-trust-new", &|implementation, backend| {
        let participant = participant(implementation, backend);
        assert_eq!(
            participant.trust().expect("trust"),
            untrusted(Untrusted::Created),
            "{}",
            backend.name()
        );
    });
}

#[test]
fn state_made_in_this_boot_is_trusted_only_once_vouched() {
    on_every_implementation("coord-trust-vouched", &|implementation, backend| {
        let maker = participant(implementation, backend);
        let other = participant(implementation, backend);
        maker.record_boot(Standing::Untrusted).expect("record");
        for observer in [&maker, &other] {
            assert_eq!(
                observer.trust().expect("trust"),
                untrusted(Untrusted::Created)
            );
        }
        maker.record_boot(Standing::Vouched).expect("vouch");
        for observer in [&maker, &other] {
            assert_eq!(
                observer.trust().expect("trust"),
                Trust::Vouched,
                "{}",
                backend.name()
            );
        }
        other.record_boot(Standing::Untrusted).expect("made again");
        assert_eq!(maker.trust().expect("trust"), untrusted(Untrusted::Created));
    });
}

#[test]
fn a_record_is_replaced_whole_and_leaves_nothing_staged() {
    on_every_implementation("coord-trust-staging", &|implementation, backend| {
        let participant = participant(implementation, backend);
        for standing in [Standing::Untrusted, Standing::Vouched, Standing::Untrusted] {
            participant.record_boot(standing).expect("record");
        }
        let names: Vec<_> = participant
            .vfs()
            .read_dir(&participant.layout().state_dir())
            .expect("list the state")
            .into_iter()
            .map(|entry| entry.file_name)
            .collect();
        assert_eq!(names, ["boot"], "{}", backend.name());
    });
}

/// Records vouched-for state over `mem`, made durable so that a crash keeps it.
fn vouched_and_durable(mem: &MemFs, root: &std::path::Path) {
    for implementation in implementations() {
        let participant = participant_on(&implementation, Arc::new(mem.another_process()), root);
        participant.record_boot(Standing::Vouched).expect("vouch");
        let layout = participant.layout();
        for path in [layout.boot(), layout.state_dir(), root.to_path_buf()] {
            mem.fsync(&path).expect("sync");
        }
    }
}

#[test]
fn a_restart_leaves_no_state_trusted() {
    for implementation in implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        vouched_and_durable(&mem, &root);
        let participant = participant_on(&implementation, Arc::new(mem.another_process()), &root);
        assert_eq!(participant.trust().expect("trust"), Trust::Vouched);
        mem.simulate_crash();
        assert_eq!(
            participant.trust().expect("trust"),
            untrusted(Untrusted::HostRestarted),
            "a record that survived the crash names the boot before it"
        );
        participant
            .record_boot(Standing::Vouched)
            .expect("vouch again");
        assert_eq!(participant.trust().expect("trust"), Trust::Vouched);
    }
}

#[test]
fn a_restart_that_loses_the_record_leaves_the_state_untrusted() {
    for implementation in implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        let participant = participant_on(&implementation, Arc::new(mem.another_process()), &root);
        participant.record_boot(Standing::Vouched).expect("vouch");
        mem.simulate_crash();
        assert!(matches!(
            participant.trust().expect("trust"),
            Trust::Untrusted(_)
        ));
    }
}

fn boot_identity_failing(mem: &MemFs, kind: VfsErrorKind) -> Arc<dyn Vfs> {
    let policy = FaultPolicy::new().inject_matching(
        FaultOp::BootIdentity,
        PathMatcher::Predicate(Arc::new(|_| true)),
        Fault::error(kind),
        When::Always,
    );
    Arc::new(FaultyFs::new(mem.another_process(), policy))
}

#[test]
fn a_filesystem_that_cannot_identify_a_boot_never_trusts_the_state() {
    for implementation in implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        vouched_and_durable(&mem, &root);
        let blind = participant_on(
            &implementation,
            boot_identity_failing(&mem, VfsErrorKind::Unsupported),
            &root,
        );
        assert_eq!(
            blind.trust().expect("trust"),
            untrusted(Untrusted::CannotTell)
        );
        assert!(matches!(
            blind.record_boot(Standing::Vouched),
            Err(CoordError::Vfs(error)) if error.kind() == VfsErrorKind::Unsupported
        ));
    }
}

#[test]
fn a_boot_identity_that_fails_is_an_error() {
    for implementation in implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        let failing = participant_on(
            &implementation,
            boot_identity_failing(&mem, VfsErrorKind::Other),
            &root,
        );
        assert!(matches!(failing.trust(), Err(CoordError::Vfs(_))));
    }
}

#[test]
fn a_record_that_is_not_one_is_not_trusted() {
    on_every_implementation("coord-trust-garbage", &|implementation, backend| {
        let participant = participant(implementation, backend);
        participant.record_boot(Standing::Vouched).expect("vouch");
        let boot = participant.layout().boot();
        let good = participant.vfs().read(&boot).expect("the record");
        let mut wrong_standing = good.clone();
        wrong_standing[4] = 7;
        for garbage in [
            Vec::new(),
            b"ACB".to_vec(),
            b"XXXX\x01".to_vec(),
            wrong_standing,
        ] {
            participant.vfs().write(&boot, &garbage).expect("garble");
            assert_eq!(
                participant.trust().expect("trust"),
                untrusted(Untrusted::CannotTell),
                "{garbage:?}"
            );
        }
    });
}

#[test]
fn an_abandoned_participant_cannot_say() {
    on_every_implementation("coord-trust-dead", &|implementation, backend| {
        let participant = participant(implementation, backend);
        participant.abandon();
        assert!(matches!(participant.trust(), Err(CoordError::Abandoned)));
        assert!(matches!(
            participant.record_boot(Standing::Vouched),
            Err(CoordError::Abandoned)
        ));
    });
}

#[test]
fn a_boot_identity_is_shared_by_every_view_and_changes_only_with_a_crash() {
    let mem = MemFs::new();
    let first = mem.boot_identity().expect("an identity");
    assert_eq!(mem.another_process().boot_identity().expect("same"), first);
    assert_eq!(mem.boot_identity().expect("same"), first);
    let fork = mem.fork();
    mem.simulate_crash();
    let second = mem.boot_identity().expect("an identity");
    assert_ne!(second, first);
    assert_eq!(mem.another_process().boot_identity().expect("same"), second);
    assert_eq!(
        fork.boot_identity().expect("the fork's"),
        first,
        "a fork has its own boots"
    );
}

#[test]
fn a_fault_decorator_forwards_the_boot_identity() {
    let mem = MemFs::new();
    let faulty = FaultyFs::new(mem.another_process(), FaultPolicy::new());
    assert_eq!(
        faulty.boot_identity().expect("forwarded"),
        mem.boot_identity().expect("an identity")
    );
}

#[cfg(any(target_os = "linux", windows))]
#[test]
fn the_operating_system_names_this_boot_the_same_way_every_time() {
    let first = OsFs::new().boot_identity().expect("an identity");
    assert!(!first.is_empty());
    for _ in 0..3 {
        assert_eq!(OsFs::new().boot_identity().expect("an identity"), first);
    }
}
