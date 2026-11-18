// Copyright (c) Michael Grier.

use super::*;
use crate::coord::{
    Access, ArtifactMaker, CoordError, Coordination, Created, CreationNonce, CreationPlan,
    ModificationEnd, Purpose, ReaderSafety, Registration,
};
use std::ffi::OsStr;
use std::time::Instant;

fn opened(factory: &dyn CoordinationFactory, backend: &Backend) -> Box<dyn Coordination> {
    factory
        .open(backend.process(), &backend.root())
        .expect("open participant")
}

#[test]
fn public_count_and_generation_contract_on_every_implementation_and_backend() {
    on_every_contract("coord-contract-count", &|factory, backend| {
        let first = opened(factory, backend);
        let second = opened(factory, backend);
        assert!(!first.sample().unwrap().is_settled());
        let mut initial = first.acquire(crate::coord::Waiting::Never).unwrap();
        assert!(initial.acquired().recovery_required());
        assert!(matches!(
            initial.begin_modification(Purpose::Ordinary),
            Err(CoordError::RecoveryRequired)
        ));
        initial.complete_recovery().unwrap();
        let mut generation = initial.acquired().advanced_to();
        initial.release().unwrap();
        for _ in 0..12 {
            let before = second.sample().unwrap();
            let mut hold = first.acquire(crate::coord::Waiting::Never).unwrap();
            assert_eq!(hold.acquired().found(), generation);
            generation = hold.acquired().advanced_to();
            assert!(matches!(
                second.acquire(crate::coord::Waiting::Never),
                Err(CoordError::WouldWait)
            ));
            hold.begin_modification(Purpose::Ordinary).unwrap();
            hold.begin_modification(Purpose::Ordinary).unwrap();
            assert_eq!(second.sample().unwrap().get(), before.get() + 1);
            hold.end_modification(ModificationEnd::Settled).unwrap();
            assert!(!second.sample().unwrap().is_settled());
            hold.end_modification(ModificationEnd::Settled).unwrap();
            assert_eq!(second.sample().unwrap().get(), before.get() + 2);
            hold.release().unwrap();
        }
    });
}

#[test]
fn no_participant_takes_exclusion_or_attaches_on_a_root_reserved_for_another_implementation() {
    on_every_contract("coord-contract-identity", &|owner, backend| {
        opened(owner, backend)
            .reserve(b"inputs", CreationNonce::from_bytes([1; 16]))
            .unwrap();
        let others: Vec<Box<dyn CoordinationFactory>> = vec![
            Box::new(FileFactory::new()),
            Box::new(FileFactory::alternate()),
            Box::new(crate::coord::shared::SharedFactory::new()),
        ];
        for other in others
            .iter()
            .filter(|other| other.implementation() != owner.implementation())
        {
            let participant = opened(other.as_ref(), backend);
            assert!(
                matches!(
                    participant.acquire(crate::coord::Waiting::Never),
                    Err(CoordError::ImplementationMismatch { .. })
                ),
                "{} acquired on {}'s root",
                other.implementation(),
                owner.implementation()
            );
            assert!(
                matches!(
                    participant.attach(Access::Read, crate::coord::Waiting::Never),
                    Err(CoordError::ImplementationMismatch { .. })
                ),
                "{} attached on {}'s root",
                other.implementation(),
                owner.implementation()
            );
        }
        let own = opened(owner, backend);
        drop(own.acquire(crate::coord::Waiting::Never).unwrap());
        drop(
            own.attach(Access::Read, crate::coord::Waiting::Never)
                .unwrap(),
        );
    });
}

