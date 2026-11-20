// Copyright (c) Michael Grier.

//! Creation exclusion and the creation reservation (canopy's DESIGN-NOTES.md, "Creating a store
//! root, modelled in `Creation.tla`").

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

fn artifacts() -> Vec<&'static str> {
    ARTIFACTS.to_vec()
}

pub(super) fn nonce(n: u8) -> CreationNonce {
    CreationNonce::from_bytes([n; 16])
}

/// Makes each artifact as a file holding the inputs, made durable, and can fail at an artifact or
/// run an action before one.
struct FileMaker {
    vfs: Arc<dyn Vfs>,
    root: PathBuf,
    made: Vec<String>,
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

impl FileMaker {
    /// Runs the action or the failure scripted for the next artifact.
    fn step(&mut self) -> CoordResult<()> {
        let index = self.made.len();
        if let Some((at, action)) = &mut self.before
            && *at == index
        {
            action();
        }
        if self.fail_at == Some(index) {
            return Err(VfsError::new(VfsErrorKind::Other, "make", "injected").into());
        }
        Ok(())
    }
}

impl ArtifactMaker for FileMaker {
    fn make(&mut self, name: &str, inputs: &[u8]) -> CoordResult<()> {
        self.step()?;
        let path = self.root.join(name);
        self.vfs.write(&path, inputs)?;
        self.vfs.fsync(&path)?;
        self.made.push(name.to_owned());
        Ok(())
    }

    fn marker(&mut self, name: &str, inputs: &[u8]) -> CoordResult<Vec<u8>> {
        self.step()?;
        self.made.push(name.to_owned());
        Ok(inputs.to_vec())
    }
}

impl crate::coord::ManagedArtifactMaker for FileMaker {
    fn make(&mut self, logical: &str, target: &Path, _: &[u8]) -> CoordResult<()> {
        self.step()?;
        assert_eq!(
            target.parent().and_then(Path::parent),
            Some(self.root.as_path())
        );
        assert_eq!(target.file_name().unwrap(), "artifact");
        let physical = target.file_name().unwrap().to_str().unwrap();
        assert!(
            physical
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        );
        self.vfs.write(target, logical.as_bytes())?;
        self.vfs.fsync(target)?;
        self.made.push(logical.to_owned());
        Ok(())
    }

