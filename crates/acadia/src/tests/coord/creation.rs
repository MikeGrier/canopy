// Copyright (c) Michael Grier.

//! Creation exclusion and the creation reservation (canopy's DESIGN-NOTES.md, "Creating a store
//! root, modelled in `Creation.tla`").

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;

use test_timers::mpsc::recv_timeout;
use test_timers::thread::sleep;

use super::super::TempDir;
use super::{Backend, LONG, implementations, on_every_backend, on_every_implementation};
use crate::coord::file::FileFactory;
use crate::coord::file::reservation::Reservation;
use crate::coord::{
    ArtifactMaker, CoordError, CoordResult, Coordination, CoordinationFactory, Created,
    CreationNonce, CreationPlan,
};
use crate::{
    Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, MemFs, OsFs, PathMatcher, Vfs, VfsError,
    VfsErrorKind, When,
};

/// What every creation here makes, in order; the last is the marker.
const ARTIFACTS: [&str; 3] = ["files", "log", "marker"];
const MARKER: &str = "marker";
const INPUTS: &[u8] = b"genesis: three replicas";
const OTHER_INPUTS: &[u8] = b"genesis: five replicas";
/// Long enough for a participant that is not waiting to have finished.
const SETTLE: Duration = Duration::from_millis(50);

fn artifacts() -> Vec<&'static OsStr> {
    ARTIFACTS.iter().map(OsStr::new).collect()
}

fn nonce(n: u8) -> CreationNonce {
    CreationNonce::from_bytes([n; 16])
}

/// Makes each artifact as a file holding the inputs, made durable, and can fail at an artifact or
/// run an action before one.
struct FileMaker {
    vfs: Arc<dyn Vfs>,
    root: PathBuf,
    made: Vec<OsString>,
    fail_at: Option<usize>,
    before: Option<(usize, Box<dyn FnMut() + Send>)>,
}

impl FileMaker {
    fn new(vfs: Arc<dyn Vfs>, root: &Path) -> Self {
        FileMaker {
            vfs,
            root: root.to_path_buf(),
            made: Vec::new(),
            fail_at: None,
            before: None,
        }
    }

    fn failing_at(mut self, artifact: usize) -> Self {
        self.fail_at = Some(artifact);
        self
    }

    fn before(mut self, artifact: usize, action: impl FnMut() + Send + 'static) -> Self {
        self.before = Some((artifact, Box::new(action)));
        self
    }
}

impl ArtifactMaker for FileMaker {
    fn make(&mut self, name: &OsStr, inputs: &[u8]) -> CoordResult<()> {
        let index = self.made.len();
        if let Some((at, action)) = &mut self.before
            && *at == index
        {
            action();
        }
        if self.fail_at == Some(index) {
            return Err(VfsError::new(VfsErrorKind::Other, "make", "injected").into());
        }
        let path = self.root.join(name);
        self.vfs.write(&path, inputs)?;
        self.vfs.fsync(&path)?;
        self.made.push(name.to_os_string());
        Ok(())
    }
}

fn create(
    participant: &dyn Coordination,
    inputs: &[u8],
    nonce: CreationNonce,
    maker: &mut FileMaker,
) -> CoordResult<Created> {
    let artifacts = artifacts();
    participant.create(
        &CreationPlan {
            inputs,
            nonce,
            artifacts: &artifacts,
        },
        maker,
    )
}

/// Creates the root through a fresh participant of `implementation` over `vfs`.
fn create_through(
    implementation: &FileFactory,
    vfs: Arc<dyn Vfs>,
    root: &Path,
    inputs: &[u8],
    n: u8,
) -> CoordResult<Created> {
    let participant = implementation.open(Arc::clone(&vfs), root).expect("open");
    create(
        &*participant,
        inputs,
        nonce(n),
        &mut FileMaker::new(vfs, root),
    )
}

fn reservation(vfs: &dyn Vfs, root: &Path) -> Option<Reservation> {
    let path = FileFactory::new()
        .participant(Arc::new(MemFs::new()), root)
        .layout()
        .reservation();
    vfs.read(&path)
        .ok()
        .map(|bytes| Reservation::decode(&bytes).expect("a reservation"))
}