#[test]
fn a_participant_that_used_a_root_before_another_implementation_reserved_it_is_refused_after() {
    on_every_contract("coord-contract-identity-later", &|owner, backend| {
        let others: Vec<Box<dyn CoordinationFactory>> = vec![
            Box::new(FileFactory::new()),
            Box::new(FileFactory::alternate()),
            Box::new(crate::coord::shared::SharedFactory::new()),
        ];
        let early: Vec<Box<dyn Coordination>> = others
            .iter()
            .filter(|other| other.implementation() != owner.implementation())
            .map(|other| opened(other.as_ref(), backend))
            .collect();
        for participant in &early {
            participant.sample().unwrap();
        }
        opened(owner, backend)
            .reserve(b"inputs", CreationNonce::from_bytes([1; 16]))
            .unwrap();
        for participant in &early {
            assert!(
                matches!(
                    participant.acquire(crate::coord::Waiting::Never),
                    Err(CoordError::ImplementationMismatch { .. })
                ),
                "{} acquired on {}'s root",
                participant.implementation(),
                owner.implementation()
            );
            assert!(
                matches!(
                    participant.attach(Access::Read, crate::coord::Waiting::Never),
                    Err(CoordError::ImplementationMismatch { .. })
                ),
                "{} attached on {}'s root",
                participant.implementation(),
                owner.implementation()
            );
        }
    });
}

#[test]
fn a_creator_abandoned_while_it_publishes_its_marker_publishes_none_after() {
    struct Maker {
        fs: Arc<dyn Vfs>,
        root: PathBuf,
    }
    impl ArtifactMaker for Maker {
        fn make(&mut self, name: &OsStr, inputs: &[u8]) -> crate::coord::CoordResult<()> {
            let path = self.root.join(name);
            self.fs.write(&path, inputs)?;
            self.fs.fsync(&path)?;
            Ok(())
        }

        fn marker(&mut self, _: &OsStr, inputs: &[u8]) -> crate::coord::CoordResult<Vec<u8>> {
            Ok(inputs.to_vec())
        }
    }
    let factories: Vec<Box<dyn CoordinationFactory>> = vec![
        Box::new(FileFactory::new()),
        Box::new(crate::coord::shared::SharedFactory::new()),
    ];
    for factory in factories {
        let mem = MemFs::new();
        let root = PathBuf::from(Backend::MEM_ROOT);
        mem.create_dir_all(&root).unwrap();
        let marker = root.join("marker");
        let participant: Arc<std::sync::OnceLock<Arc<dyn Coordination>>> = Arc::default();
        let returned_during_pause = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (reached, returned, pausing) = (
            Arc::clone(&participant),
            Arc::clone(&returned_during_pause),
            Arc::clone(&paused),
        );
        let policy = FaultPolicy::new().inject(
            crate::FaultOp::PublishNew,
            marker.clone(),
            crate::Fault::observe_with(move |_, _| {
                pausing.store(true, std::sync::atomic::Ordering::Release);
                let target = Arc::clone(reached.get().expect("installed"));
                let (done, finished) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    target.abandon();
                    let _ = done.send(());
                });
                if recv_timeout(&finished, Duration::from_millis(200)).is_ok() {
                    returned.store(true, std::sync::atomic::Ordering::Release);
                }
            }),
            crate::When::Once,
        );
        let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(mem.another_process(), policy));
        let opened: Arc<dyn Coordination> = Arc::from(factory.open(vfs, &root).unwrap());
        let _ = participant.set(Arc::clone(&opened));
        let names = [OsStr::new("data"), OsStr::new("marker")];
        let plan = CreationPlan {
            inputs: b"inputs",
            nonce: CreationNonce::from_bytes([1; 16]),
            artifacts: &names,
        };
        let _ = opened.create(
            &plan,
            &mut Maker {
                fs: Arc::new(mem.another_process()),
                root: root.clone(),
            },
        );
        assert!(
            paused.load(std::sync::atomic::Ordering::Acquire),
            "{}: the marker's publication was reached",
            factory.implementation()
        );
        if returned_during_pause.load(std::sync::atomic::Ordering::Acquire) {
            assert!(
                !mem.exists(&marker).unwrap(),
                "{}: a marker was published after abandon returned",
                factory.implementation()
            );
        }
    }
}

