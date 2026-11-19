// Copyright (c) Michael Grier.

use super::*;
use crate::Vfs;
use crate::coord::CoordError;
use std::path::Path;
use std::sync::Arc;

#[test]
fn a_panicking_gate_release_refuses_the_file_attachment() {
    let fs = Arc::new(crate::MemFs::new());
    let root = Path::new("/admission-gate-release");
    fs.create_dir_all(root).unwrap();
    let vfs: Arc<dyn Vfs> = fs.clone();
    let participant = crate::coord::file::FileFactory::new().participant(vfs, root);
    participant
        .vfs()
        .create_dir_all(&participant.layout().state_dir())
        .unwrap();
    fs.panic_when_next_lock_drops(&participant.layout().gate_lock());

    assert!(matches!(
        participant.attach(Access::Read, Waiting::Never),
        Err(CoordError::Abandoned)
    ));
}