    fn marker(&mut self, logical: &str, inputs: &[u8]) -> CoordResult<Vec<u8>> {
        ArtifactMaker::marker(self, logical, inputs)
    }
}

#[test]
fn managed_creation_resumes_and_reopens_exact_names() {
    super::on_every_contract("managed-names", &|factory, backend| {
        let fs = backend.process();
        let root = backend.root();
        let participant = factory.open(fs.clone(), &root).unwrap();
        let long = "\u{1f642}".repeat(256);
        let names = [
            "",
            ".",
            "..",
            "a",
            "A",
            "\u{e9}",
            "\u{c9}",
            "e\u{301}",
            "f:g",
            "marker.",
            "marker ",
            "report:summary",
            "CON",
            "COM\u{b9}",
            "COM\u{b2}.txt",
            "LPT\u{b3}",
            "NUL .txt",
            "Con  .dat",
            "COM\u{b9} .txt",
            "lpt\u{b2} .log",
            "bad?name",
            "/",
            "\\",
            "\0\r\n,\"",
            ".coord-reservation",
            ".names.csv",
            &long,
        ];
        let plan = CreationPlan {
            inputs: INPUTS,
            nonce: nonce(1),
            artifacts: &names,
        };
        let mut interrupted = FileMaker::new(fs.clone(), &root).failing_at(names.len() - 1);
        assert!(participant.create_managed(&plan, &mut interrupted).is_err());
        assert_eq!(interrupted.made, names[..names.len() - 1]);
        assert!(matches!(
            participant.confirm_managed_created(&long),
            Err(CoordError::NotCreated)
        ));
        drop(participant);
        let fs = backend.process();
        let participant = factory.open(fs.clone(), &root).unwrap();
        let mut maker = FileMaker::new(fs.clone(), &root);
        assert_eq!(
            participant.create_managed(&plan, &mut maker).unwrap(),
            Created::Resumed
        );
        assert_eq!(maker.made, names[names.len() - 1..]);
        participant.confirm_managed_created(&long).unwrap();
        assert_eq!(
            participant.create_managed(&plan, &mut maker).unwrap(),
            Created::Existing
        );
        assert_eq!(maker.made, names[names.len() - 1..]);
        let dir = crate::names::ComponentDirectory::open(&*fs, &root).unwrap();
        assert_eq!(
            dir.list()
                .unwrap()
                .into_iter()
                .map(|entry| entry.file_name)
                .collect::<std::collections::BTreeSet<_>>(),
            names.iter().map(|name| name.to_string()).collect()
        );
        for logical in names {
            let path = dir.lookup(logical).unwrap().unwrap();
            assert_eq!(
                fs.read(&path).unwrap(),
                if logical == long {
                    INPUTS
                } else {
                    logical.as_bytes()
                }
            );
        }
    });
}

#[test]
fn managed_creation_serializes_competing_creators() {
    super::on_every_contract("managed-creators", &|factory, backend| {
        let gate = std::sync::Barrier::new(2);
        let run = || {
            let fs = backend.process();
            let root = backend.root();
            let participant = factory.open(fs.clone(), &root).unwrap();
            let mut maker = FileMaker::new(fs, &root);
            gate.wait();
            participant
                .create_managed(
                    &CreationPlan {
                        inputs: INPUTS,
                        nonce: nonce(2),
                        artifacts: &["A:B", "m/arker"],
                    },
                    &mut maker,
                )
                .unwrap()
        };
        std::thread::scope(|scope| {
            let peer = scope.spawn(run);
            let results = [run(), peer.join().unwrap()];
            assert!(results.contains(&Created::Created) && results.contains(&Created::Existing));
        });
    });
}

#[test]
fn managed_creation_recovers_csv_publication_siblings() {
    super::on_every_contract("managed-csv-stage-unowned", &|factory, backend| {
        let fs = backend.process();
        let root = backend.root();
        let participant = factory.open(fs.clone(), &root).unwrap();
        let stage = root.join(format!(".names.csv{}interrupted", crate::PUBLISHING_INFIX));
        fs.write(&stage, b"unowned").unwrap();
        let mut maker = FileMaker::new(fs.clone(), &root);
        assert!(matches!(
            participant.create_managed(
                &CreationPlan {
                    inputs: INPUTS,
                    nonce: nonce(8),
                    artifacts: &["data", "marker"],
                },
                &mut maker,
            ),
            Err(CoordError::Occupied { .. })
        ));
        assert_eq!(fs.read(&stage).unwrap(), b"unowned");
    });

    for (tag, with_map) in [
        ("managed-csv-stage-only", false),
        ("managed-csv-and-stage", true),
    ] {
        super::on_every_contract(tag, &|factory, backend| {
            let fs = backend.process();
            let root = backend.root();
            let participant = factory.open(fs.clone(), &root).unwrap();
            assert_eq!(
                participant.reserve(INPUTS, nonce(8)).unwrap(),
                Created::Created
            );
            if with_map {
                fs.write(
                    &root.join(".names.csv"),
                    b"acadia_names_v1,logical_utf8_hex\n",
                )
                .unwrap();
            }
            let stage = root.join(format!(".names.csv{}interrupted", crate::PUBLISHING_INFIX));
            fs.write(&stage, b"complete or partial publication bytes")
                .unwrap();
            let plan = CreationPlan {
                inputs: INPUTS,
                nonce: nonce(9),
                artifacts: &["data", "marker"],
            };
            let mut maker = FileMaker::new(fs.clone(), &root);
            assert_eq!(
                participant.create_managed(&plan, &mut maker).unwrap(),
                Created::Resumed
            );
            participant.confirm_managed_created("marker").unwrap();
        });
    }
}

#[test]
fn abandoning_managed_creation_releases_the_resolver_lock() {
    struct Paused {
        fs: Arc<dyn Vfs>,
        entered: std::sync::mpsc::Sender<()>,
        resume: std::sync::mpsc::Receiver<()>,
    }

    impl crate::coord::ManagedArtifactMaker for Paused {
        fn make(&mut self, _: &str, target: &Path, _: &[u8]) -> CoordResult<()> {
            self.entered.send(()).unwrap();
            self.resume.recv().unwrap();
            self.fs.write(target, b"stale after abandonment")?;
            self.fs.fsync(target)?;
            Ok(())
        }

        fn marker(&mut self, _: &str, inputs: &[u8]) -> CoordResult<Vec<u8>> {
            Ok(inputs.to_vec())
        }
    }

    super::on_every_contract("managed-abandon", &|factory, backend| {
        let fs = backend.process();
        let root = backend.root();
        let participant = factory.open(fs.clone(), &root).unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let mut maker = Paused {
            fs: fs.clone(),
            entered: entered_tx,
            resume: resume_rx,
        };
        let plan = CreationPlan {
            inputs: INPUTS,
            nonce: nonce(7),
            artifacts: &["data", "marker"],
        };
        std::thread::scope(|scope| {
            let creator = scope.spawn(|| participant.create_managed(&plan, &mut maker));
            test_timers::mpsc::recv_timeout(&entered_rx, std::time::Duration::from_secs(5))
                .unwrap();
            participant.abandon();
            let acquired = fs.try_lock(
                &root.join(".names.lock"),
                crate::LockMode::Exclusive,
                crate::LockSpan::File,
            );
            assert!(acquired.is_ok(), "{acquired:?}");
            drop(acquired.unwrap());
            let replacement_fs = backend.process();
            let replacement = factory.open(replacement_fs.clone(), &root).unwrap();
            let mut fresh = FileMaker::new(replacement_fs.clone(), &root);
            assert_eq!(
                replacement.create_managed(&plan, &mut fresh).unwrap(),
                Created::Resumed
            );
            replacement.confirm_managed_created("marker").unwrap();
            resume_tx.send(()).unwrap();
            assert!(matches!(
                creator.join().unwrap(),
                Err(CoordError::Abandoned)
            ));
            let directory =
                crate::names::ComponentDirectory::open(&*replacement_fs, &root).unwrap();
            assert_eq!(
                replacement_fs
                    .read(&directory.lookup("data").unwrap().unwrap())
                    .unwrap(),
                b"data"
            );
        });
    });
}

#[test]
fn rejected_managed_creation_does_not_poison_a_raw_reservation() {
    super::on_every_contract("managed-raw-reservation", &|factory, backend| {
        let fs = backend.process();
        let root = backend.root();
        let participant = factory.open(fs.clone(), &root).unwrap();
        assert_eq!(
            participant.reserve(INPUTS, nonce(1)).unwrap(),
            Created::Created
        );
        let before = fs.read_dir(&root).unwrap();
        let different = CreationPlan {
            inputs: b"different inputs",
            nonce: nonce(2),
            artifacts: &["data", "marker"],
        };
        let mut managed = FileMaker::new(fs.clone(), &root);
        assert!(matches!(
            participant.create_managed(&different, &mut managed),
            Err(CoordError::Reserved)
        ));
        assert_eq!(fs.read_dir(&root).unwrap(), before);
        let mut raw = FileMaker::new(fs, &root);
        assert_eq!(
            create(&*participant, INPUTS, nonce(1), &mut raw).unwrap(),
            Created::Resumed
        );
    });
}

#[test]
fn nested_publication_staging_resumes_inside_a_claimed_attempt() {
    struct Interrupt {
        fs: Arc<dyn Vfs>,
    }

    impl crate::coord::ManagedArtifactMaker for Interrupt {
        fn make(&mut self, _: &str, target: &Path, _: &[u8]) -> CoordResult<()> {
            let leftover = target
                .parent()
                .unwrap()
                .join(format!("artifact{}interrupted", crate::PUBLISHING_INFIX));
            self.fs.write(&leftover, b"partial publication")?;
            Err(VfsError::new(VfsErrorKind::Other, "make", "interrupted").into())
        }

        fn marker(&mut self, _: &str, _: &[u8]) -> CoordResult<Vec<u8>> {
            unreachable!("the interrupted artifact precedes the marker")
        }
    }

    struct Publisher {
        fs: Arc<dyn Vfs>,
    }

    impl crate::coord::ManagedArtifactMaker for Publisher {
        fn make(&mut self, logical: &str, target: &Path, _: &[u8]) -> CoordResult<()> {
            self.fs.publish_new(target, logical.as_bytes())?;
            self.fs.fsync(target.parent().unwrap())?;
            Ok(())
        }

        fn marker(&mut self, _: &str, inputs: &[u8]) -> CoordResult<Vec<u8>> {
            Ok(inputs.to_vec())
        }
    }

    super::on_every_contract("managed-nested-publish", &|factory, backend| {
        let fs = backend.process();
        let root = backend.root();
        let logical = "x".repeat(212);
        let plan = CreationPlan {
            inputs: INPUTS,
            nonce: nonce(13),
            artifacts: &[&logical, "marker"],
        };
        let participant = factory.open(fs.clone(), &root).unwrap();
        assert!(
            participant
                .create_managed(&plan, &mut Interrupt { fs: fs.clone() },)
                .is_err()
        );
        drop(participant);

        let participant = factory.open(backend.process(), &root).unwrap();
        let mut publisher = Publisher { fs: fs.clone() };
        assert_eq!(
            participant.create_managed(&plan, &mut publisher).unwrap(),
            Created::Resumed
        );
        participant.confirm_managed_created("marker").unwrap();
        let directory = crate::names::ComponentDirectory::open(&*fs, &root).unwrap();
        assert_eq!(
            fs.read(&directory.lookup(&logical).unwrap().unwrap())
                .unwrap(),
            logical.as_bytes()
        );
    });
}

#[cfg(unix)]
#[test]
fn managed_creation_refuses_a_symlink_staging_entry() {
    struct SymlinkMaker;

    impl crate::coord::ManagedArtifactMaker for SymlinkMaker {
        fn make(&mut self, _: &str, target: &Path, _: &[u8]) -> CoordResult<()> {
            let referent = target.parent().unwrap().join("referent");
            std::fs::write(&referent, b"not the entry kind we claim").unwrap();
            std::os::unix::fs::symlink(Path::new("referent"), target).unwrap();
            Ok(())
        }

        fn marker(&mut self, _: &str, inputs: &[u8]) -> CoordResult<Vec<u8>> {
            Ok(inputs.to_vec())
        }
    }

    let factories: Vec<Box<dyn CoordinationFactory>> = vec![
        Box::new(FileFactory::new()),
        Box::new(FileFactory::alternate()),
        Box::new(crate::coord::shared::SharedFactory::new()),
    ];
    for factory in factories {
        let temp = TempDir::new("managed-symlink-staging");
        let fs: Arc<dyn Vfs> = Arc::new(crate::OsFs::new());
        let participant = factory.open(fs.clone(), temp.path()).unwrap();
        let plan = CreationPlan {
            inputs: INPUTS,
            nonce: nonce(14),
            artifacts: &["data", "marker"],
        };
        assert!(matches!(
            participant.create_managed(&plan, &mut SymlinkMaker),
            Err(CoordError::Malformed { .. })
        ));
        assert!(!temp.path().join("marker").exists());
    }
}

#[test]
fn invalid_raw_csv_does_not_poison_managed_probe_or_raw_resume() {
    super::on_every_contract("managed-invalid-raw-csv", &|factory, backend| {
        let fs = backend.process();
        let root = backend.root();
        let participant = factory.open(fs.clone(), &root).unwrap();
        assert_eq!(
            participant.reserve(INPUTS, nonce(10)).unwrap(),
            Created::Created
        );
        let map = root.join(".names.csv");
        fs.write(&map, b"ordinary raw artifact payload").unwrap();
        let before = fs.read_dir(&root).unwrap();
        let plan = CreationPlan {
            inputs: INPUTS,
            nonce: nonce(11),
            artifacts: &[".names.csv", "marker"],
        };
        let mut maker = FileMaker::new(fs.clone(), &root);
        assert!(participant.create_managed(&plan, &mut maker).is_err());
        assert_eq!(fs.read_dir(&root).unwrap(), before);
        assert_eq!(
            participant.create(&plan, &mut maker).unwrap(),
            Created::Resumed
        );
        participant.confirm_created("marker").unwrap();
    });
}

#[test]
fn valid_raw_header_does_not_let_a_managed_probe_claim_unregistered_children() {
    super::on_every_contract("managed-valid-header-raw-root", &|factory, backend| {
        let fs = backend.process();
        let root = backend.root();
        let participant = factory.open(fs.clone(), &root).unwrap();
        assert_eq!(
            participant.reserve(INPUTS, nonce(15)).unwrap(),
            Created::Created
        );
        fs.write(
            &root.join(".names.csv"),
            b"acadia_names_v1,logical_utf8_hex\n",
        )
        .unwrap();
        fs.write(&root.join("data"), b"raw payload").unwrap();
        let before = fs.read_dir(&root).unwrap();
        let managed = CreationPlan {
            inputs: INPUTS,
            nonce: nonce(16),
            artifacts: &["data", "marker"],
        };
        let mut maker = FileMaker::new(fs.clone(), &root);
        assert!(participant.create_managed(&managed, &mut maker).is_err());
        assert_eq!(fs.read_dir(&root).unwrap(), before);

        let raw = CreationPlan {
            inputs: INPUTS,
            nonce: nonce(15),
            artifacts: &[".names.csv", "data", "marker"],
        };
        assert_eq!(
            participant.create(&raw, &mut maker).unwrap(),
            Created::Resumed
        );
        participant.confirm_created("marker").unwrap();
    });
}

#[test]
fn managed_creation_never_adopts_an_unbound_raw_marker() {
    super::on_every_contract("managed-no-adopt", &|factory, backend| {
        let fs = backend.process();
        let root = backend.root();
        let participant = factory.open(fs.clone(), &root).unwrap();
        let mut maker = FileMaker::new(fs.clone(), &root);
        create(&*participant, INPUTS, nonce(1), &mut maker).unwrap();
        let before = store_names(&*fs, &root);
        assert!(participant.confirm_managed_created(MARKER).is_err());
        let plan = CreationPlan {
            inputs: INPUTS,
            nonce: nonce(2),
            artifacts: &ARTIFACTS,
        };
        assert!(participant.create_managed(&plan, &mut maker).is_err());
        assert_eq!(store_names(&*fs, &root), before);
        participant.confirm_created(MARKER).unwrap();
    });
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

pub(super) fn reservation(vfs: &dyn Vfs, root: &Path) -> Option<Reservation> {
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
        .map(|entry| entry.file_name)
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
fn a_non_file_marker_is_not_a_created_root() {
    on_every_implementation("coord-marker-kind", &|implementation, backend| {
        let vfs = backend.process();
        let root = backend.root();
        create_through(implementation, Arc::clone(&vfs), &root, INPUTS, 1).expect("create");
        vfs.remove_file(&root.join(MARKER))
            .expect("remove the marker file");
        vfs.create_dir(&root.join(MARKER))
            .expect("replace the marker with a directory");

        assert!(matches!(
            create_through(implementation, backend.process(), &root, INPUTS, 2),
            Err(CoordError::Occupied { .. })
        ));
        assert!(matches!(
            implementation
                .open(backend.process(), &root)
                .expect("open")
                .confirm_created(MARKER),
            Err(CoordError::NotCreated)
        ));
    });
}

#[test]
fn a_created_root_is_found_when_artifact_names_only_differ_by_case() {
    on_every_implementation("coord-create-folded", &|implementation, backend| {
        let vfs = backend.process();
        let root = backend.root();
        create_through(implementation, Arc::clone(&vfs), &root, INPUTS, 1).expect("create");
        let folded = ["FILES", "LOG", "MARKER"];
        let mut maker = FileMaker::new(Arc::clone(&vfs), &root);
        let participant = implementation.open(Arc::clone(&vfs), &root).expect("open");
        assert_eq!(
            participant
                .create(
                    &CreationPlan {
                        inputs: INPUTS,
                        nonce: nonce(2),
                        artifacts: &folded,
                    },
                    &mut maker,
                )
                .expect("find the store"),
            Created::Existing
        );
        assert!(maker.made.is_empty());
        participant
            .confirm_created(folded[folded.len() - 1])
            .expect("confirm the marker");
    });
}

#[test]
fn a_folded_resume_uses_the_physical_names_of_existing_artifacts() {
    on_every_implementation("coord-resume-folded", &|implementation, backend| {
        let vfs = backend.process();
        let root = backend.root();
        let participant = implementation.open(Arc::clone(&vfs), &root).expect("open");
        let mut interrupted = FileMaker::new(Arc::clone(&vfs), &root).failing_at(2);
        assert!(create(&*participant, INPUTS, nonce(1), &mut interrupted).is_err());

        let folded = ["FILES", "LOG", "MARKER"];
        let mut resumed = FileMaker::new(Arc::clone(&vfs), &root);
        assert_eq!(
            participant
                .create(
                    &CreationPlan {
                        inputs: INPUTS,
                        nonce: nonce(2),
                        artifacts: &folded,
                    },
                    &mut resumed,
                )
                .expect("resume the store"),
            Created::Resumed
        );
        assert_eq!(resumed.made, ["files", "log", "MARKER"].map(str::to_owned));
        assert_eq!(
            store_names(&*vfs, &root),
            ["MARKER", "files", "log"],
            "the resumed artifacts do not create differently-cased duplicates"
        );
    });
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
        assert_eq!(maker.made, ARTIFACTS.map(str::to_owned));
        assert_created(&*vfs, &root, INPUTS);
        let recorded = reservation(&*vfs, &root).expect("a reservation");
        assert_eq!(&recorded.implementation, implementation.implementation());
        assert_eq!(recorded.nonce, nonce(1));
        assert_eq!(recorded.inputs, INPUTS);
        participant
            .confirm_created(MARKER)
            .unwrap_or_else(|error| panic!("{}: {error}", backend.name()));
    });
}

#[test]
fn a_created_root_is_found_by_an_identical_creator_and_refused_to_any_other() {
    on_every_implementation("coord-create-again", &|implementation, backend| {
        let root = backend.root();
        create_through(implementation, backend.process(), &root, INPUTS, 1).expect("create");
        let vfs = backend.process();
        let before = store_names(&*vfs, &root);
        assert_eq!(
            create_through(implementation, backend.process(), &root, INPUTS, 2).expect("found"),
            Created::Existing
        );
        assert!(matches!(
            create_through(implementation, backend.process(), &root, OTHER_INPUTS, 3),
            Err(CoordError::AlreadyCreated)
        ));
        for again in implementations() {
            if again.implementation() != implementation.implementation() {
                assert!(matches!(
                    create_through(&again, backend.process(), &root, INPUTS, 4),
                    Err(CoordError::AlreadyCreated)
                ));
            }
        }
        assert_eq!(store_names(&*vfs, &root), before, "nothing is made again");
        assert_eq!(reservation(&*vfs, &root).expect("reserved").nonce, nonce(1));
        assert_created(&*vfs, &root, INPUTS);
    });
}

#[test]
fn of_identical_creators_at_once_one_creates_and_the_others_find_its_store() {
    for round in 0..8 {
        on_every_implementation("coord-identical-race", &|implementation, backend| {
            let root = backend.root();
            let start = Arc::new(Barrier::new(4));
            let racers: Vec<_> = (1..=4u8)
                .map(|n| {
                    let (implementation, vfs, root, start) = (
                        implementation.clone(),
                        backend.process(),
                        root.clone(),
                        Arc::clone(&start),
                    );
                    thread::spawn(move || {
                        start.wait();
                        create_through(&implementation, vfs, &root, INPUTS, n)
                    })
                })
                .collect();
            let outcomes: Vec<_> = racers
                .into_iter()
                .map(|racer| racer.join().expect("a racer"))
                .collect();
            let created = outcomes
                .iter()
                .filter(|outcome| matches!(outcome, Ok(Created::Created)))
                .count();
            assert_eq!(created, 1, "{} round {round}: {outcomes:?}", backend.name());
            assert!(
                outcomes
                    .iter()
                    .all(|outcome| matches!(outcome, Ok(Created::Created | Created::Existing))),
                "{} round {round}: {outcomes:?}",
                backend.name()
            );
            assert_created(&*backend.process(), &root, INPUTS);
        });
    }
}

#[test]
fn an_open_through_another_implementation_is_refused() {
    on_every_backend("coord-open-mismatch", &|backend| {
        let root = backend.root();
        create_through(&FileFactory::new(), backend.process(), &root, INPUTS, 1).expect("create");
        let other = FileFactory::alternate()
            .open(backend.process(), &root)
            .expect("open");
        match other.confirm_created(MARKER) {
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
            participant.confirm_created(MARKER),
            Err(CoordError::NotCreated)
        ));
        let vfs = backend.process();
        let mut maker = FileMaker::new(Arc::clone(&vfs), &root).failing_at(2);
        let creator = implementation.open(vfs, &root).expect("open");
        assert!(create(&*creator, INPUTS, nonce(1), &mut maker).is_err());
        assert!(matches!(
            participant.confirm_created(MARKER),
            Err(CoordError::NotCreated)
        ));
    });
}