#[test]
fn a_holder_that_dies_mid_modification_is_reported_dead_and_not_fenced() {
    on_every_contract("coord-contract-dead-unfenced", &|factory, backend| {
        let observer = opened(factory, backend);
        let mut first = observer.acquire(crate::coord::Waiting::Never).unwrap();
        first.complete_recovery().unwrap();
        first.release().unwrap();
        let dying = opened(factory, backend);
        let mut hold = dying.acquire(crate::coord::Waiting::Never).unwrap();
        hold.begin_modification(Purpose::Ordinary).unwrap();
        dying.abandon();
        drop(hold);
        let need = observer.discover_recovery().unwrap();
        assert!(need.holder_died(), "{}", factory.implementation());
        assert!(
            !need.fenced(),
            "{}: only an acquirer sets the fence",
            factory.implementation()
        );
        assert!(need.required());
    });
}

#[test]
fn an_abandonment_waits_for_a_discovery_inspection_in_progress() {
    on_every_contract("coord-contract-inspect-abandon", &|factory, backend| {
        let participant: Arc<dyn Coordination> = Arc::from(opened(factory, backend));
        participant
            .acquire(crate::coord::Waiting::Never)
            .unwrap()
            .release()
            .unwrap();
        let target = Arc::clone(&participant);
        let mut returned_during_inspection = None;
        let discovered = participant.discover_recovery_with(&mut || {
            let target = Arc::clone(&target);
            let (done, finished) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                target.abandon();
                let _ = done.send(());
            });
            returned_during_inspection =
                Some(recv_timeout(&finished, Duration::from_millis(200)).is_ok());
        });
        assert_eq!(
            returned_during_inspection,
            Some(false),
            "{}: abandon returned while the inspection ran",
            factory.implementation()
        );
        assert!(matches!(discovered, Ok(_) | Err(CoordError::Abandoned)));
    });
}

#[test]
fn an_abandoned_conversion_no_longer_reports_holding_the_gate() {
    on_every_contract(
        "coord-contract-conversion-abandoned",
        &|factory, backend| {
            let participant = opened(factory, backend);
            let attachment = participant
                .attach(Access::Read, crate::coord::Waiting::Never)
                .unwrap();
            let mut conversion = attachment.begin_conversion().unwrap();
            assert!(conversion.poll_gate().unwrap());
            participant.abandon();
            assert!(matches!(conversion.poll_gate(), Err(CoordError::Abandoned)));
            assert!(matches!(
                conversion.poll_tiers(ReaderSafety::Safe),
                Err(CoordError::Abandoned | CoordError::Malformed { .. })
            ));
        },
    );
}

#[test]
fn ending_a_modification_when_none_is_open_changes_nothing() {
    on_every_contract("coord-contract-end-none", &|factory, backend| {
        let first = opened(factory, backend);
        let second = opened(factory, backend);
        let mut hold = first.acquire(crate::coord::Waiting::Never).unwrap();
        hold.complete_recovery().unwrap();
        for end in [ModificationEnd::Settled, ModificationEnd::Unresolved] {
            assert!(matches!(
                hold.end_modification(end),
                Err(CoordError::NoModificationOpen)
            ));
        }
        hold.begin_modification(Purpose::Ordinary)
            .expect("refusing the end left the hold unfenced");
        hold.end_modification(ModificationEnd::Settled).unwrap();
        hold.release().unwrap();
        assert!(!second.discover_recovery().unwrap().fenced());
        assert!(second.sample().unwrap().is_settled());
    });
}

