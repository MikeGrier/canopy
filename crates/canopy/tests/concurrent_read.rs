// Copyright (c) Michael Grier.

//! `CVS-22`: a reader interleaving with a **live** applier, on the content-verified format.
//!
//! The unit tests construct a mid-apply store by hand, which is the right way to pin the
//! classification -- the state is exact and the test is deterministic. They cannot show the
//! property that matters operationally, though: that a reader sampling replicas one at a time,
//! while another thread is genuinely part-way through `apply_transaction`, is never handed a
//! value no transaction ever wrote.
//!
//! # The invariant
//!
//! **A reader must never observe a body no transaction wrote.** Nothing in these tests
//! corrupts a blob, writes a divergent pointer, or kills a transaction, so every byte a read
//! returns must be traceable to a committed swap. This holds on the new format for a reason
//! the old one could not offer: a blob is immutable and named by its own digest, so a body
//! that verifies *is* a body someone wrote -- there is no vote that could synthesise one.
//!
//! **Being told the value is undetermined is not a violation of it.** The namespace is still
//! a mutable cell sampled one replica at a time (`D333`), so a read landing inside a commit
//! window can fail to find a majority. That is the store declining to answer, which is true
//! and is what `D319` entitles it to say; it is not damage and it is not an invented body.
//!
//! Since `D338` the store answers rather than erroring in both of the windows a live writer
//! opens. A **create** part-way through is *decidable*: the replicas holding nothing carry the
//! majority, so the read is `Absent` and stays monotonic. A **fast succession of writes** is
//! not decidable -- the pointer moves between two of the reader's samples, so three adjacent
//! versions are seen at once and no majority exists at that instant -- so it is returned as
//! [`Read::Contended`], which the caller's loop can wait out.
//!
//! # Why these tests force the window rather than hoping for it
//!
//! A concurrency test that merely hopes to hit a race passes vacuously when it misses. The
//! first three tests therefore *force* the interleaving: a fault observer on the namespace's
//! second replica parks a real apply mid-commit -- a real apply, really in progress, just
//! paused -- until the reader has looked. They cannot miss.
//!
//! [`readers_looping_against_a_live_writer_never_see_damage`] then runs the unforced thing the
//! property is about, at scale, and reports what it saw.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, MemFs, PathMatcher, Vfs, When};
use canopy::{
    Contention, DocumentBody, DocumentName, GenesisConfig, NodePath, Nonce, Read, ShireId, Store,
    StoreError, Swap, SwapSet, read_settled,
};

/// The store root these tests build under.
const ROOT: &str = "shire";

/// How long a test waits for the parked applier before calling the matcher broken.
///
/// A matcher that has stopped matching should fail this test, not hang it.
const PARK_TIMEOUT: Duration = Duration::from_secs(10);

fn node() -> NodePath {
    NodePath::parse("record").expect("valid node path")
}

fn document() -> DocumentName {
    DocumentName::new("record.json").expect("valid document name")
}

fn nonce(n: usize) -> Nonce {
    Nonce::new(&format!("t{n:06}")).expect("valid nonce")
}

/// The body written by transaction `n`.
fn body(n: usize) -> DocumentBody {
    DocumentBody::new(format!("value-{n:06}").into_bytes())
}

/// Installs the subscriber that carries canopy's own WARN lines out of a failing run.
///
/// `D309`'s lesson: without one, the store's account of the exact moment it failed is
/// discarded, and the failure has to be guessed at instead.
fn install_tracing() {
    let _ = komorebi::LogConfig::default()
        .with_default_directives("warn")
        .with_ansi(false)
        .install();
}

/// A fresh store at [`ROOT`] over `vfs`.
fn fresh_store(vfs: Arc<dyn Vfs>) -> Store {
    install_tracing();
    vfs.create_dir_all(Path::new(ROOT)).expect("create root");
    Store::init(
        vfs,
        ROOT,
        GenesisConfig::new(ShireId::new("cvs22-shire").expect("valid shire id"), 0),
    )
    .expect("init store")
}

/// Commits `body(n)` over the shared document.
fn commit(store: &Store, n: usize) {
    store
        .apply_transaction(
            &SwapSet::single(Swap::new(node(), document(), body(n))),
            &nonce(n),
        )
        .expect("a transaction over an uncontended writer path commits");
}

