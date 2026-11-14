// Copyright (c) Michael Grier.

//! The coordination interface's conformance suite: every case runs against every implementation
//! over every backend, and the cross-process cases in [`process`] against real processes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use test_timers::mpsc::recv_timeout;

use super::TempDir;
use crate::coord::file::{FileCoordination, FileFactory};
use crate::coord::{CoordinationFactory, ImplementationId};
use crate::{FaultPolicy, FaultyFs, MemFs, OsFs, Vfs};

mod admission;
mod basics;
mod count;
mod creation;
mod exclusion;
mod hold;
mod layout;
mod notify;
mod process;
mod registry;
mod restart;
mod values;

/// Long enough that a wait the case expects to end never reaches it.
const LONG: Duration = Duration::from_secs(30);

/// Every implementation, each of which every case runs against: the file-based one, and the second
/// one that differs from it in identity and creation exclusion.
fn implementations() -> Vec<FileFactory> {
    vec![FileFactory::new(), FileFactory::alternate()]
}

/// A filesystem the participants of a case share, with a view of it for each participant as a
/// process of its own would have.
enum Backend {
    Mem(MemFs),
    Os(TempDir),
    Faulty(MemFs),
}

impl Backend {
    const MEM_ROOT: &'static str = "/store";

    fn mem() -> MemFs {
        let fs = MemFs::new();
        let root = Path::new(Self::MEM_ROOT);
        fs.create_dir_all(root).expect("create the root");
        // A direct entry of MemFs's implicit root is durable once it is itself synced.
        fs.fsync(root).expect("make the root durable");
        fs
    }

    fn root(&self) -> PathBuf {
        match self {
            Backend::Mem(_) | Backend::Faulty(_) => PathBuf::from(Self::MEM_ROOT),
            Backend::Os(dir) => dir.path().to_path_buf(),
        }
    }

    /// The filesystem as one more participant sees it.
    fn process(&self) -> Arc<dyn Vfs> {
        match self {
            Backend::Mem(fs) => Arc::new(fs.another_process()),
            Backend::Os(_) => Arc::new(OsFs::new()),
            Backend::Faulty(fs) => {
                Arc::new(FaultyFs::new(fs.another_process(), FaultPolicy::new()))
            }
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Backend::Mem(_) => "MemFs",
            Backend::Os(_) => "OsFs",
            Backend::Faulty(_) => "FaultyFs",
        }
    }
}

/// Runs `case` over a fresh root on every backend.
fn on_every_backend(tag: &str, case: &dyn Fn(&Backend)) {
    for backend in [
        Backend::Mem(Backend::mem()),
        Backend::Os(TempDir::new(tag)),
        Backend::Faulty(Backend::mem()),
    ] {
        case(&backend);
    }
}

/// Runs `case` for every implementation, over a fresh root on every backend.
fn on_every_implementation(tag: &str, case: &dyn Fn(&FileFactory, &Backend)) {
    for implementation in implementations() {
        on_every_backend(tag, &|backend| case(&implementation, backend));
    }
}

/// A participant of `implementation` on `backend`'s root, as a process of its own, with the
/// directory its state lives in made.
fn participant(implementation: &FileFactory, backend: &Backend) -> FileCoordination {
    participant_on(implementation, backend.process(), &backend.root())
}

/// A participant of `implementation` over `vfs`, with the directory its state lives in made.
fn participant_on(
    implementation: &FileFactory,
    vfs: Arc<dyn Vfs>,
    root: &Path,
) -> FileCoordination {
    let participant = implementation.participant(vfs, root);
    participant
        .vfs()
        .create_dir_all(&participant.layout().state_dir())
        .expect("make the state directory");
    participant
}

/// The identity of every implementation.
fn identities() -> Vec<ImplementationId> {
    implementations()
        .iter()
        .map(|implementation| implementation.implementation().clone())
        .collect()
}

/// Runs `wait` on a thread of its own and reports whether it returns within `within`.
fn returns_within(within: Duration, wait: impl FnOnce() + Send + 'static) -> bool {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        wait();
        let _ = done.send(());
    });
    recv_timeout(&finished, within).is_ok()
}
