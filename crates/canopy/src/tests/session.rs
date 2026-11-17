// Copyright (c) Michael Grier.

//! Session queue pairs and the asynchronous engine's public outcomes.

use super::*;

use std::sync::mpsc;
use std::time::Duration;

use crate::Read;
use crate::ring::{Command, CommandValue, SnapshotCommand, SubmissionRefusal, SubmitError};

const TIMEOUT: Duration = Duration::from_secs(3);

fn seed(store: &Store, name: &str, document: &str, contents: &[u8]) {
    store
        .update_document(Swap::new(super::name(name), doc(document), body(contents)))
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
}

#[test]
fn a_pair_reserves_its_completion_capacity_before_accepting() {
    let (_vfs, store) = sourced_store();
    let session = store.session().unwrap();
    let (mut submissions, completions) = session.queue_pair(1).unwrap();
    let first = submissions
        .push(Command::ListDocuments)
        .expect("first slot is free");
    let error = submissions
        .push(Command::ListDocuments)
        .expect_err("the second slot is reserved");
    assert_eq!(first.get(), 1);
    assert_eq!(error, SubmissionRefusal::CompletionQueueFull);
    drop((submissions, completions));
}

#[test]
fn commands_complete_in_pair_order() {
    let (_vfs, store) = sourced_store();
    seed(&store, "alpha", "one", b"1");
    seed(&store, "alpha", "two", b"2");
    let session = store.session().unwrap();
    let (mut submissions, mut completions) = session.queue_pair(3).unwrap();
    let first = submissions
        .push(Command::ReadDocument(name("alpha"), doc("one")))
        .unwrap();
    let second = submissions
        .push(Command::ReadDocument(name("alpha"), doc("two")))
        .unwrap();
    submissions.notify().unwrap();
    let first_completion = completions.wait(TIMEOUT).unwrap().unwrap();
    let second_completion = completions.wait(TIMEOUT).unwrap().unwrap();
    assert_eq!(first_completion.request(), first);
    assert_eq!(second_completion.request(), second);
    assert!(
        matches!(first_completion.outcome(), Ok(CommandValue::Read(Read::Found(body))) if body.as_bytes() == b"1")
    );
    assert!(
        matches!(second_completion.outcome(), Ok(CommandValue::Read(Read::Found(body))) if body.as_bytes() == b"2")
    );
}

#[test]
fn an_unnotified_pair_drop_withdraws_every_entry() {
    let (_vfs, store) = sourced_store();
    let transaction = {
        let mut builder = store.transaction();
        builder.write(name("alpha"), doc("one"), body(b"withdrawn"));
        builder.build().unwrap()
    };
    let session = store.session().unwrap();
    {
        let (mut submissions, completions) = session.queue_pair(1).unwrap();
        submissions
            .push(Command::Transaction(transaction, crate::TransactionOptions))
            .unwrap();
        drop((submissions, completions));
    }
    test_timers::thread::sleep(Duration::from_millis(50));
    assert_eq!(text(&store, "alpha", "one"), None);
}

#[test]
fn a_completion_watch_runs_on_readiness() {
    let (_vfs, store) = sourced_store();
    let session = store.session().unwrap();
    let (mut submissions, mut completions) = session.queue_pair(1).unwrap();
    let (sender, receiver) = mpsc::channel();
    let watch = completions
        .watch(
            move |completion| sender.send(completion.request()).unwrap(),
            |error| panic!("completion watch failed: {error}"),
        )
        .unwrap();
    let request = submissions.submit(Command::ListDocuments).unwrap();
    assert_eq!(
        test_timers::mpsc::recv_timeout(&receiver, TIMEOUT).unwrap(),
        request
    );
    drop(watch);
}

#[test]
fn a_blocked_transaction_does_not_hold_an_engine_worker() {
    let (_vfs, store) = sourced_store();
    let session = store.session().unwrap();
    let (mut submissions, mut completions) = session.queue_pair(1).unwrap();
    let guard = store.exclusive();
    let mut builder = store.transaction();
    builder.write(name("alpha"), doc("one"), body(b"later"));
    let request = submissions
        .submit(Command::Transaction(
            builder.build().unwrap(),
            crate::TransactionOptions,
        ))
        .unwrap();
    let other = submissions.submit(Command::ListDocuments).unwrap_err();
    assert!(matches!(
        other,
        SubmitError::Refused(SubmissionRefusal::CompletionQueueFull)
    ));
    assert!(
        completions
            .wait(Duration::from_millis(50))
            .unwrap()
            .is_none()
    );
    drop(guard);
    let completion = completions.wait(TIMEOUT).unwrap().unwrap();
    assert_eq!(completion.request(), request);
    assert!(matches!(
        completion.outcome(),
        Ok(CommandValue::Transaction(_))
    ));
}

#[test]
fn a_parked_write_does_not_block_another_pair() {
    let (_vfs, store) = sourced_store();
    let session = store.session().unwrap();
    let (mut blocked_submit, mut blocked_complete) = session.queue_pair(1).unwrap();
    let (mut ready_submit, mut ready_complete) = session.queue_pair(1).unwrap();
    let guard = store.exclusive();
    let mut builder = store.transaction();
    builder.write(name("alpha"), doc("one"), body(b"later"));
    blocked_submit
        .submit(Command::Transaction(
            builder.build().unwrap(),
            crate::TransactionOptions,
        ))
        .unwrap();
    let ready = ready_submit.submit(Command::ListDocuments).unwrap();
    let completion = ready_complete.wait(TIMEOUT).unwrap().unwrap();
    assert_eq!(completion.request(), ready);
    assert!(blocked_complete.try_pop().is_none());
    drop(guard);
    assert!(blocked_complete.wait(TIMEOUT).unwrap().is_some());
}