/// The names beneath `root` that are not the coordination's own.
fn store_names(vfs: &dyn Vfs, root: &Path) -> Vec<String> {
    let mut names: Vec<String> = vfs
        .read_dir(root)
        .expect("list the root")
        .into_iter()
        .map(|entry| entry.file_name.to_string_lossy().into_owned())
        .filter(|name| !name.starts_with(crate::coord::RESERVED_PREFIX))
        .collect();
    names.sort();
    names
}

/// Asserts that every artifact exists and holds `inputs`.
fn assert_created(vfs: &dyn Vfs, root: &Path, inputs: &[u8]) {
    for artifact in ARTIFACTS {
        assert_eq!(
            vfs.read(&root.join(artifact)).expect("an artifact"),
            inputs,
            "{artifact}"
        );
    }
}

#[test]
fn creation_records_the_reservation_then_makes_every_artifact_in_order() {
    on_every_implementation("coord-create", &|implementation, backend| {
        let vfs = backend.process();
        let root = backend.root();
        let participant = implementation.open(Arc::clone(&vfs), &root).expect("open");
        let mut maker = FileMaker::new(Arc::clone(&vfs), &root);
        assert_eq!(
            create(&*participant, INPUTS, nonce(1), &mut maker).expect("create"),
            Created::Created
        );
        assert_eq!(maker.made, ARTIFACTS.map(OsString::from));
        assert_created(&*vfs, &root, INPUTS);
        let recorded = reservation(&*vfs, &root).expect("a reservation");
        assert_eq!(&recorded.implementation, implementation.implementation());
        assert_eq!(recorded.nonce, nonce(1));
        assert_eq!(recorded.inputs, INPUTS);
        participant
            .confirm_created(OsStr::new(MARKER))
            .unwrap_or_else(|error| panic!("{}: {error}", backend.name()));
    });
}

#[test]
fn a_created_root_is_not_created_again_by_any_implementation() {
    on_every_implementation("coord-create-again", &|implementation, backend| {
        let root = backend.root();
        create_through(implementation, backend.process(), &root, INPUTS, 1).expect("create");
        for again in implementations() {
            assert!(matches!(
                create_through(&again, backend.process(), &root, INPUTS, 2),
                Err(CoordError::AlreadyCreated)
            ));
        }
    });
}

#[test]
fn an_open_through_another_implementation_is_refused() {
    on_every_backend("coord-open-mismatch", &|backend| {
        let root = backend.root();
        create_through(&FileFactory::new(), backend.process(), &root, INPUTS, 1).expect("create");
        let other = FileFactory::alternate()
            .open(backend.process(), &root)
            .expect("open");
        match other.confirm_created(OsStr::new(MARKER)) {
            Err(CoordError::ImplementationMismatch { recorded }) => {
                assert_eq!(recorded, FileFactory::IMPLEMENTATION);
            }
            other => panic!("{}: {other:?}", backend.name()),
        }
    });
}

#[test]
fn an_open_of_a_root_never_created_or_never_finished_is_refused() {
    on_every_implementation("coord-open-uncreated", &|implementation, backend| {
        let root = backend.root();
        let participant = implementation.open(backend.process(), &root).expect("open");
        assert!(matches!(
            participant.confirm_created(OsStr::new(MARKER)),
            Err(CoordError::NotCreated)
        ));
        let vfs = backend.process();
        let mut maker = FileMaker::new(Arc::clone(&vfs), &root).failing_at(2);
        let creator = implementation.open(vfs, &root).expect("open");
        assert!(create(&*creator, INPUTS, nonce(1), &mut maker).is_err());
        assert!(matches!(
            participant.confirm_created(OsStr::new(MARKER)),
            Err(CoordError::NotCreated)
        ));
    });
}