#[test]
fn creation_checks_the_marker_staging_budget_before_reservation() {
    super::on_every_contract("coord-marker-budget", &|factory, backend| {
        let vfs = backend.process();
        let root = backend.root();
        let participant = factory.open(Arc::clone(&vfs), &root).unwrap();
        let mut maker = FileMaker::new(Arc::clone(&vfs), &root);
        let budget = CreationPlan::MAX_MARKER_BYTES;
        for marker in [
            "x".repeat(budget + 1),
            "x".repeat(255),
            "\u{e9}".repeat(budget / 2 + 1),
        ] {
            let plan = CreationPlan {
                inputs: INPUTS,
                nonce: nonce(1),
                artifacts: &[&marker],
            };
            assert!(matches!(
                participant.create(&plan, &mut maker),
                Err(CoordError::InvalidPlan { .. })
            ));
            assert!(vfs.read_dir(&root).unwrap().is_empty());
        }
        let marker = "x".repeat(budget);
        let plan = CreationPlan {
            inputs: INPUTS,
            nonce: nonce(2),
            artifacts: &[&marker],
        };
        participant.create(&plan, &mut maker).unwrap();
        assert!(vfs.exists(&root.join(marker)).unwrap());
    });
}

#[test]
fn raw_non_marker_names_obey_the_component_byte_ceiling() {
    on_every_implementation("coord-component-length", &|implementation, backend| {
        let fs = backend.process();
        let root = backend.root();
        let participant = implementation.open(fs.clone(), &root).unwrap();
        let oversized = "x".repeat(256);
        let plan = CreationPlan {
            inputs: INPUTS,
            nonce: nonce(12),
            artifacts: &[&oversized, "marker"],
        };
        let mut maker = FileMaker::new(fs.clone(), &root);
        assert!(matches!(
            participant.create(&plan, &mut maker),
            Err(CoordError::InvalidPlan { .. })
        ));
        assert!(fs.read_dir(&root).unwrap().is_empty());

        let maximum = "x".repeat(255);
        let plan = CreationPlan {
            inputs: INPUTS,
            nonce: nonce(13),
            artifacts: &[&maximum, "marker"],
        };
        assert_eq!(
            participant.create(&plan, &mut maker).unwrap(),
            Created::Created
        );
        assert!(fs.exists(&root.join(maximum)).unwrap());
    });
}