#[test]
fn synchronous_session_helpers_wrap_submit_and_wait() {
    let (_vfs, store) = sourced_store();
    let session = store.session().unwrap();
    let mut builder = store.transaction();
    builder.write(name("alpha"), doc("one"), body(b"session"));
    let (_, outcome) = session.transact(builder.build().unwrap(), TIMEOUT).unwrap();
    assert!(outcome.is_committed());
    assert_eq!(text(&store, "alpha", "one").as_deref(), Some("session"));
}

#[test]
fn fixed_snapshot_and_enumeration_commands_cross_the_queue() {
    let (_vfs, store) = sourced_store();
    seed(&store, "alpha", "one", b"1");
    let session = store.session().unwrap();
    let (mut submissions, mut completions) = session.queue_pair(2).unwrap();
    let read = submissions
        .submit(Command::Snapshot(SnapshotCommand::ReadDocument(
            name("alpha"),
            doc("one"),
        )))
        .unwrap();
    let list = submissions
        .submit(Command::Snapshot(SnapshotCommand::ListDocuments))
        .unwrap();
    let first = completions.wait(TIMEOUT).unwrap().unwrap();
    let second = completions.wait(TIMEOUT).unwrap().unwrap();
    assert_eq!(first.request(), read);
    assert_eq!(second.request(), list);
    assert!(matches!(first.outcome(), Ok(CommandValue::Snapshot(_))));
    assert!(matches!(second.outcome(), Ok(CommandValue::Snapshot(_))));
}

#[test]
fn dropping_one_end_closes_the_pair_for_the_other_end() {
    let (_vfs, store) = sourced_store();
    let session = store.session().unwrap();
    let (mut submissions, completions) = session.queue_pair(1).unwrap();
    drop(completions);
    assert!(matches!(
        submissions.push(Command::ListDocuments),
        Err(SubmissionRefusal::PairClosed)
    ));
}

#[test]
fn bounded_wait_reports_none_when_no_completion_is_ready() {
    let (_vfs, store) = sourced_store();
    let session = store.session().unwrap();
    let (submissions, mut completions) = session.queue_pair(1).unwrap();
    assert!(
        completions
            .wait(Duration::from_millis(20))
            .unwrap()
            .is_none()
    );
    drop(submissions);
}

#[test]
fn many_pairs_all_make_progress() {
    let (_vfs, store) = sourced_store();
    let session = store.session().unwrap();
    let pairs = 8;
    let mut ends = Vec::new();
    for _ in 0..pairs {
        let (mut submissions, completions) = session.queue_pair(1).unwrap();
        let request = submissions.submit(Command::ListDocuments).unwrap();
        ends.push((request, submissions, completions));
    }
    for (request, submissions, mut completions) in ends {
        assert_eq!(
            completions.wait(TIMEOUT).unwrap().unwrap().request(),
            request
        );
        drop(submissions);
    }
}

#[test]
fn synchronous_read_and_enumeration_wrappers_preserve_command_values() {
    let (_vfs, store) = sourced_store();
    seed(&store, "alpha", "one", b"1");
    let session = store.session().unwrap();
    let (_, read) = session
        .read_document(name("alpha"), doc("one"), TIMEOUT)
        .unwrap();
    assert!(matches!(read, Read::Found(body) if body.as_bytes() == b"1"));
    let (_, documents) = session.list_documents(TIMEOUT).unwrap();
    assert_eq!(documents, vec![(name("alpha"), doc("one"))]);
}

struct BlockingObserver {
    entered: mpsc::Sender<()>,
    release: std::sync::Mutex<mpsc::Receiver<()>>,
}

impl crate::CommitObserver for BlockingObserver {
    fn committed(&self, _node: &NodePath, _document: &DocumentName, _body: &DocumentBody) {
        self.entered.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
    }

    fn removed(&self, _node: &NodePath, _document: &DocumentName) {}
}

#[test]
fn dropping_a_pair_does_not_cancel_started_work() {
    let (_vfs, store) = sourced_store();
    let (entered_sender, entered) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    store
        .observe(Arc::new(BlockingObserver {
            entered: entered_sender,
            release: std::sync::Mutex::new(wait),
        }))
        .unwrap();
    let mut builder = store.transaction();
    builder.write(name("alpha"), doc("one"), body(b"committed"));
    let session = store.session().unwrap();
    let (mut submissions, completions) = session.queue_pair(1).unwrap();
    submissions
        .submit(Command::Transaction(
            builder.build().unwrap(),
            crate::TransactionOptions,
        ))
        .unwrap();
    test_timers::mpsc::recv_timeout(&entered, TIMEOUT).unwrap();
    drop((submissions, completions));
    release.send(()).unwrap();
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        if text(&store, "alpha", "one").as_deref() == Some("committed") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "started command did not finish"
        );
        std::thread::yield_now();
    }
}

#[test]
fn queue_ends_are_send() {
    fn assert_send<T: Send>() {}
    assert_send::<crate::ring::SubmissionQueue<'static>>();
    assert_send::<crate::ring::CompletionQueue<'static>>();
}