#[test]
fn public_abandonment_and_unresolved_work_preserve_recovery_on_every_backend() {
    on_every_contract("coord-contract-abandon", &|factory, backend| {
        let first = opened(factory, backend);
        let second = opened(factory, backend);
        let mut hold = first.acquire(crate::coord::Waiting::Wait).unwrap();
        second.sample().unwrap();
        hold.complete_recovery().unwrap();
        hold.begin_modification(Purpose::Ordinary).unwrap();
        first.abandon();
        assert!(second.discover_recovery().unwrap().holder_died());
        let mut next = second.acquire(crate::coord::Waiting::Wait).unwrap();
        assert!(next.acquired().holder_died());
        assert!(next.acquired().fenced());
        drop(hold);
        assert!(matches!(
            next.begin_modification(Purpose::Ordinary),
            Err(CoordError::RecoveryRequired)
        ));
        next.complete_recovery().unwrap();
        next.begin_modification(Purpose::Ordinary).unwrap();
        next.begin_modification(Purpose::Ordinary).unwrap();
        next.end_modification(ModificationEnd::Unresolved).unwrap();
        assert!(second.discover_recovery().unwrap().fenced());
        next.end_modification(ModificationEnd::Settled).unwrap();
        assert!(!second.sample().unwrap().is_settled());
        next.complete_recovery().unwrap();
        assert!(second.sample().unwrap().is_settled());
    });
}

#[test]
fn public_admission_conversion_and_membership_contract_on_every_backend() {
    on_every_contract("coord-contract-tiers", &|factory, backend| {
        let first = opened(factory, backend);
        let second = opened(factory, backend);
        let mut read = second
            .attach(Access::Read, crate::coord::Waiting::Never)
            .unwrap();
        let token = read.register(b"read member").unwrap();
        assert_eq!(read.register(b"updated member").unwrap(), token);
        let listing = first.list().unwrap();
        assert_eq!(listing.live.len(), 1);
        assert_eq!(listing.live[0].payload, b"updated member");
        let writer = first
            .attach(Access::Write, crate::coord::Waiting::Never)
            .unwrap();
        let safe = writer
            .upgrade(ReaderSafety::Safe, crate::coord::Waiting::Never)
            .unwrap();
        assert!(
            second
                .attach(Access::Read, crate::coord::Waiting::Never)
                .is_ok()
        );
        assert!(matches!(
            second.attach(Access::Write, crate::coord::Waiting::Never),
            Err(CoordError::WouldWait)
        ));
        drop(safe);
        drop(read);
        assert!(first.list().unwrap().live.is_empty());
        let blocker = second
            .attach(Access::Read, crate::coord::Waiting::Never)
            .unwrap();
        let mut conversion = first
            .attach(Access::Write, crate::coord::Waiting::Never)
            .unwrap()
            .begin_conversion()
            .unwrap();
        assert!(conversion.poll_gate().unwrap());
        assert!(
            conversion
                .poll_tiers(ReaderSafety::Unsafe)
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            second.attach(Access::Read, crate::coord::Waiting::Never),
            Err(CoordError::WouldWait)
        ));
        drop(blocker);
        let mut upgrade = conversion
            .poll_tiers(ReaderSafety::Unsafe)
            .unwrap()
            .unwrap();
        assert!(matches!(
            upgrade.register(b"upgrader").unwrap(),
            Registration::Registered(_)
        ));
        assert_eq!(second.list().unwrap().live[0].payload, b"upgrader");
        drop(upgrade);
        assert!(second.list().unwrap().live.is_empty());
        assert!(
            second
                .attach(Access::Write, crate::coord::Waiting::Never)
                .is_ok()
        );
    });
}