/// Parks a real apply at the moment it is committing the namespace's **second** replica.
///
/// A pointer move renames a staged temp over each replica's entry in turn, so pausing inside
/// `p1`'s rename leaves `p0` advanced and `p1`/`p2` not -- the exact one-of-three state a
/// reader can otherwise only catch by luck.
///
/// **Scoped to the namespace, and that is not decoration.** The blob tree carries its own
/// `p{i}` replica directories, and its renames happen in phase 2, *before* the commit point.
/// A matcher naming `p1` alone would park the apply before it had committed anything at all,
/// silently inverting the state these tests are built to observe.
///
/// Gated, so the seeding transaction runs unhindered: the rule claims no call until the gate
/// opens, so `Once` means "the first such rename after the gate opens".
fn parked_at_the_second_replica() -> (FaultPolicy, FaultGate, mpsc::Receiver<()>, mpsc::Sender<()>)
{
    let (paused_tx, paused_rx) = mpsc::channel::<()>();
    let (resume_tx, resume_rx) = mpsc::channel::<()>();
    // Both endpoints are `Send` but not `Sync`, and an observation must be both. The mutexes
    // are never contended -- one observer, firing once -- they only carry the bound.
    let paused_tx = Mutex::new(paused_tx);
    let resume_rx = Mutex::new(resume_rx);
    let gate = FaultGate::closed();
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Rename,
        PathMatcher::predicate(|path| {
            path.components().any(|c| c.as_os_str() == "namespace")
                && path.components().any(|c| c.as_os_str() == "p1")
        }),
        Fault::observe_with(move |_, _| {
            paused_tx
                .lock()
                .expect("uncontended")
                .send(())
                .expect("the reader is waiting");
            resume_rx
                .lock()
                .expect("uncontended")
                .recv()
                .expect("the reader releases the apply");
        }),
        When::Once,
        gate.clone(),
    );
    (policy, gate, paused_rx, resume_tx)
}

/// A store whose applier can be parked mid-commit, plus the channels that do the parking.
fn parkable() -> (Arc<Store>, FaultGate, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (policy, gate, paused, resume) = parked_at_the_second_replica();
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(MemFs::new(), policy));
    (Arc::new(fresh_store(vfs)), gate, paused, resume)
}

/// Waits for the applier to report it is parked, failing rather than hanging.
fn await_park(paused: &mpsc::Receiver<()>) {
    test_timers::mpsc::recv_timeout(paused, PARK_TIMEOUT)
        .expect("the applier never reached the namespace rename the matcher names");
}

#[test]
fn a_reader_inside_a_live_overwrite_sees_the_committed_value_it_has_not_yet_replaced() {
    // The overwrite window, and the one `D320` bought: with one replica advanced the majority
    // still holds the *previous* pointer, so the read is defined, and it is defined as the
    // last committed value rather than as an error or a guess. Monotonic, never atomic -- a
    // reader is told a true thing about the store, just not the newest true thing.
    //
    // **One commit in flight, which is the whole of the guarantee.** Two commits landing
    // across the reader's samples leave three adjacent versions and no majority; that is
    // `Contention::Undetermined`, and `readers_looping_against_a_live_writer_never_see_damage`
    // is where it shows up.
    let (store, gate, paused, resume) = parkable();
    commit(&store, 1);
    gate.open();

    let writer = {
        let store = Arc::clone(&store);
        std::thread::spawn(move || commit(&store, 2))
    };
    // Reaching the park *is* the evidence that `p0` is already committed and `p1` is not:
    // the observer fires inside `p1`'s rename, which the apply only reaches having finished
    // `p0`'s. Nothing further needs probing, and probing it would race the parked thread.
    await_park(&paused);

    assert_eq!(
        store
            .read_document(node(), &document())
            .expect("a mid-overwrite read is defined, because the majority still agrees"),
        Read::Found(body(1)),
        "a reader inside an overwrite window must see the value being replaced, not the \
         replacement and not an invention"
    );

    resume.send(()).expect("the applier is parked");
    writer.join().expect("the applier commits");

    assert_eq!(
        store.read_document(node(), &document()).expect("read"),
        Read::Found(body(2)),
        "and once the apply finishes, the new value"
    );
}

#[test]
fn a_reader_inside_a_live_create_is_told_the_document_is_not_there_yet() {
    // The create window: no previous pointer, so one replica holds the new entry and two hold
    // nothing. `D338` counts those two -- absence is a candidate like any other, and they are
    // holding the document's previous state exactly as a mid-overwrite majority holds the
    // previous pointer. So the window is *decidable*, not merely retryable.
    //
    // Monotonic, which is what makes the answer safe rather than merely convenient: absent
    // while a minority has landed, then found once a majority has, and never back again.
    let (store, gate, paused, resume) = parkable();
    gate.open();

    let writer = {
        let store = Arc::clone(&store);
        std::thread::spawn(move || commit(&store, 1))
    };
    await_park(&paused);

    match store.read_document(node(), &document()) {
        Ok(Read::Absent) => {}
        Ok(Read::Found(body)) => panic!("a create that has not committed yielded a body: {body:?}"),
        Ok(Read::Contended(contended)) => panic!(
            "a minority-landed create is decidable, not contended: {:?}",
            contended.cause()
        ),
        Err(error) => panic!("a live create is neither damage nor an I/O fault: {error:?}"),
    }

    resume.send(()).expect("the applier is parked");
    writer.join().expect("the applier commits");

    assert_eq!(
        store.read_document(node(), &document()).expect("read"),
        Read::Found(body(1)),
        "and once the apply finishes the document resolves normally"
    );
}

