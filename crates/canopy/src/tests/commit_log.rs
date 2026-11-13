// Copyright (c) Michael Grier.
// Split from tests.rs at 2a4dd8f.

//! TXN-1: the append-only commit log -- record round-trips and torn-tail recovery.

use super::*;
use acadia::{Fault, FaultOp, FaultPolicy, FaultyFs, VfsError, VfsErrorKind, When};
use std::error::Error as _;

// -- Commit log (TXN-1) --------------------------------------------------------

#[test]
fn commit_log_lives_at_the_store_root_dotfile() {
    let (_vfs, root) = fresh_root();
    let log = CommitLog::new(Arc::new(MemFs::new()), &root);
    assert_eq!(log.path(), root.join(LOG_NAME));
    assert_eq!(LOG_NAME, ".commit-log");
}

#[test]
fn absent_log_reads_as_no_records() {
    let (_vfs, log) = fresh_log();
    assert_eq!(log.read().unwrap(), Vec::new());
}

#[test]
fn empty_log_file_reads_as_no_records() {
    let (vfs, log) = fresh_log();
    vfs.write(log.path(), b"").unwrap();
    assert_eq!(log.read().unwrap(), Vec::new());
}

#[test]
fn append_and_read_round_trips_a_begin() {
    let (_vfs, log) = fresh_log();
    let record = begin(&nonce("tx1"));
    log.append(&record).unwrap();
    assert_eq!(log.read().unwrap(), vec![record]);
}

#[test]
fn append_and_read_round_trips_an_end() {
    let (_vfs, log) = fresh_log();
    let record = LogRecord::End { txn: nonce("tx1") };
    log.append(&record).unwrap();
    assert_eq!(log.read().unwrap(), vec![record]);
}

#[test]
fn append_and_read_round_trips_a_prepare_with_its_swap_set() {
    let (_vfs, log) = fresh_log();
    let record = LogRecord::Prepare {
        txn: nonce("tx1"),
        prepared: prepared(&sample_swaps()),
    };
    log.append(&record).unwrap();
    assert_eq!(log.read().unwrap(), vec![record]);
}

#[test]
fn prepare_round_trips_an_empty_swap_set() {
    let (_vfs, log) = fresh_log();
    let record = LogRecord::Prepare {
        txn: nonce("tx1"),
        prepared: prepared(&SwapSet::new([]).unwrap()),
    };
    log.append(&record).unwrap();
    assert_eq!(log.read().unwrap(), vec![record]);
}

#[test]
fn a_conditional_swap_preserves_its_expected_digest() {
    let (_vfs, log) = fresh_log();
    let expected = body(b"prior").digest();
    let record = LogRecord::Prepare {
        txn: nonce("tx1"),
        prepared: prepared(&SwapSet::single(Swap::conditional(
            name("alpha"),
            doc("record"),
            body(b"new"),
            expected,
        ))),
    };
    log.append(&record).unwrap();
    let read = log.read().unwrap();
    let LogRecord::Prepare { prepared, .. } = &read[0] else {
        panic!("expected a prepare record");
    };
    assert_eq!(
        prepared.swaps()[0].precondition(),
        crate::Precondition::ExpectedDigest(expected)
    );
}

#[test]
fn a_conditional_absent_swap_preserves_its_precondition_across_the_log() {
    let (_vfs, log) = fresh_log();
    let record = LogRecord::Prepare {
        txn: nonce("tx1"),
        prepared: prepared(&SwapSet::single(Swap::conditional_absent(
            name("alpha"),
            doc("engine"),
            body(b"created"),
        ))),
    };
    log.append(&record).unwrap();
    let read = log.read().unwrap();
    let LogRecord::Prepare { prepared, .. } = &read[0] else {
        panic!("expected a prepare record");
    };
    // One assertion where there were two: the precondition is a single enum, so naming
    // the variant already says the swap carries no expected digest.
    assert_eq!(
        prepared.swaps()[0].precondition(),
        crate::Precondition::ExpectedAbsent
    );
}

