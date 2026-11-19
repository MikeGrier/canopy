// Copyright (c) Michael Grier.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use super::*;
use crate::coord::file::FileFactory;
use crate::{Fault, FaultOp, FaultPolicy, FaultyFs, OsFs, Vfs, When};

#[test]
fn a_registration_sample_failure_rings_without_falling_back_to_polling() {
    let temp = crate::tests::TempDir::new("oplock-sample-failure");
    let root = temp.path();
    let os = OsFs::new();
    let count = root.join(".coord/count");
    os.create_dir_all(count.parent().expect("count parent"))
        .unwrap();
    os.write(&count, b"not a count record").unwrap();
    let policy = FaultPolicy::new().inject(FaultOp::Read, count, Fault::io_error(), When::Once);
    let participant = FileFactory::new().participant(Arc::new(FaultyFs::new(os, policy)), root);

    let rings = Arc::new(AtomicUsize::new(0));
    let rung = Arc::clone(&rings);
    let (ringed, ring_thread) = mpsc::channel();
    let registering_thread = std::thread::current().id();
    let notice = match register(
        &participant,
        ModCount::new(1),
        Instant::now() + Duration::from_secs(10),
        Box::new(move || {
            rung.fetch_add(1, Ordering::AcqRel);
            let _ = ringed.send(std::thread::current().id());
        }),
    ) {
        Ok(notice) => notice,
        Err(_) => panic!("a completed native registration must return a rung notice"),
    };
    assert_ne!(
        test_timers::mpsc::recv_timeout(&ring_thread, Duration::from_secs(5)).unwrap(),
        registering_thread,
        "native failure rings on an implementation thread"
    );
    assert_eq!(rings.load(Ordering::Acquire), 1);
    drop(notice);
    assert_eq!(rings.load(Ordering::Acquire), 1);
}

#[derive(Debug)]
struct PublishOnDrop {
    fs: Arc<OsFs>,
    staging: std::path::PathBuf,
    target: std::path::PathBuf,
    result: mpsc::Sender<bool>,
}

impl Drop for PublishOnDrop {
    fn drop(&mut self) {
        let published = self.fs.write(&self.staging, b"published").is_ok()
            && self.fs.rename(&self.staging, &self.target).is_ok();
        let _ = self.result.send(published);
    }
}

#[test]
fn withdrawing_releases_native_io_before_dropping_user_state() {
    let temp = crate::tests::TempDir::new("oplock-withdrawal-order");
    let root = temp.path();
    let os = Arc::new(OsFs::new());
    let vfs: Arc<dyn Vfs> = os.clone();
    let participant = FileFactory::new().participant(vfs, root);
    let count = participant.layout().count();
    let staging = count.with_extension("replacement");
    os.create_dir_all(count.parent().expect("count parent"))
        .unwrap();
    os.write(&count, b"not a count record").unwrap();

    let (published, publication) = mpsc::channel();
    let publisher = PublishOnDrop {
        fs: Arc::clone(&os),
        staging,
        target: count,
        result: published,
    };
    let notice = match register(
        &participant,
        ModCount::new(1),
        Instant::now() + Duration::from_secs(10),
        Box::new(move || drop(publisher)),
    ) {
        Ok(notice) => notice,
        Err(_) => panic!("native registration was unavailable"),
    };

    let (dropped, drop_finished) = mpsc::channel();
    std::thread::spawn(move || {
        drop(notice);
        let _ = dropped.send(());
    });
    assert!(test_timers::mpsc::recv_timeout(&publication, Duration::from_secs(5)).unwrap());
    test_timers::mpsc::recv_timeout(&drop_finished, Duration::from_secs(5))
        .expect("withdrawal completed");
}