#[test]
fn a_plan_that_creation_could_not_make_is_refused_and_changes_nothing() {
    on_every_implementation("coord-plan", &|implementation, backend| {
        let vfs = backend.process();
        let root = backend.root();
        let participant = implementation.open(Arc::clone(&vfs), &root).expect("open");
        let bad: &[&[&str]] = &[
            &["\u{1f80}", "\u{1f88}"],
            &["\u{1fb3}x", "\u{1fbc}X", "marker"],
            &["\u{1f80}", "\u{1f88}.publishing.1-0"],
            &["\u{1ff3}", "\u{1ffc}"],
            &[],
            &["dir/marker"],
            &["dir\\marker"],
            &[".."],
            &[".coord-marker"],
            &["log", "log"],
            &["files", ""],
            &[".COORD-reservation"],
            &["log", "LOG", "marker"],
            &["\u{e9}t\u{e9}", "\u{c9}T\u{c9}"],
            &["files", "files.publishing.1-0"],
            &["files", "FILES.publishing.x", "marker"],
            &["files.publishing.7", "files"],
            &["bad\0name", "marker"],
            &["C:artifact", "marker"],
            &["artifact", "z:marker"],
            &["marker.", "marker"],
            &["marker ", "marker"],
            &["report:summary", "marker"],
            &["CON", "marker"],
            &["con.txt", "marker"],
            &["COM0", "marker"],
            &["LPT9.ext", "marker"],
            &["COM\u{b9}", "marker"],
            &["com\u{b2}.txt", "marker"],
            &["COM\u{b3}", "marker"],
            &["LPT\u{b9}", "marker"],
            &["lpt\u{b2}.ext", "marker"],
            &["LPT\u{b3}", "marker"],
            &["NUL .txt", "marker"],
            &["Con  .dat", "marker"],
            &["COM\u{b9} .txt", "marker"],
            &["lpt\u{b2} .log", "marker"],
            &["bad?name", "marker"],
            &["bad*name", "marker"],
            &["bad|name", "marker"],
            &["bad<name", "marker"],
            &["bad>name", "marker"],
            &["bad\"name", "marker"],
            &["bad\u{1f}name", "marker"],
        ];
        for artifacts in bad {
            let plan = CreationPlan {
                inputs: INPUTS,
                nonce: nonce(1),
                artifacts,
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
        for invalid in [
            "a/b",
            "marker.",
            "marker ",
            "report:summary",
            "CON",
            "NUL .txt",
            "Con  .dat",
            "COM\u{b9} .txt",
            "lpt\u{b2} .log",
        ] {
            assert!(matches!(
                participant.confirm_created(invalid),
                Err(CoordError::InvalidPlan { .. })
            ));
        }
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
fn a_publication_left_beside_any_artifact_does_not_keep_its_creation_from_finishing() {
    on_every_implementation("coord-publication-left", &|implementation, backend| {
        let root = backend.root();
        for (at, artifact) in ARTIFACTS.iter().enumerate() {
            let vfs = backend.process();
            let creator = implementation.open(Arc::clone(&vfs), &root).expect("open");
            let mut interrupted = FileMaker::new(Arc::clone(&vfs), &root).failing_at(at);
            assert!(create(&*creator, INPUTS, nonce(1), &mut interrupted).is_err());
            creator.abandon();
            let left = root.join(format!("{}.PUBLISHING.9-9", artifact.to_ascii_uppercase()));
            vfs.write(&left, b"cut short")
                .expect("a case-variant publication left behind");
            assert_eq!(
                create_through(implementation, backend.process(), &root, INPUTS, 2)
                    .unwrap_or_else(|error| panic!("{} {artifact}: {error}", backend.name())),
                Created::Resumed
            );
            assert_created(&*vfs, &root, INPUTS);
            for name in ARTIFACTS.iter().chain([&".coord-reservation"]) {
                let _ = vfs.remove_file(&root.join(name));
            }
            vfs.remove_file(&left).expect("remove the leftover");
        }
    });
}

#[test]
fn a_publication_left_with_no_reservation_or_beside_no_artifact_occupies_the_root() {
    on_every_implementation("coord-publication-foreign", &|implementation, backend| {
        let vfs = backend.process();
        let root = backend.root();
        for left in ["marker.publishing.1-0", "stray.publishing.1-0"] {
            vfs.write(&root.join(left), b"x").expect("a leftover");
            match create_through(implementation, backend.process(), &root, INPUTS, 1) {
                Err(CoordError::Occupied { name }) => assert_eq!(name, left),
                other => panic!("{}: {left}: {other:?}", backend.name()),
            }
            assert!(reservation(&*vfs, &root).is_none(), "nothing is reserved");
            vfs.remove_file(&root.join(left))
                .expect("remove the leftover");
        }
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
fn a_creator_abandoned_by_its_maker_makes_no_further_artifact_and_is_resumed() {
    on_every_implementation("coord-create-abandoned", &|implementation, backend| {
        let root = backend.root();
        let vfs = backend.process();
        let creator: Arc<dyn Coordination> = implementation
            .open(Arc::clone(&vfs), &root)
            .expect("open")
            .into();
        let dying = Arc::clone(&creator);
        let mut maker = FileMaker::new(Arc::clone(&vfs), &root).before(0, move || dying.abandon());
        assert!(matches!(
            create(&*creator, INPUTS, nonce(1), &mut maker),
            Err(CoordError::Abandoned)
        ));
        assert_eq!(
            maker.made.len(),
            1,
            "only the artifact being made when it died"
        );
        assert!(!vfs.exists(&root.join(MARKER)).unwrap());
        assert_eq!(
            create_through(implementation, backend.process(), &root, INPUTS, 2).expect("resume"),
            Created::Resumed
        );
        assert_created(&*vfs, &root, INPUTS);
    });
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
                    create_through(&f1, creator_vfs, &r1, INPUTS, 3)
                        .map(|created| assert_eq!(created, Created::Existing))
                });
                let opener = still_waiting(move || {
                    f2.open(opener_vfs, &r2)
                        .expect("open")
                        .confirm_created(MARKER)
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
        assert!(matches!(creator.join().expect("joined"), Ok(())));
        opener
            .join()
            .expect("joined")
            .expect("the opener finds a store");
    }
}

#[test]
fn a_creator_suspended_after_publishing_refuses_another_implementation_and_holds_off_its_own() {
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
                    .confirm_created(MARKER),
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
            Ok(Created::Existing)
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
            Ok(Created::Existing)
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
            .confirm_created(MARKER)
            .expect("the open finds the marker");
        let mem = mem.simulate_restart();
        assert_created(&mem, &root, INPUTS);
        implementation
            .open(Arc::new(mem.another_process()), &root)
            .expect("open")
            .confirm_created(MARKER)
            .expect("the store survives the crash");
    }
}

#[test]
fn an_identical_creator_makes_a_marker_its_creator_did_not_make_durable() {
    for implementation in implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        root_with_an_unsynced_marker(&implementation, &mem, &root);
        assert_eq!(
            create_through(
                &implementation,
                Arc::new(mem.another_process()),
                &root,
                INPUTS,
                2
            )
            .expect("found"),
            Created::Existing
        );
        let mem = mem.simulate_restart();
        assert_created(&mem, &root, INPUTS);
    }
}

#[test]
fn a_marker_no_open_made_durable_is_lost_to_a_crash() {
    let mem = Backend::mem();
    let root = PathBuf::from(Backend::MEM_ROOT);
    root_with_an_unsynced_marker(&FileFactory::new(), &mem, &root);
    let mem = mem.simulate_restart();
    assert!(!mem.exists(&root.join(MARKER)).expect("exists"));
}

#[test]
fn a_marker_cut_short_is_never_found_as_the_store() {
    for implementation in implementations() {
        for publication_fails in [false, true] {
            let mem = Backend::mem();
            let root = PathBuf::from(Backend::MEM_ROOT);
            let marker = root.join(MARKER);
            let policy = FaultPolicy::new()
                .inject(
                    FaultOp::Write,
                    &marker,
                    Fault::ShortWrite { persist: 3 },
                    When::Always,
                )
                .inject(
                    FaultOp::PublishNew,
                    &marker,
                    Fault::io_error(),
                    if publication_fails {
                        When::Always
                    } else {
                        When::OnCall(0)
                    },
                );
            let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(mem.another_process(), policy));
            let creator = implementation.open(Arc::clone(&vfs), &root).expect("open");
            let first = create(
                &*creator,
                INPUTS,
                nonce(1),
                &mut FileMaker::new(Arc::clone(&vfs), &root),
            );
            creator.abandon();
            create_through(
                &implementation,
                Arc::new(mem.another_process()),
                &root,
                INPUTS,
                2,
            )
            .expect("an identical creator ends with the store");
            assert_created(&mem, &root, INPUTS);
            assert_eq!(first.is_err(), publication_fails, "{first:?}");
        }
    }
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
            let mem = mem.simulate_restart();

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
            let mem = mem.simulate_restart();
            assert_created(&mem, &root, INPUTS);
            implementation
                .open(Arc::new(mem.another_process()), &root)
                .expect("open")
                .confirm_created(MARKER)
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