#[test]
fn a_hard_read_error_on_a_replica_surfaces_rather_than_falling_through_to_the_next() {
    // The distinction the content read draws, and the one worth pinning: a copy that is
    // *absent* is tolerable, because that is what a replica losing a file looks like and the
    // next replica answers. A copy whose read *fails* is not -- masking an I/O fault by
    // quietly reading elsewhere would turn a failing disk into a store that looks healthy
    // until every replica has gone.
    let gate = FaultGate::closed();
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Read,
        PathMatcher::predicate(|path| path.components().any(|c| c.as_os_str() == "files")),
        Fault::io_error(),
        When::Always,
        gate.clone(),
    );
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(MemFs::new(), policy));
    let store = fresh_store(vfs);
    commit(&store, 1);
    gate.open();

    match store.read_document(node(), &document()) {
        Err(StoreError::Vfs(error)) => {
            assert_ne!(
                error.kind(),
                acadia::VfsErrorKind::NotFound,
                "a NotFound is the tolerable case and must not arrive here"
            );
        }
        other => panic!("a faulted content read must surface a typed Vfs error: {other:?}"),
    }
}

#[test]
fn a_missing_blob_replica_is_read_around_without_the_caller_ever_knowing() {
    // The contrast that gives the test above its meaning: the same read, over a replica that
    // is simply gone, succeeds.
    let vfs: Arc<dyn Vfs> = Arc::new(MemFs::new());
    let store = fresh_store(Arc::clone(&vfs));
    commit(&store, 1);

    let target = store
        .read_pointer(&node(), &document())
        .expect("a pointer")
        .pointer()
        .target();
    let gone = store
        .root()
        .join("files")
        .join("p0")
        .join(target.relative_path());
    vfs.remove_file(&gone).expect("remove one replica's blob");

    assert_eq!(
        store.read_document(node(), &document()).expect("read"),
        Read::Found(body(1))
    );
}

#[test]
fn readers_looping_against_a_live_writer_never_see_damage() {
    // The unforced run: a real writer committing as fast as it can, and readers looping
    // against it with no coordination at all.
    //
    // **Seeded first**, so `Ok(None)` below can be an outright failure rather than the create
    // window showing through: once the document exists, absence is never a truthful answer.
    const WRITES: usize = 400;
    const READERS: usize = 3;

    let vfs: Arc<dyn Vfs> = Arc::new(MemFs::new());
    let store = Arc::new(fresh_store(vfs));
    commit(&store, 0);

    let reads = Arc::new(AtomicUsize::new(0));
    let contention = Arc::new(AtomicUsize::new(0));
    let undetermined = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicUsize::new(0));

    std::thread::scope(|threads| {
        for _ in 0..READERS {
            let store = Arc::clone(&store);
            let reads = Arc::clone(&reads);
            let contention = Arc::clone(&contention);
            let undetermined = Arc::clone(&undetermined);
            let done = Arc::clone(&done);
            threads.spawn(move || {
                let written: Vec<DocumentBody> = (0..=WRITES).map(body).collect();
                while done.load(Ordering::Acquire) == 0 {
                    // A budget rather than a deadline: being outrun is contention, and a
                    // reader that keeps being outrun is what `D336` made observable.
                    let mut attempts = 64usize;
                    let read = read_settled(&store, &node(), &document(), |contended| {
                        // Both shapes are reachable here and both are loopable since `D338`:
                        // the namespace failing to settle used to leave as an `Err`, which
                        // this loop never saw.
                        contention.fetch_add(1, Ordering::Release);
                        if let Contention::Undetermined { .. } = contended.cause() {
                            undetermined.fetch_add(1, Ordering::Release);
                        }
                        attempts -= 1;
                        attempts > 0
                    });
                    match read {
                        Ok(Some(body)) => {
                            assert!(
                                written.contains(&body),
                                "a reader observed a body no transaction wrote: {body:?}"
                            );
                            reads.fetch_add(1, Ordering::Release);
                        }
                        Ok(None) => panic!(
                            "the document was committed before any reader started; it can \
                             never be decidably absent"
                        ),
                        // The budget ran out, which is the only thing a healthy store under a
                        // writer that never pauses is allowed to do to a reader. Every
                        // contended attempt inside the budget is counted by the closure.
                        Err(StoreError::DocumentOvertaken { .. }) => {}
                        Err(other) => {
                            panic!("a healthy store under load reported a fault: {other:?}")
                        }
                    }
                }
            });
        }

        for n in 1..=WRITES {
            commit(&store, n);
        }
        done.store(1, Ordering::Release);
    });

    // Not asserted on: whether either contended shape is hit at all depends on how the writer
    // and readers happen to interleave, and pinning a count would pin the timing of the
    // machine that ran it. Read here only so a failure has the numbers to hand.
    let contention = contention.load(Ordering::Acquire);
    let undetermined = undetermined.load(Ordering::Acquire);
    assert!(
        reads.load(Ordering::Acquire) > 0,
        "no reader ever resolved a body ({contention} contended attempts, {undetermined} of \
         them undetermined), so this run proved nothing"
    );
    assert_eq!(
        store.read_document(node(), &document()).expect("read"),
        Read::Found(body(WRITES)),
        "the last write is what stands once the writer stops"
    );
}