#[test]
fn an_unconditional_swap_omits_the_false_expect_absent_field() {
    let (vfs, log) = fresh_log();
    log.append(&LogRecord::Prepare {
        txn: nonce("tx1"),
        prepared: prepared(&SwapSet::single(Swap::new(
            name("alpha"),
            doc("record"),
            body(b"new"),
        ))),
    })
    .unwrap();

    let encoded = String::from_utf8(vfs.read(log.path()).unwrap()).unwrap();
    assert!(!encoded.contains("expect_absent"));
}

#[test]
fn appends_accumulate_in_order() {
    let (_vfs, log) = fresh_log();
    let begin = begin(&nonce("tx1"));
    let prepare = LogRecord::Prepare {
        txn: nonce("tx1"),
        prepared: prepared(&sample_swaps()),
    };
    let end = LogRecord::End { txn: nonce("tx1") };
    log.append(&begin).unwrap();
    log.append(&prepare).unwrap();
    log.append(&end).unwrap();
    assert_eq!(log.read().unwrap(), vec![begin, prepare, end]);
}

#[test]
fn interleaved_transactions_round_trip_in_append_order() {
    let (_vfs, log) = fresh_log();
    let records = vec![
        begin(&nonce("tx1")),
        begin(&nonce("tx2")),
        LogRecord::Prepare {
            txn: nonce("tx1"),
            prepared: prepared(&SwapSet::single(Swap::new(
                name("alpha"),
                doc("record"),
                body(b"a"),
            ))),
        },
        LogRecord::End { txn: nonce("tx1") },
        LogRecord::Prepare {
            txn: nonce("tx2"),
            prepared: prepared(&SwapSet::single(Swap::new(
                name("bravo"),
                doc("record"),
                body(b"b"),
            ))),
        },
        LogRecord::End { txn: nonce("tx2") },
    ];
    for record in &records {
        log.append(record).unwrap();
    }
    assert_eq!(log.read().unwrap(), records);
}