#[test]
fn public_notification_contract_on_every_implementation_and_backend() {
    on_every_contract("coord-contract-notice", &|factory, backend| {
        let first = opened(factory, backend);
        let second = opened(factory, backend);
        let mut hold = first.acquire(crate::coord::Waiting::Wait).unwrap();
        hold.complete_recovery().unwrap();
        let before = second.sample().unwrap();
        let (sender, receiver) = mpsc::channel();
        let notice = second
            .notify(
                before,
                Instant::now() + LONG,
                Box::new(move || {
                    let _ = sender.send(());
                }),
            )
            .unwrap();
        hold.begin_modification(Purpose::Ordinary).unwrap();
        recv_timeout(&receiver, LONG).unwrap();
        assert!(!second.sample().unwrap().is_settled());
        assert!(receiver.try_recv().is_err());
        hold.end_modification(ModificationEnd::Settled).unwrap();
        drop(notice);
        let (sender, receiver) = mpsc::channel();
        let deadline = second
            .notify(
                second.sample().unwrap(),
                Instant::now(),
                Box::new(move || {
                    let _ = sender.send(());
                }),
            )
            .unwrap();
        recv_timeout(&receiver, LONG).unwrap();
        drop(deadline);
    });
}

#[test]
fn failed_payload_updates_keep_the_previous_registration_across_implementations() {
    let factories: Vec<Box<dyn CoordinationFactory>> = vec![
        Box::new(FileFactory::new()),
        Box::new(FileFactory::alternate()),
        Box::new(crate::coord::shared::SharedFactory::new()),
    ];
    for factory in factories {
        let mem = MemFs::new();
        mem.create_dir_all(Path::new("/failed-update")).unwrap();
        let gate = crate::FaultGate::closed();
        let policy = FaultPolicy::new()
            .inject_gated(
                crate::FaultOp::Write,
                crate::PathMatcher::predicate(|_| true),
                crate::Fault::short_write(1),
                crate::When::Always,
                gate.clone(),
            )
            .inject_gated(
                crate::FaultOp::PublishNew,
                crate::PathMatcher::predicate(|_| true),
                crate::Fault::io_error(),
                crate::When::Always,
                gate.clone(),
            );
        let fs = Arc::new(FaultyFs::new(mem, policy));
        let participant = factory.open(fs, Path::new("/failed-update")).unwrap();
        let mut attachment = participant
            .attach(Access::Read, crate::coord::Waiting::Never)
            .unwrap();
        let token = attachment.register(b"original payload").unwrap();
        gate.open();
        assert!(attachment.register(b"replacement payload").is_err());
        gate.close();
        let listing = participant.list().unwrap();
        assert_eq!(
            listing.live.len(),
            1,
            "{} lost a live registration after an update failure",
            factory.implementation()
        );
        assert_eq!(listing.live[0].payload, b"original payload");
        assert_eq!(Registration::Registered(listing.live[0].token), token);
    }
}

#[test]
fn public_creation_reservation_contract_on_every_implementation_and_backend() {
    on_every_contract("coord-contract-create", &|factory, backend| {
        let participant = opened(factory, backend);
        let fs = backend.process();
        struct Maker {
            fs: Arc<dyn Vfs>,
            root: PathBuf,
        }
        impl ArtifactMaker for Maker {
            fn make(&mut self, name: &OsStr, inputs: &[u8]) -> crate::coord::CoordResult<()> {
                let path = self.root.join(name);
                self.fs.write(&path, inputs)?;
                self.fs.fsync(&path)?;
                Ok(())
            }

            fn marker(&mut self, _: &OsStr, inputs: &[u8]) -> crate::coord::CoordResult<Vec<u8>> {
                Ok(inputs.to_vec())
            }
        }
        let names = [OsStr::new("data"), OsStr::new("marker")];
        let plan = CreationPlan {
            inputs: b"inputs",
            nonce: CreationNonce::from_bytes([1; 16]),
            artifacts: &names,
        };
        assert_eq!(
            participant
                .create(
                    &plan,
                    &mut Maker {
                        fs,
                        root: backend.root()
                    }
                )
                .unwrap(),
            Created::Created
        );
        participant.confirm_created(OsStr::new("marker")).unwrap();
        assert!(matches!(
            participant.reserve(b"different", CreationNonce::from_bytes([2; 16])),
            Err(CoordError::Reserved)
        ));
    });
}
