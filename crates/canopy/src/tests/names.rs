// Copyright (c) Michael Grier.
// Split from tests.rs at 2a4dd8f.

//! Validation and normalization of the leaf value types: `DocumentName` (LAY-2) and
//! `Nonce` (LAY-3).
//!
//! Path-component validation moved to [`node_path`](super::node_path) with `D222`,
//! which retired the flat node-name type and its shard-key derivation.

use super::*;

#[test]
fn value_types_and_validation_errors_have_stable_display_text() {
    assert_eq!(
        DocumentName::new("record.json").unwrap().to_string(),
        "RECORD.JSON"
    );
    assert_eq!(
        DocumentNameError::Empty.to_string(),
        "document name is empty"
    );
    assert_eq!(
        Nonce::new("transaction-1").unwrap().to_string(),
        "TRANSACTION-1"
    );
    assert_eq!(NonceError::Empty.to_string(), "nonce is empty");
}

// -- DocumentName: validation & normalization (LAY-2) --------------------------

#[test]
fn document_name_accepts_and_folds_a_simple_name() {
    assert_eq!(DocumentName::new("engine").unwrap().as_str(), "ENGINE");
    assert_eq!(
        DocumentName::new("Record-1_v2").unwrap().as_str(),
        "RECORD-1_V2"
    );
}

#[test]
fn document_name_accepts_an_interior_dot() {
    assert_eq!(
        DocumentName::new("record.json").unwrap().as_str(),
        "RECORD.JSON"
    );
}

#[test]
fn document_name_rejects_the_empty_string() {
    assert_eq!(DocumentName::new(""), Err(DocumentNameError::Empty));
}

#[test]
fn document_name_rejects_a_leading_dot() {
    assert_eq!(
        DocumentName::new(".shire"),
        Err(DocumentNameError::LeadingDot)
    );
}

#[test]
fn document_name_rejects_a_trailing_dot() {
    assert_eq!(
        DocumentName::new("record."),
        Err(DocumentNameError::TrailingDot)
    );
}

#[test]
fn document_name_rejects_path_separators() {
    assert!(matches!(
        DocumentName::new("a/b"),
        Err(DocumentNameError::InvalidCharacter { character: '/', .. })
    ));
    assert!(matches!(
        DocumentName::new("a\\b"),
        Err(DocumentNameError::InvalidCharacter {
            character: '\\',
            ..
        })
    ));
}

#[test]
fn document_name_rejects_a_non_ascii_character_with_its_offset() {
    assert!(matches!(
        DocumentName::new("re\u{00e9}cord"),
        Err(DocumentNameError::InvalidCharacter { byte_offset: 2, .. })
    ));
}

#[test]
fn document_name_rejects_a_reserved_stem() {
    assert_eq!(DocumentName::new("con"), Err(DocumentNameError::Reserved));
    assert_eq!(
        DocumentName::new("nul.json"),
        Err(DocumentNameError::Reserved)
    );
}

#[test]
fn document_name_accepts_a_non_reserved_lookalike() {
    assert_eq!(
        DocumentName::new("console.json").unwrap().as_str(),
        "CONSOLE.JSON"
    );
}

#[test]
fn document_name_enforces_the_length_limit() {
    let at_limit = "a".repeat(DocumentName::MAX_LEN);
    assert_eq!(
        DocumentName::new(&at_limit).unwrap().as_str().len(),
        DocumentName::MAX_LEN
    );
    let past_limit = "a".repeat(DocumentName::MAX_LEN + 1);
    assert!(matches!(
        DocumentName::new(&past_limit),
        Err(DocumentNameError::TooLong { .. })
    ));
}

// -- Nonce: validation & normalization (LAY-3) ---------------------------------

#[test]
fn nonce_folds_letters_to_uppercase() {
    assert_eq!(nonce("txn-abc").as_str(), "TXN-ABC");
}

#[test]
fn nonce_accepts_digits_and_separators() {
    assert_eq!(nonce("TXN_00-19a").as_str(), "TXN_00-19A");
}

#[test]
fn nonce_rejects_the_empty_token() {
    assert_eq!(Nonce::new(""), Err(NonceError::Empty));
}

