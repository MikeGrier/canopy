// Copyright (c) Michael Grier.

use super::*;
use crate::coord::{ArtifactMaker, Created, CreationNonce, CreationPlan, ModificationEnd, Purpose};
use std::ffi::OsStr;

struct Maker {
    fs: Arc<dyn Vfs>,
    root: PathBuf,
}
impl ArtifactMaker for Maker {
    fn make(&mut self, name: &OsStr, inputs: &[u8]) -> CoordResult<()> {
        let path = self.root.join(name);
        self.fs.write(&path, inputs)?;
        self.fs.fsync(&path)?;
        Ok(())
    }
}

fn filesystem() -> Arc<crate::MemFs> {
    let fs = Arc::new(crate::MemFs::new());
    fs.create_dir_all(Path::new("/factory")).unwrap();
    fs
}

#[test]
fn factory_open_is_lazy_and_facets_are_reached_through_the_public_contract() {
    let fs = filesystem();
    let before = fs.read_dir(Path::new("/factory")).unwrap();
    let factory = SharedFactory::new();
    let participant = factory.open(fs.clone(), Path::new("/factory")).unwrap();
    assert_eq!(fs.read_dir(Path::new("/factory")).unwrap(), before);
    assert_eq!(participant.implementation(), factory.implementation());
    assert!(!participant.sample().unwrap().is_settled());
    let mut hold = participant.acquire(Waiting::Wait).unwrap();
    hold.complete_recovery().unwrap();
    let count = participant.sample().unwrap();
    hold.begin_modification(Purpose::Ordinary).unwrap();
    assert!(!participant.sample().unwrap().is_settled());
    hold.end_modification(ModificationEnd::Settled).unwrap();
    assert_eq!(participant.sample().unwrap().get(), count.get() + 2);
}

#[test]
fn normal_creation_records_the_identity_and_marker_last() {
    for value in 0..12 {
        let fs = filesystem();
        let participant = SharedFactory::new()
            .open(fs.clone(), Path::new("/factory"))
            .unwrap();
        let inputs = vec![value; value as usize];
        let names = [OsStr::new("data"), OsStr::new("marker")];
        let plan = CreationPlan {
            inputs: &inputs,
            nonce: CreationNonce::from_bytes([value; 16]),
            artifacts: &names,
        };
        let mut maker = Maker {
            fs: fs.clone(),
            root: PathBuf::from("/factory"),
        };
        assert_eq!(
            participant.create(&plan, &mut maker).unwrap(),
            Created::Created
        );
        participant.confirm_created(OsStr::new("marker")).unwrap();
        assert_eq!(fs.read(Path::new("/factory/data")).unwrap(), inputs);
        assert!(matches!(
            participant.create(&plan, &mut maker),
            Err(CoordError::AlreadyCreated)
        ));
    }
}

#[test]
fn foreign_reservations_are_refused_before_shared_initialization() {
    for file_first in [false, true] {
        let fs = filesystem();
        let file = crate::coord::file::FileFactory::new()
            .open(fs.clone(), Path::new("/factory"))
            .unwrap();
        let shared = SharedFactory::new()
            .open(fs.clone(), Path::new("/factory"))
            .unwrap();
        let (first, second) = if file_first {
            (&file, &shared)
        } else {
            (&shared, &file)
        };
        first
            .reserve(b"inputs", CreationNonce::from_bytes([1; 16]))
            .unwrap();
        assert!(matches!(
            second.reserve(b"inputs", CreationNonce::from_bytes([1; 16])),
            Err(CoordError::Reserved)
        ));
        fs.write(Path::new("/factory/marker"), b"marker").unwrap();
        assert!(matches!(
            second.confirm_created(OsStr::new("marker")),
            Err(CoordError::ImplementationMismatch { .. })
        ));
        if file_first {
            assert!(matches!(
                shared.sample(),
                Err(CoordError::ImplementationMismatch { .. })
            ));
        }
    }
}