#[test]
fn a_plan_that_creation_could_not_make_is_refused_and_changes_nothing() {
    on_every_implementation("coord-plan", &|implementation, backend| {
        let vfs = backend.process();
        let root = backend.root();
        let participant = implementation.open(Arc::clone(&vfs), &root).expect("open");
        let bad: [&[&str]; 6] = [
            &[],
            &["dir/marker"],
            &[".."],
            &[".coord-marker"],
            &["log", "log"],
            &["files", ""],
        ];
        for artifacts in bad {
            let artifacts: Vec<&OsStr> = artifacts.iter().map(OsStr::new).collect();
            let plan = CreationPlan {
                inputs: INPUTS,
                nonce: nonce(1),
                artifacts: &artifacts,
            };
            let mut maker = FileMaker::new(Arc::clone(&vfs), &root);
            assert!(
                matches!(
                    participant.create(&plan, &mut maker),
                    Err(CoordError::InvalidPlan { .. })
                ),
                "{artifacts:?}"
            );
        }
        assert!(matches!(
            participant.confirm_created(OsStr::new("a/b")),
            Err(CoordError::InvalidPlan { .. })
        ));
        assert!(vfs.read_dir(&root).expect("list").is_empty());
    });
}

#[test]
fn a_root_holding_something_creation_does_not_make_is_refused_to_a_creator() {
    on_every_implementation("coord-occupied", &|implementation, backend| {
        let vfs = backend.process();
        let root = backend.root();
        for stray in ["stray", "files"] {
            vfs.write(&root.join(stray), b"not ours").expect("a stray");
            match create_through(implementation, backend.process(), &root, INPUTS, 1) {
                Err(CoordError::Occupied { name }) => assert_eq!(name, stray),
                other => panic!("{}: {stray}: {other:?}", backend.name()),
            }
            assert!(reservation(&*vfs, &root).is_none(), "nothing is reserved");
            vfs.remove_file(&root.join(stray))
                .expect("remove the stray");
        }
    });
}

#[test]
fn a_root_holding_something_creation_does_not_make_is_refused_to_a_resumer() {
    on_every_implementation("coord-occupied-resume", &|implementation, backend| {
        let vfs = backend.process();
        let root = backend.root();
        let creator = implementation.open(Arc::clone(&vfs), &root).expect("open");
        let mut interrupted = FileMaker::new(Arc::clone(&vfs), &root).failing_at(1);
        assert!(create(&*creator, INPUTS, nonce(1), &mut interrupted).is_err());
        vfs.create_dir(&root.join("stray"))
            .expect("a stray directory");
        assert!(matches!(
            create_through(implementation, backend.process(), &root, INPUTS, 2),
            Err(CoordError::Occupied { .. })
        ));
        assert_eq!(store_names(&*vfs, &root), ["files", "stray"]);
    });
}

#[test]
fn something_that_appears_while_a_creator_waits_refuses_it() {
    for implementation in implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        let lock = implementation
            .participant(Arc::new(MemFs::new()), &root)
            .layout()
            .creation_lock();
        let intruder = mem.another_process();
        let stray = root.join("stray");
        let policy = FaultPolicy::new().inject(
            FaultOp::Lock,
            &lock,
            Fault::observe_with(move |_, _| intruder.write(&stray, b"x").expect("intrude")),
            When::Once,
        );
        let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(mem.another_process(), policy));
        assert!(matches!(
            create_through(&implementation, Arc::clone(&vfs), &root, INPUTS, 1),
            Err(CoordError::Occupied { .. })
        ));
        assert!(reservation(&*vfs, &root).is_none());
    }
}

#[test]
fn an_interrupted_creation_is_resumed_with_identical_inputs() {
    on_every_implementation("coord-resume", &|implementation, backend| {
        let root = backend.root();
        for interrupt_at in 0..ARTIFACTS.len() {
            let vfs = backend.process();
            let creator = implementation.open(Arc::clone(&vfs), &root).expect("open");
            let mut interrupted = FileMaker::new(Arc::clone(&vfs), &root).failing_at(interrupt_at);
            assert!(create(&*creator, INPUTS, nonce(1), &mut interrupted).is_err());
            creator.abandon();
            assert_eq!(
                create_through(implementation, backend.process(), &root, INPUTS, 2)
                    .expect("resume"),
                Created::Resumed
            );
            assert_created(&*vfs, &root, INPUTS);
            assert_eq!(
                reservation(&*vfs, &root).expect("reserved").nonce,
                nonce(1),
                "a resumer adopts the creation's nonce"
            );
            for artifact in ARTIFACTS.iter().chain([&".coord-reservation"]) {
                let path = root.join(artifact);
                let _ = vfs.remove_file(&path);
            }
        }
    });
}