#[test]
fn nonce_rejects_a_dot() {
    assert_eq!(
        Nonce::new("txn.1"),
        Err(NonceError::InvalidCharacter {
            character: '.',
            byte_offset: 3,
        })
    );
}

#[test]
fn nonce_rejects_a_path_separator() {
    assert_eq!(
        Nonce::new("txn/1"),
        Err(NonceError::InvalidCharacter {
            character: '/',
            byte_offset: 3,
        })
    );
}

#[test]
fn nonce_rejects_whitespace() {
    assert_eq!(
        Nonce::new("txn 1"),
        Err(NonceError::InvalidCharacter {
            character: ' ',
            byte_offset: 3,
        })
    );
}

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

#[test]
fn a_reserved_device_name_is_a_valid_nonce_and_keeps_its_logical_value() {
    for raw in ["con", "Nul", "COM1", "lpt9"] {
        let nonce = Nonce::new(raw).expect("a reserved name is still a nonce");
        assert_eq!(nonce.as_str(), raw.to_ascii_uppercase());
    }
}

#[test]
fn con_and_its_substitute_are_distinct() {
    let substitute: String = std::iter::once(crate::name::RESERVED_NONCE_SUBSTITUTE)
        .chain("ON".chars())
        .collect();
    let con = Nonce::new("con").unwrap();

    // The substitute is not a nonce at all, so no caller can mint one that collides with it.
    assert!(matches!(
        Nonce::new(&substitute),
        Err(NonceError::InvalidCharacter { byte_offset: 0, .. })
    ));
    assert_ne!(con.as_str(), substitute);
    assert_eq!(
        con.as_str(),
        "CON",
        "the logical nonce is never substituted"
    );

    if cfg!(windows) {
        assert_eq!(con.path_component(), substitute);
    } else {
        assert_eq!(con.path_component(), "CON");
    }
    assert_ne!(
        con.path_component(),
        Nonce::new("ON").unwrap().path_component()
    );
}

#[test]
fn every_reserved_substitute_differs_from_every_valid_nonce() {
    // A substitute begins with a character no nonce may contain, so it can never equal the
    // path component of any nonce that is not itself the reserved name it stands for.
    for raw in RESERVED {
        let component = Nonce::new(raw).unwrap().path_component().into_owned();
        assert!(Nonce::new(&component).is_ok() != cfg!(windows), "{raw}");
        for other in ["ON", "OM1", "RN", "UX", "UL", "PT9", "CONSOLE", "TXN-1"] {
            assert_ne!(
                component,
                Nonce::new(other).unwrap().path_component(),
                "{raw}/{other}"
            );
        }
    }
}

#[test]
fn only_exact_reserved_names_are_substituted_in_path_components() {
    for raw in [
        "CONSOLE",
        "CONSOLIDATE",
        "COM10",
        "LPT0",
        "NULL",
        "AUXILIARY",
        "TXN-1",
    ] {
        let nonce = Nonce::new(raw).unwrap();
        assert_eq!(nonce.path_component(), nonce.as_str(), "{raw}");
    }
}

#[test]
fn reserved_nonces_take_a_distinct_same_length_substitute_on_windows_only() {
    let mut seen = std::collections::HashSet::new();
    for raw in RESERVED {
        let nonce = Nonce::new(raw).unwrap();
        let component = nonce.path_component();
        if cfg!(windows) {
            assert_ne!(component, raw);
            assert_eq!(component.chars().count(), raw.chars().count());
            assert!(component.starts_with(crate::name::RESERVED_NONCE_SUBSTITUTE));
            assert_eq!(
                &component[crate::name::RESERVED_NONCE_SUBSTITUTE.len_utf8()..],
                &raw[1..]
            );
            // Never a valid nonce, so it cannot collide with one.
            assert!(Nonce::new(&component).is_err());
        } else {
            assert_eq!(component, raw);
        }
        assert!(seen.insert(component.into_owned()), "{raw} collides");
    }
}

#[test]
fn nonce_rejects_an_over_long_token() {
    let raw = "a".repeat(Nonce::MAX_LEN + 1);
    assert_eq!(
        Nonce::new(&raw),
        Err(NonceError::TooLong {
            len: Nonce::MAX_LEN + 1,
            max: Nonce::MAX_LEN,
        })
    );
}