#[test]
fn reservations_resume_only_identical_inputs_and_abandonment_refuses_all_facets() {
    let fs = filesystem();
    let participant = SharedFactory::new()
        .open(fs, Path::new("/factory"))
        .unwrap();
    let nonce = CreationNonce::from_bytes([1; 16]);
    assert_eq!(
        participant.reserve(b"same", nonce).unwrap(),
        Created::Created
    );
    assert_eq!(
        participant
            .reserve(b"same", CreationNonce::from_bytes([2; 16]))
            .unwrap(),
        Created::Resumed
    );
    assert!(matches!(
        participant.reserve(b"different", nonce),
        Err(CoordError::Reserved)
    ));
    participant.abandon();
    assert!(matches!(participant.sample(), Err(CoordError::Abandoned)));
    assert!(matches!(
        participant.acquire(Waiting::Never),
        Err(CoordError::Abandoned)
    ));
    assert!(matches!(
        participant.attach(Access::Read, Waiting::Never),
        Err(CoordError::Abandoned)
    ));
    assert!(matches!(
        participant.reserve(b"same", nonce),
        Err(CoordError::Abandoned)
    ));
}

#[test]
fn failed_creation_resumes_and_invalid_plans_leave_the_root_unreserved() {
    let fs = filesystem();
    let participant = SharedFactory::new()
        .open(fs.clone(), Path::new("/factory"))
        .unwrap();
    let inputs = b"creation";
    for name in ["../escape", ".coord-owned", ""] {
        let names = [OsStr::new(name)];
        let plan = CreationPlan {
            inputs,
            nonce: CreationNonce::from_bytes([1; 16]),
            artifacts: &names,
        };
        assert!(matches!(
            participant.create(
                &plan,
                &mut Maker {
                    fs: fs.clone(),
                    root: PathBuf::from("/factory")
                }
            ),
            Err(CoordError::InvalidPlan { .. })
        ));
        assert!(!fs.exists(Path::new("/factory/.coord-reservation")).unwrap());
    }
    struct Failed;
    impl ArtifactMaker for Failed {
        fn make(&mut self, _: &OsStr, _: &[u8]) -> CoordResult<()> {
            Err(CoordError::Malformed {
                what: "failed artifact",
            })
        }
    }
    let names = [OsStr::new("data"), OsStr::new("marker")];
    let plan = CreationPlan {
        inputs,
        nonce: CreationNonce::from_bytes([1; 16]),
        artifacts: &names,
    };
    assert!(participant.create(&plan, &mut Failed).is_err());
    assert!(matches!(
        participant.confirm_created(OsStr::new("marker")),
        Err(CoordError::NotCreated)
    ));
    assert_eq!(
        participant
            .create(
                &plan,
                &mut Maker {
                    fs,
                    root: PathBuf::from("/factory")
                }
            )
            .unwrap(),
        Created::Resumed
    );
}

#[test]
fn creation_abandonment_releases_exclusion_and_cannot_make_the_next_artifact() {
    let fs = filesystem();
    let participant = SharedFactory::new()
        .open(fs.clone(), Path::new("/factory"))
        .unwrap();
    struct Abandon<'a> {
        participant: &'a dyn Coordination,
    }
    impl ArtifactMaker for Abandon<'_> {
        fn make(&mut self, _: &OsStr, _: &[u8]) -> CoordResult<()> {
            self.participant.abandon();
            Ok(())
        }
    }
    let names = [OsStr::new("data"), OsStr::new("marker")];
    let plan = CreationPlan {
        inputs: b"inputs",
        nonce: CreationNonce::from_bytes([1; 16]),
        artifacts: &names,
    };
    assert!(matches!(
        participant.create(
            &plan,
            &mut Abandon {
                participant: participant.as_ref()
            }
        ),
        Err(CoordError::Abandoned)
    ));
    assert!(!fs.exists(Path::new("/factory/marker")).unwrap());
    let next = SharedFactory::new()
        .open(fs.clone(), Path::new("/factory"))
        .unwrap();
    assert_eq!(
        next.create(
            &plan,
            &mut Maker {
                fs,
                root: PathBuf::from("/factory")
            }
        )
        .unwrap(),
        Created::Resumed
    );
}
