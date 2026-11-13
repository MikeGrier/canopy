// Copyright (c) Michael Grier.

use super::*;

#[test]
fn transaction_grouping_preserves_each_first_seen_transaction_state() {
    let first = Nonce::new("first").unwrap();
    let second = Nonce::new("second").unwrap();
    let swap = Swap::new(
        NodePath::parse("record").unwrap(),
        DocumentName::new("record.json").unwrap(),
        DocumentBody::new(b"value".to_vec()),
    );
    let digest = swap.body().expect("a write carries a body").digest();
    let prepared = PreparedSet::new(vec![PreparedSwap::writing(&swap, digest)], Vec::new());
    let records = [
        LogRecord::Begin {
            txn: first.clone(),
            targets: Vec::new(),
        },
        LogRecord::Prepare {
            txn: second.clone(),
            prepared,
        },
        LogRecord::Begin {
            txn: second.clone(),
            targets: Vec::new(),
        },
        LogRecord::End { txn: first.clone() },
    ];

    let grouped = group_by_transaction(&records);
    assert_eq!(grouped.len(), 2);
    assert_eq!(grouped[0].id, first);
    assert!(grouped[0].prepared.is_none());
    assert!(grouped[0].ended);
    assert_eq!(grouped[1].id, second);
    assert!(grouped[1].prepared.is_some());
    assert!(!grouped[1].ended);
}