#[test]
fn an_interrupted_creation_is_refused_to_other_inputs_and_to_another_implementation() {
    on_every_backend("coord-resume-refused", &|backend| {
        let vfs = backend.process();
        let root = backend.root();
        let creator = FileFactory::new()
            .open(Arc::clone(&vfs), &root)
            .expect("open");
        let mut interrupted = FileMaker::new(Arc::clone(&vfs), &root).failing_at(1);
        assert!(create(&*creator, INPUTS, nonce(1), &mut interrupted).is_err());
        let before = store_names(&*vfs, &root);
        assert!(matches!(
            create_through(
                &FileFactory::new(),
                backend.process(),
                &root,
                OTHER_INPUTS,
                2
            ),
            Err(CoordError::Reserved)
        ));
        assert!(matches!(
            create_through(
                &FileFactory::alternate(),
                backend.process(),
                &root,
                INPUTS,
                3
            ),
            Err(CoordError::Reserved)
        ));
        assert_eq!(
            store_names(&*vfs, &root),
            before,
            "a refusal changes nothing"
        );
        assert_eq!(reservation(&*vfs, &root).expect("reserved").nonce, nonce(1));
    });
}

#[test]
fn of_two_implementations_creating_at_once_exactly_one_creates() {
    for round in 0..8 {
        on_every_backend("coord-race", &|backend| {
            let root = backend.root();
            let start = Arc::new(Barrier::new(2));
            let racers: Vec<_> = implementations()
                .into_iter()
                .enumerate()
                .map(|(i, implementation)| {
                    let vfs = backend.process();
                    let root = root.clone();
                    let start = Arc::clone(&start);
                    thread::spawn(move || {
                        start.wait();
                        let outcome = create_through(&implementation, vfs, &root, INPUTS, i as u8);
                        (implementation.implementation().clone(), outcome)
                    })
                })
                .collect();
            let outcomes: Vec<_> = racers
                .into_iter()
                .map(|racer| racer.join().expect("a racer"))
                .collect();
            let winners: Vec<_> = outcomes
                .iter()
                .filter(|(_, outcome)| matches!(outcome, Ok(Created::Created)))
                .collect();
            assert_eq!(
                winners.len(),
                1,
                "{} round {round}: {outcomes:?}",
                backend.name()
            );
            for (_, outcome) in &outcomes {
                assert!(matches!(
                    outcome,
                    Ok(Created::Created) | Err(CoordError::Reserved | CoordError::AlreadyCreated)
                ));
            }
            let vfs = backend.process();
            assert_eq!(
                reservation(&*vfs, &root).expect("reserved").implementation,
                winners[0].0
            );
            assert_created(&*vfs, &root, INPUTS);
        });
    }
}

#[test]
fn a_creator_whose_reservation_is_published_second_is_refused() {
    for (first, second) in [
        (FileFactory::alternate(), FileFactory::new()),
        (FileFactory::new(), FileFactory::alternate()),
    ] {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        let rival = mem.another_process();
        let reservation_path = second
            .participant(Arc::new(MemFs::new()), &root)
            .layout()
            .reservation();
        let rival_root = root.clone();
        let policy = FaultPolicy::new().inject(
            FaultOp::PublishNew,
            &reservation_path,
            Fault::observe_with(move |_, _| {
                create_through(
                    &first,
                    Arc::new(rival.another_process()),
                    &rival_root,
                    INPUTS,
                    1,
                )
                .expect("the rival creates");
            }),
            When::Once,
        );
        let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(mem.another_process(), policy));
        let participant = second.open(Arc::clone(&vfs), &root).expect("open");
        let mut maker = FileMaker::new(Arc::clone(&vfs), &root);
        assert!(matches!(
            create(&*participant, OTHER_INPUTS, nonce(2), &mut maker),
            Err(CoordError::Reserved)
        ));
        assert!(maker.made.is_empty());
        assert_ne!(
            &reservation(&*vfs, &root).expect("reserved").implementation,
            second.implementation()
        );
        assert_created(&*vfs, &root, INPUTS);
    }
}