#[test]
fn a_torn_final_line_is_discarded() {
    let (vfs, log) = fresh_log();
    let begin = begin(&nonce("tx1"));
    let end = LogRecord::End { txn: nonce("tx1") };
    log.append(&begin).unwrap();
    log.append(&end).unwrap();
    // A crash mid-append leaves a partial line with no terminating newline.
    vfs.append(log.path(), br#"{"type":"begin","txn":"TX2"#)
        .unwrap();
    assert_eq!(log.read().unwrap(), vec![begin, end]);
}

#[test]
fn a_partial_first_line_reads_as_no_records() {
    let (vfs, log) = fresh_log();
    // A crash during the very first append: bytes with no newline at all.
    vfs.write(log.path(), br#"{"type":"begin","txn":"TX1"#)
        .unwrap();
    assert_eq!(log.read().unwrap(), Vec::new());
}

#[test]
fn appending_after_a_torn_tail_still_reads_the_good_prefix() {
    let (vfs, log) = fresh_log();
    let begin = begin(&nonce("tx1"));
    log.append(&begin).unwrap();
    // Torn tail from an interrupted append...
    vfs.append(log.path(), b"{\"type\":\"end\"").unwrap();
    // ...only whole lines up to the last newline are authoritative.
    assert_eq!(log.read().unwrap(), vec![begin]);
}

#[test]
fn an_append_after_a_torn_tail_cuts_the_fragment_instead_of_welding_onto_it() {
    let (vfs, log) = fresh_log();
    let begin = begin(&nonce("tx1"));
    log.append(&begin).unwrap();
    vfs.append(log.path(), b"{\"type\":\"end\"").unwrap();

    let end = LogRecord::End { txn: nonce("tx1") };
    log.append(&end).unwrap();
    assert_eq!(log.read().unwrap(), vec![begin, end]);
}

#[test]
fn an_append_after_a_torn_first_line_starts_the_log_clean() {
    let (vfs, log) = fresh_log();
    vfs.write(log.path(), br#"{"type":"begin","txn":"TX1"#)
        .unwrap();

    let record = begin(&nonce("tx2"));
    log.append(&record).unwrap();
    assert_eq!(log.read().unwrap(), vec![record]);
}

#[test]
fn an_append_to_a_whole_log_leaves_its_records_alone() {
    let (vfs, log) = fresh_log();
    let first = begin(&nonce("tx1"));
    log.append(&first).unwrap();
    let before = vfs.read(log.path()).unwrap();

    let second = LogRecord::End { txn: nonce("tx1") };
    log.append(&second).unwrap();
    let after = vfs.read(log.path()).unwrap();
    assert!(after.starts_with(&before));
    assert_eq!(log.read().unwrap(), vec![first, second]);
}

#[test]
fn a_newly_created_log_survives_a_crash() {
    let mem = Arc::new(MemFs::new());
    let vfs: Arc<dyn Vfs> = mem.clone();
    let root = PathBuf::from("shire");
    vfs.create_dir_all(&root).unwrap();
    let log = CommitLog::new(Arc::clone(&vfs), &root);

    let record = begin(&nonce("tx1"));
    log.append(&record).unwrap();
    mem.simulate_crash();
    assert_eq!(log.read().unwrap(), vec![record]);
}

#[test]
fn a_failed_root_sync_is_retried_by_the_next_append() {
    // A log created with a root sync that failed must not stay losable just because later
    // appends find the file already there: the next append syncs the root again.
    let root = PathBuf::from("shire");
    let base = MemFs::new();
    base.create_dir_all(&root).unwrap();
    let policy = FaultPolicy::new().inject(FaultOp::Fsync, &root, Fault::io_error(), When::Once);
    let faulty = Arc::new(FaultyFs::new(base, policy));
    let vfs: Arc<dyn Vfs> = faulty.clone();
    let log = CommitLog::new(Arc::clone(&vfs), &root);

    log.append(&begin(&nonce("tx1")))
        .expect_err("the root sync after creating the log fails");
    log.reset().unwrap();
    let record = begin(&nonce("tx2"));
    log.append(&record).unwrap();

    faulty.inner().simulate_crash();
    assert_eq!(log.read().unwrap(), vec![record]);
}

#[test]
fn a_complete_but_non_json_line_is_a_decode_error() {
    let (vfs, log) = fresh_log();
    vfs.write(log.path(), b"not json at all\n").unwrap();
    assert!(matches!(log.read(), Err(CommitLogError::Decode(_))));
}

#[test]
fn a_complete_line_with_an_unknown_type_is_a_decode_error() {
    let (vfs, log) = fresh_log();
    vfs.write(log.path(), b"{\"type\":\"commit\",\"txn\":\"TX1\"}\n")
        .unwrap();
    assert!(matches!(log.read(), Err(CommitLogError::Decode(_))));
}

#[test]
fn a_complete_line_with_an_invalid_transaction_id_is_malformed() {
    let (vfs, log) = fresh_log();
    vfs.write(log.path(), b"{\"type\":\"begin\",\"txn\":\"\"}\n")
        .unwrap();
    assert!(matches!(log.read(), Err(CommitLogError::Malformed(_))));
}

#[test]
fn resetting_an_empty_log_flushes_once_and_never_truncates() {
    // The first reset cannot know whether an earlier truncation's flush landed, so it
    // flushes; once that succeeds, later resets of the empty log cost nothing.
    let base = MemFs::new();
    let root = PathBuf::from("shire");
    base.create_dir_all(&root).unwrap();
    let path = root.join(LOG_NAME);
    base.write(&path, b"").unwrap();
    let policy = FaultPolicy::new()
        .inject(FaultOp::Truncate, &path, Fault::io_error(), When::Always)
        .inject(FaultOp::Fsync, &path, Fault::io_error(), When::OnCall(2));
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(base, policy));
    let log = CommitLog::new(vfs, &root);

    log.reset().expect("the first reset flushes the empty log");
    log.reset()
        .expect("an empty log known flushed needs no further sync");
}

#[test]
fn a_truncation_whose_flush_failed_is_flushed_by_the_next_reset() {
    // The failed reset leaves an empty live log that a crash would refill; the next reset
    // sees it empty and must flush it rather than call it done.
    let root = PathBuf::from("shire");
    let path = root.join(LOG_NAME);
    let base = MemFs::new();
    base.create_dir_all(&root).unwrap();
    let policy =
        FaultPolicy::new().inject(FaultOp::Fsync, &path, Fault::io_error(), When::OnCall(2));
    let faulty = Arc::new(FaultyFs::new(base, policy));
    let vfs: Arc<dyn Vfs> = faulty.clone();
    let log = CommitLog::new(Arc::clone(&vfs), &root);

    log.append(&begin(&nonce("tx1"))).unwrap();
    log.reset().expect_err("the truncation's flush fails");
    log.reset().expect("the empty log is flushed this time");

    faulty.inner().simulate_crash();
    assert!(log.read().unwrap().is_empty(), "the old record came back");
}

#[test]
fn reset_propagates_metadata_errors() {
    let root = PathBuf::from("shire");
    let path = root.join(LOG_NAME);
    let policy =
        FaultPolicy::new().inject(FaultOp::Metadata, &path, Fault::io_error(), When::Always);
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(MemFs::new(), policy));
    let log = CommitLog::new(vfs, &root);

    match log.reset() {
        Err(CommitLogError::Vfs(error)) => assert_eq!(error.kind(), VfsErrorKind::Other),
        other => panic!("expected Vfs(Other), got {other:?}"),
    }
}

#[test]
fn commit_log_errors_have_context_and_sources() {
    let vfs_error = CommitLogError::Vfs(VfsError::new(
        VfsErrorKind::Other,
        "read",
        "injected failure",
    ));
    assert_eq!(
        vfs_error.to_string(),
        "commit log filesystem error: read failed: other filesystem error: injected failure"
    );
    assert!(vfs_error.source().is_some());

    let malformed = CommitLogError::Malformed("bad record".to_owned());
    assert_eq!(
        malformed.to_string(),
        "commit log line is malformed: bad record"
    );
    assert!(malformed.source().is_none());
}

#[test]
fn a_prepare_with_an_invalid_swap_node_is_malformed() {
    // `a b` is not a valid component (space is outside the charset). Note that `a/b`
    // would now be *valid* -- a two-component path -- where before `D222` it was the
    // canonical example of a bad node name.
    let (vfs, log) = fresh_log();
    let line =
        b"{\"type\":\"prepare\",\"txn\":\"TX1\",\"swaps\":[{\"node\":\"a b\",\"document\":\"record\",\"body\":[1,2]}]}\n";
    vfs.write(log.path(), line).unwrap();
    assert!(matches!(log.read(), Err(CommitLogError::Malformed(_))));
}

#[test]
fn a_prepare_round_trips_a_multi_component_swap_node() {
    // The commit log records a swap's target as a path string, so a nested path must
    // survive the write/read round trip -- otherwise a transaction over a hierarchical
    // target could not be recovered.
    let (_vfs, log) = fresh_log();
    let txn = Nonce::new("TX1").unwrap();
    let swaps = SwapSet::single(Swap::new(
        NodePath::parse("tickets/ab/abcdef").unwrap(),
        DocumentName::new("record.json").unwrap(),
        DocumentBody::new(b"x".to_vec()),
    ));
    log.append(&LogRecord::Prepare {
        txn: txn.clone(),
        prepared: prepared(&swaps),
    })
    .unwrap();

    let records = log.read().expect("read back");
    let LogRecord::Prepare { prepared, .. } = &records[0] else {
        panic!("expected a prepare record, got {:?}", records[0]);
    };
    assert_eq!(prepared.swaps()[0].node().to_string(), "TICKETS/AB/ABCDEF");
}

#[test]
fn a_prepare_with_an_invalid_swap_document_is_malformed() {
    let (vfs, log) = fresh_log();
    let line =
        b"{\"type\":\"prepare\",\"txn\":\"TX1\",\"swaps\":[{\"node\":\"ALPHA\",\"document\":\".hidden\",\"body\":[1,2]}]}\n";
    vfs.write(log.path(), line).unwrap();
    assert!(matches!(log.read(), Err(CommitLogError::Malformed(_))));
}

#[test]
fn a_prepare_with_a_mis_sized_expected_digest_is_malformed() {
    let (vfs, log) = fresh_log();
    let line =
        b"{\"type\":\"prepare\",\"txn\":\"TX1\",\"swaps\":[{\"node\":\"ALPHA\",\"document\":\"record\",\"body\":[1],\"expected\":[1,2,3]}]}\n";
    vfs.write(log.path(), line).unwrap();
    assert!(matches!(log.read(), Err(CommitLogError::Malformed(_))));
}

/// A prepare line whose swaps are otherwise valid writes to the given documents under `ALPHA`.
fn prepare_writing(documents: &[&str]) -> String {
    let swaps: Vec<String> = documents
        .iter()
        .enumerate()
        .map(|(i, document)| {
            let digest = ContentDigest::of(&[i as u8]).to_hex();
            format!(r#"{{"node":"ALPHA","document":"{document}","digest":"{digest}"}}"#)
        })
        .collect();
    format!(
        "{{\"type\":\"prepare\",\"txn\":\"TX1\",\"swaps\":[{}]}}\n",
        swaps.join(",")
    )
}

#[test]
fn a_prepare_with_duplicate_swap_targets_is_malformed() {
    let (vfs, log) = fresh_log();
    vfs.write(
        log.path(),
        prepare_writing(&["record", "record"]).as_bytes(),
    )
    .unwrap();
    match log.read() {
        Err(CommitLogError::Malformed(message)) => assert!(
            message.contains("names ALPHA/RECORD twice"),
            "rejected for another reason: {message}"
        ),
        other => panic!("a duplicate target must be malformed, got {other:?}"),
    }
}

#[test]
fn a_prepare_with_distinct_swap_targets_reads() {
    let (vfs, log) = fresh_log();
    vfs.write(log.path(), prepare_writing(&["record", "other"]).as_bytes())
        .unwrap();
    let records = log
        .read()
        .expect("the same line with distinct targets is valid");
    assert!(
        matches!(&records[..], [LogRecord::Prepare { prepared, .. }] if prepared.swaps().len() == 2)
    );
}

#[test]
fn an_empty_body_swap_round_trips() {
    let (_vfs, log) = fresh_log();
    let record = LogRecord::Prepare {
        txn: nonce("tx1"),
        prepared: prepared(&SwapSet::single(Swap::new(
            name("alpha"),
            doc("record"),
            body(b""),
        ))),
    };
    log.append(&record).unwrap();
    assert_eq!(log.read().unwrap(), vec![record]);
}

#[test]
fn each_record_is_a_single_ndjson_line() {
    let (vfs, log) = fresh_log();
    log.append(&begin(&nonce("tx1"))).unwrap();
    log.append(&LogRecord::Prepare {
        txn: nonce("tx1"),
        prepared: prepared(&sample_swaps()),
    })
    .unwrap();
    log.append(&LogRecord::End { txn: nonce("tx1") }).unwrap();
    let raw = vfs.read(log.path()).unwrap();
    assert_eq!(raw.last(), Some(&b'\n'), "the log ends with a newline");
    assert_eq!(
        raw.iter().filter(|&&b| b == b'\n').count(),
        3,
        "exactly one newline per record"
    );
}

#[test]
fn the_transaction_id_accessor_names_each_record() {
    assert_eq!(begin(&nonce("tx1")).txn(), &nonce("tx1"));
    assert_eq!(
        LogRecord::Prepare {
            txn: nonce("tx2"),
            prepared: prepared(&SwapSet::new([]).unwrap()),
        }
        .txn(),
        &nonce("tx2")
    );
    assert_eq!(LogRecord::End { txn: nonce("tx3") }.txn(), &nonce("tx3"));
}
