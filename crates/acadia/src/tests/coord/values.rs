// Copyright (c) Michael Grier.

//! The values the interface speaks in.

use std::error::Error;

use crate::coord::{
    Acquired, CoordError, CreationNonce, Generation, ImplementationId, ModCount, Registration,
    RegistryToken, Trust, Untrusted,
};
use crate::{VfsError, VfsErrorKind};

#[test]
fn an_implementation_identity_is_short_printable_ascii() {
    for good in [
        "a",
        "acadia-file-1",
        "x.y_z/2",
        &"k".repeat(ImplementationId::MAX_LEN),
    ] {
        let id = ImplementationId::new(good).unwrap_or_else(|| panic!("{good}"));
        assert_eq!(id.as_str(), good);
        assert_eq!(id.to_string(), good);
    }
    for bad in [
        "",
        "has space",
        "tab\t",
        "caf\u{e9}",
        "line\n",
        &"k".repeat(ImplementationId::MAX_LEN + 1),
    ] {
        assert!(ImplementationId::new(bad).is_none(), "{bad:?}");
    }
}

#[test]
fn an_identity_read_back_equals_the_constant_it_was_written_from() {
    const ID: ImplementationId = ImplementationId::from_static("acadia-file-1");
    assert_eq!(ImplementationId::new(ID.as_str()), Some(ID));
}

#[test]
fn a_nonce_keeps_its_bytes() {
    let bytes = *b"0123456789abcdef";
    assert_eq!(CreationNonce::from_bytes(bytes).to_bytes(), bytes);
    assert_ne!(
        CreationNonce::from_bytes(bytes),
        CreationNonce::from_bytes([0; 16])
    );
}

#[test]
fn an_even_count_is_settled_and_an_odd_one_is_not() {
    for (value, settled) in [(0, true), (1, false), (2, true), (u64::MAX, false)] {
        assert_eq!(ModCount::new(value).is_settled(), settled, "{value}");
        assert_eq!(ModCount::new(value).get(), value);
    }
}

#[test]
fn a_registration_names_its_token() {
    let token = RegistryToken::new(u128::MAX - 1);
    assert_eq!(token.get(), u128::MAX - 1);
    assert_ne!(Registration::Registered(token), Registration::Unregistered);
    assert_ne!(token, RegistryToken::new(0));
}

#[test]
fn generations_order_by_value() {
    assert!(Generation::new(1) < Generation::new(2));
    assert_eq!(Generation::new(7).get(), 7);
}

#[test]
fn recovery_is_required_by_a_fence_or_by_untrusted_state() {
    let acquired = |fenced, trust| {
        Acquired::new(
            Generation::new(3),
            Generation::new(4),
            fenced,
            fenced,
            trust,
        )
    };
    assert!(!acquired(false, Trust::Vouched).recovery_required());
    assert!(acquired(true, Trust::Vouched).recovery_required());
    for why in [
        Untrusted::HostRestarted,
        Untrusted::Created,
        Untrusted::CannotTell,
    ] {
        assert!(acquired(false, Trust::Untrusted(why)).recovery_required());
    }
    let found = acquired(false, Trust::Vouched);
    assert_eq!(found.found(), Generation::new(3));
    assert_eq!(found.advanced_to(), Generation::new(4));
}

#[test]
fn a_filesystem_failure_is_the_source_of_a_coordination_failure() {
    let error = CoordError::from(VfsError::new(VfsErrorKind::Other, "read", "broken"));
    assert!(error.source().is_some());
    assert!(error.to_string().contains("broken"));
    assert!(CoordError::Abandoned.source().is_none());
}

#[test]
fn every_refusal_says_what_it_refused() {
    let refusals = [
        CoordError::Abandoned,
        CoordError::WouldWait,
        CoordError::ImplementationMismatch {
            recorded: ImplementationId::from_static("other"),
        },
        CoordError::Occupied {
            name: "stray".into(),
        },
        CoordError::Reserved,
        CoordError::AlreadyCreated,
        CoordError::NotCreated,
        CoordError::RecoveryRequired,
        CoordError::NoModificationOpen,
        CoordError::ModificationOpen,
        CoordError::Malformed { what: "count" },
    ];
    let messages: Vec<_> = refusals.iter().map(ToString::to_string).collect();
    for (i, message) in messages.iter().enumerate() {
        assert!(!message.is_empty());
        assert!(!messages[i + 1..].contains(message), "{message}");
    }
    assert!(messages[2].contains("other"));
    assert!(messages[3].contains("stray"));
}