/// Runs `body` on a thread and reports whether it has finished after [`SETTLE`].
fn still_waiting<T: Send + 'static>(
    body: impl FnOnce() -> T + Send + 'static,
) -> (bool, thread::JoinHandle<T>) {
    let handle = thread::spawn(body);
    sleep(SETTLE);
    (!handle.is_finished(), handle)
}

#[test]
fn a_creator_suspended_before_publishing_holds_off_its_own_implementation() {
    for implementation in implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        let reservation_path = implementation
            .participant(Arc::new(MemFs::new()), &root)
            .layout()
            .reservation();
        let waiters = Arc::new(Mutex::new(Vec::new()));
        let (seen, observed) = (Arc::clone(&waiters), mem.another_process());
        let (factory, waiter_root) = (implementation.clone(), root.clone());
        let policy = FaultPolicy::new().inject(
            FaultOp::PublishNew,
            &reservation_path,
            Fault::observe_with(move |_, _| {
                let (creator_vfs, opener_vfs) = (
                    Arc::new(observed.another_process()),
                    Arc::new(observed.another_process()),
                );
                let (f1, f2, r1, r2) = (
                    factory.clone(),
                    factory.clone(),
                    waiter_root.clone(),
                    waiter_root.clone(),
                );
                let creator = still_waiting(move || {
                    create_through(&f1, creator_vfs, &r1, INPUTS, 3).map(|_| ())
                });
                let opener = still_waiting(move || {
                    f2.open(opener_vfs, &r2)
                        .expect("open")
                        .confirm_created(OsStr::new(MARKER))
                });
                assert!(creator.0, "a creator of the same implementation waits");
                assert!(opener.0, "an opener of the same implementation waits");
                seen.lock().unwrap().extend([creator.1, opener.1]);
            }),
            When::Once,
        );
        let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(mem.another_process(), policy));
        assert_eq!(
            create_through(&implementation, vfs, &root, INPUTS, 1).expect("create"),
            Created::Created
        );
        let mut handles = std::mem::take(&mut *waiters.lock().unwrap()).into_iter();
        let creator = handles.next().expect("the waiting creator");
        let opener = handles.next().expect("the waiting opener");
        assert!(matches!(
            creator.join().expect("joined"),
            Err(CoordError::AlreadyCreated)
        ));
        opener
            .join()
            .expect("joined")
            .expect("the opener finds a store");
    }
}

#[test]
fn a_creator_suspended_after_publishing_refuses_every_other_creation() {
    for implementation in implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        let (rival, same) = (mem.another_process(), mem.another_process());
        let (root2, factory) = (root.clone(), implementation.clone());
        let waiting = Arc::new(Mutex::new(None));
        let parked = Arc::clone(&waiting);
        let vfs: Arc<dyn Vfs> = Arc::new(mem.another_process());
        let mut maker = FileMaker::new(Arc::clone(&vfs), &root).before(0, move || {
            let other = implementations()
                .into_iter()
                .find(|other| other.implementation() != factory.implementation())
                .expect("another implementation");
            assert!(matches!(
                create_through(&other, Arc::new(rival.another_process()), &root2, INPUTS, 7),
                Err(CoordError::Reserved)
            ));
            assert!(matches!(
                other
                    .open(Arc::new(rival.another_process()), &root2)
                    .expect("open")
                    .confirm_created(OsStr::new(MARKER)),
                Err(CoordError::ImplementationMismatch { .. })
            ));
            let (f, r, v) = (
                factory.clone(),
                root2.clone(),
                Arc::new(same.another_process()),
            );
            let waiter = still_waiting(move || create_through(&f, v, &r, INPUTS, 8));
            assert!(waiter.0, "a creator of the same implementation waits");
            *parked.lock().unwrap() = Some(waiter.1);
        });
        let participant = implementation.open(Arc::clone(&vfs), &root).expect("open");
        assert_eq!(
            create(&*participant, INPUTS, nonce(1), &mut maker).expect("create"),
            Created::Created
        );
        let waiter = waiting.lock().unwrap().take().expect("the waiter");
        assert!(matches!(
            waiter.join().expect("joined"),
            Err(CoordError::AlreadyCreated)
        ));
    }
}

#[test]
fn a_resumer_that_waited_while_its_creation_finished_creates_nothing() {
    for implementation in implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        let first = Arc::new(mem.another_process());
        let mut interrupted = FileMaker::new(first.clone(), &root).failing_at(1);
        let creator = implementation.open(first, &root).expect("open");
        assert!(create(&*creator, INPUTS, nonce(1), &mut interrupted).is_err());
        creator.abandon();

        let lock = implementation
            .participant(Arc::new(MemFs::new()), &root)
            .layout()
            .creation_lock();
        let (waits, waiting) = mpsc::channel();
        let waits = Mutex::new(waits);
        let policy = FaultPolicy::new().inject(
            FaultOp::Lock,
            &lock,
            Fault::observe_with(move |_, _| {
                let _ = waits.lock().unwrap().send(());
            }),
            When::Once,
        );
        let resumer_vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(mem.another_process(), policy));
        let (resumer_factory, resumer_root) = (implementation.clone(), root.clone());
        let parked = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&parked);
        let finisher_vfs: Arc<dyn Vfs> = Arc::new(mem.another_process());
        let mut finishing = FileMaker::new(Arc::clone(&finisher_vfs), &root).before(0, move || {
            let (factory, vfs, root) = (
                resumer_factory.clone(),
                Arc::clone(&resumer_vfs),
                resumer_root.clone(),
            );
            let resumer = thread::spawn(move || create_through(&factory, vfs, &root, INPUTS, 3));
            recv_timeout(&waiting, LONG).expect("the resumer reaches its wait");
            *slot.lock().unwrap() = Some(resumer);
        });
        let finisher = implementation.open(finisher_vfs, &root).expect("open");
        assert_eq!(
            create(&*finisher, INPUTS, nonce(2), &mut finishing).expect("resume"),
            Created::Resumed
        );
        let resumer = parked.lock().unwrap().take().expect("the resumer");
        assert!(matches!(
            resumer.join().expect("joined"),
            Err(CoordError::AlreadyCreated)
        ));
    }
}

/// A creator over `mem` whose sync of the root fails once its maker has made the marker, as a
/// creator that died between making the marker and making it durable leaves the root.
fn root_with_an_unsynced_marker(implementation: &FileFactory, mem: &MemFs, root: &Path) {
    let gate = FaultGate::closed();
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Fsync,
        PathMatcher::exact(root),
        Fault::error(VfsErrorKind::Other),
        When::Always,
        gate.clone(),
    );
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(mem.another_process(), policy));
    let mut maker = FileMaker::new(Arc::clone(&vfs), root).before(ARTIFACTS.len() - 1, move || {
        gate.open();
    });
    let creator = implementation.open(vfs, root).expect("open");
    assert!(create(&*creator, INPUTS, nonce(1), &mut maker).is_err());
    creator.abandon();
}

#[test]
fn an_open_makes_a_marker_its_creator_did_not_make_durable() {
    for implementation in implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        root_with_an_unsynced_marker(&implementation, &mem, &root);
        implementation
            .open(Arc::new(mem.another_process()), &root)
            .expect("open")
            .confirm_created(OsStr::new(MARKER))
            .expect("the open finds the marker");
        mem.simulate_crash();
        assert_created(&mem, &root, INPUTS);
        implementation
            .open(Arc::new(mem.another_process()), &root)
            .expect("open")
            .confirm_created(OsStr::new(MARKER))
            .expect("the store survives the crash");
    }
}

#[test]
fn a_marker_no_open_made_durable_is_lost_to_a_crash() {
    let mem = Backend::mem();
    let root = PathBuf::from(Backend::MEM_ROOT);
    root_with_an_unsynced_marker(&FileFactory::new(), &mem, &root);
    mem.simulate_crash();
    assert!(!mem.exists(&root.join(MARKER)).expect("exists"));
}

/// The ways a creation is cut short: the n-th sync of the root failing, or the maker failing at
/// an artifact.
fn interruptions() -> Vec<(Option<u32>, Option<usize>)> {
    let syncs = (1..=ARTIFACTS.len() as u32 + 1).map(|n| (Some(n), None));
    let makes = (0..ARTIFACTS.len()).map(|at| (None, Some(at)));
    syncs.chain(makes).collect()
}

#[test]
fn a_crash_between_any_two_steps_of_creation_leaves_a_creation_to_finish() {
    for implementation in implementations() {
        for (sync, make) in interruptions() {
            let mem = Backend::mem();
            let root = PathBuf::from(Backend::MEM_ROOT);
            let mut policy = FaultPolicy::new();
            if let Some(n) = sync {
                policy = policy.inject(FaultOp::Fsync, &root, Fault::io_error(), When::OnCall(n));
            }
            let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(mem.another_process(), policy));
            let mut maker = FileMaker::new(Arc::clone(&vfs), &root);
            maker.fail_at = make;
            let creator = implementation.open(vfs, &root).expect("open");
            assert!(create(&*creator, INPUTS, nonce(1), &mut maker).is_err());
            creator.abandon();
            mem.simulate_crash();

            let names = store_names(&mem, &root);
            if !names.is_empty() {
                assert!(
                    reservation(&mem, &root).is_some(),
                    "{sync:?} {make:?}: an artifact without its reservation: {names:?}"
                );
            }
            if names.iter().any(|name| name == MARKER) {
                assert_created(&mem, &root, INPUTS);
            }

            let outcome = create_through(
                &implementation,
                Arc::new(mem.another_process()),
                &root,
                INPUTS,
                2,
            )
            .unwrap_or_else(|error| panic!("{sync:?} {make:?}: {error}"));
            assert!(matches!(outcome, Created::Created | Created::Resumed));
            mem.simulate_crash();
            assert_created(&mem, &root, INPUTS);
            implementation
                .open(Arc::new(mem.another_process()), &root)
                .expect("open")
                .confirm_created(OsStr::new(MARKER))
                .expect("a finished creation survives a crash");
        }
    }
}

#[test]
fn creation_works_across_real_directories() {
    let dir = TempDir::new("coord-create-os");
    let vfs: Arc<dyn Vfs> = Arc::new(OsFs::new());
    assert_eq!(
        create_through(&FileFactory::new(), Arc::clone(&vfs), dir.path(), INPUTS, 1)
            .expect("create"),
        Created::Created
    );
    assert_created(&*vfs, dir.path(), INPUTS);
}

#[test]
fn a_reservation_reads_back_as_written_and_nothing_else_reads_as_one() {
    let written = Reservation {
        implementation: FileFactory::IMPLEMENTATION,
        nonce: nonce(9),
        inputs: INPUTS.to_vec(),
    };
    let bytes = written.encode();
    assert_eq!(Reservation::decode(&bytes).expect("decode"), written);
    for cut in 0..bytes.len() {
        assert!(Reservation::decode(&bytes[..cut]).is_err(), "cut at {cut}");
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(Reservation::decode(&longer).is_err());
    let mut other_magic = bytes.clone();
    other_magic[0] ^= 1;
    assert!(Reservation::decode(&other_magic).is_err());
    let empty = Reservation {
        inputs: Vec::new(),
        ..written
    };
    assert_eq!(Reservation::decode(&empty.encode()).expect("decode"), empty);
}
