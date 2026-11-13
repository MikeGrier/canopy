// Copyright (c) Michael Grier.

//! `NodePath` validation, normalization, and navigation (`BG-1`, `D222`).
//!
//! canopy is a hierarchical store, so a path is the addressing primitive: these tests
//! pin what is accepted, what is refused and why, and the navigation operations the
//! store and its callers build on.

use super::*;

/// Builds a path or panics, for the many cases where construction is not what is
/// under test.
fn path(raw: &str) -> NodePath {
    NodePath::parse(raw).expect("valid test path")
}

// -- Acceptance and normalization ----------------------------------------------

#[test]
fn parses_a_single_component() {
    let p = path("TICKETS");
    assert_eq!(p.depth(), 1);
    assert_eq!(p.name(), "TICKETS");
    assert_eq!(p.to_string(), "TICKETS");
    assert!(p.parent().is_none());
}

#[test]
fn parses_a_multi_component_path() {
    let p = path("TICKETS/AB/ABCDEF0123456");
    assert_eq!(p.depth(), 3);
    assert_eq!(
        p.components().collect::<Vec<_>>(),
        ["TICKETS", "AB", "ABCDEF0123456"]
    );
    assert_eq!(p.name(), "ABCDEF0123456");
    assert_eq!(p.to_string(), "TICKETS/AB/ABCDEF0123456");
}

#[test]
fn normalizes_each_component_to_uppercase() {
    // Folding is what makes one name render to one on-disk entry on both
    // case-sensitive and case-insensitive filesystems, so it must apply per component
    // rather than only to the last.
    let p = path("tickets/ab/abcdef");
    assert_eq!(p.to_string(), "TICKETS/AB/ABCDEF");
    assert_eq!(p.name(), "ABCDEF");
}

#[test]
fn accepts_the_full_component_charset() {
    let p = path("a-b_c/D9-_/0123456789");
    assert_eq!(p.to_string(), "A-B_C/D9-_/0123456789");
}

#[test]
fn a_parsed_path_and_a_built_path_agree() {
    assert_eq!(
        NodePath::parse("TICKETS/AB").expect("parse"),
        NodePath::new(["tickets", "ab"]).expect("new")
    );
}

#[test]
fn new_validates_its_components_too() {
    // The two constructors must not diverge: a caller assembling from pieces has the
    // same opportunity to supply a bad component as one writing the path out.
    assert!(matches!(
        NodePath::new(["TICKETS", "a/b"]),
        Err(NodePathError::InvalidCharacter { index: 1, .. })
    ));
    assert!(matches!(
        NodePath::new(["TICKETS", ""]),
        Err(NodePathError::EmptyComponent { index: 1 })
    ));
}

#[test]
fn an_empty_component_list_is_refused() {
    let empty: [&str; 0] = [];
    assert_eq!(NodePath::new(empty), Err(NodePathError::Empty));
}

// -- Refusals ------------------------------------------------------------------

#[test]
fn the_empty_path_is_refused() {
    assert_eq!(NodePath::parse(""), Err(NodePathError::Empty));
}

#[test]
fn leading_trailing_and_doubled_separators_are_refused() {
    // Each is an empty component, and the index says which one, so a caller learns
    // where the stray separator is rather than only that there was one.
    assert!(matches!(
        NodePath::parse("/TICKETS"),
        Err(NodePathError::EmptyComponent { index: 0 })
    ));
    assert!(matches!(
        NodePath::parse("TICKETS/"),
        Err(NodePathError::EmptyComponent { index: 1 })
    ));
    assert!(matches!(
        NodePath::parse("TICKETS//AB"),
        Err(NodePathError::EmptyComponent { index: 1 })
    ));
    assert!(matches!(
        NodePath::parse("/"),
        Err(NodePathError::EmptyComponent { index: 0 })
    ));
}

#[test]
fn dot_and_dotdot_are_refused_as_relative_rather_than_as_bad_characters() {
    // `.` is already outside the charset, so this would be caught either way. It is
    // reported as its own error because a caller who writes `../SIBLING` has assumed
    // traversal exists, and an "invalid character" message would send them hunting a
    // quoting bug instead.
    match NodePath::parse("TICKETS/../SHIRE") {
        Err(NodePathError::RelativeComponent { index, component }) => {
            assert_eq!(index, 1);
            assert_eq!(component, "..");
        }
        other => panic!("expected a relative-component rejection, got {other:?}"),
    }
    assert!(matches!(
        NodePath::parse("./TICKETS"),
        Err(NodePathError::RelativeComponent { index: 0, .. })
    ));
}

#[test]
fn an_interior_dot_is_refused_in_a_component() {
    // A document name may carry an extension; a directory component may not, so the
    // two namespaces stay distinguishable on disk.
    assert!(matches!(
        NodePath::parse("TICKETS/record.json"),
        Err(NodePathError::InvalidCharacter {
            index: 1,
            character: '.',
            ..
        })
    ));
}

#[test]
fn characters_outside_the_set_are_refused_with_their_position() {
    for (raw, index, character) in [
        ("TICKETS/a b", 1, ' '),
        ("TICK ETS/ab", 0, ' '),
        ("TICKETS/a\\b", 1, '\\'),
        ("TICKETS/a:b", 1, ':'),
        ("TICKETS/\u{e9}", 1, '\u{e9}'),
    ] {
        match NodePath::parse(raw) {
            Err(NodePathError::InvalidCharacter {
                index: got_index,
                character: got_char,
                ..
            }) => {
                assert_eq!(got_index, index, "{raw}");
                assert_eq!(got_char, character, "{raw}");
            }
            other => panic!("expected an invalid character for {raw}, got {other:?}"),
        }
    }
}

#[test]
fn a_reserved_device_name_is_refused_at_any_depth() {
    // A reserved stem is unusable as a directory anywhere in the path, not only at
    // the leaf, so the check runs per component.
    assert!(matches!(
        NodePath::parse("CON"),
        Err(NodePathError::ReservedComponent { index: 0 })
    ));
    assert!(matches!(
        NodePath::parse("TICKETS/NUL/AB"),
        Err(NodePathError::ReservedComponent { index: 1 })
    ));
    assert!(matches!(
        NodePath::parse("TICKETS/lpt9"),
        Err(NodePathError::ReservedComponent { index: 1 })
    ));
}

// -- Bounds --------------------------------------------------------------------

#[test]
fn a_component_at_the_length_limit_is_accepted() {
    let at_limit = "A".repeat(NodePath::MAX_COMPONENT_LEN);
    let p = NodePath::parse(&at_limit).expect("a component at the limit is allowed");
    assert_eq!(p.name().len(), NodePath::MAX_COMPONENT_LEN);
}

#[test]
fn a_component_past_the_length_limit_is_refused() {
    let past = format!("TICKETS/{}", "A".repeat(NodePath::MAX_COMPONENT_LEN + 1));
    assert_eq!(
        NodePath::parse(&past),
        Err(NodePathError::ComponentTooLong {
            index: 1,
            len: NodePath::MAX_COMPONENT_LEN + 1,
            max: NodePath::MAX_COMPONENT_LEN,
        })
    );
}

#[test]
fn a_sha256_hex_fits_a_component_exactly() {
    // A hash-derived identifier is a SHA-256 rendered as hex, at exactly the
    // component limit. Pinned so nobody lowers the limit without meeting this.
    let hex = "A".repeat(64);
    assert_eq!(hex.len(), NodePath::MAX_COMPONENT_LEN);
    assert!(NodePath::parse(&hex).is_ok());
}

#[test]
fn a_path_at_the_depth_limit_is_accepted() {
    let components: Vec<String> = (0..NodePath::MAX_DEPTH).map(|i| format!("C{i}")).collect();
    let p = NodePath::new(&components).expect("a path at the depth limit is allowed");
    assert_eq!(p.depth(), NodePath::MAX_DEPTH);
}

#[test]
fn a_path_past_the_depth_limit_is_refused() {
    let components: Vec<String> = (0..NodePath::MAX_DEPTH + 1)
        .map(|i| format!("C{i}"))
        .collect();
    assert_eq!(
        NodePath::new(&components),
        Err(NodePathError::TooDeep {
            depth: NodePath::MAX_DEPTH + 1,
            max: NodePath::MAX_DEPTH,
        })
    );
}

#[test]
fn a_path_past_the_total_length_limit_is_refused() {
    // Every component is legal and the depth is legal; only the rendered whole is too
    // long. This is the bound that protects the on-disk path, and it is the one a
    // per-component limit alone would miss.
    let components: Vec<String> = (0..NodePath::MAX_DEPTH)
        .map(|_| "A".repeat(NodePath::MAX_COMPONENT_LEN))
        .collect();
    match NodePath::new(&components) {
        Err(NodePathError::TooLong { len, max }) => {
            assert_eq!(max, NodePath::MAX_LEN);
            assert!(len > NodePath::MAX_LEN);
        }
        other => panic!("expected a total-length rejection, got {other:?}"),
    }
}

#[test]
fn the_length_bound_counts_the_separators_and_is_exact_at_the_boundary() {
    // Off-by-one guard on the rendered length: components plus one separator between
    // each adjacent pair. Built to sit at exactly MAX_LEN within the depth limit, so
    // one character more must be the difference between accepted and refused -- which
    // a test merely "under the limit" would not catch.
    let depth = NodePath::MAX_DEPTH;
    let separators = depth - 1;
    let budget = NodePath::MAX_LEN - separators;
    let base = budget / depth;
    let remainder = budget % depth;

    let mut components: Vec<String> = (0..depth).map(|_| "A".repeat(base)).collect();
    components[0].push_str(&"A".repeat(remainder));

    let at_limit = NodePath::new(&components).expect("a path at exactly the limit is allowed");
    assert_eq!(at_limit.to_string().len(), NodePath::MAX_LEN);

    components[0].push('A');
    assert_eq!(
        NodePath::new(&components),
        Err(NodePathError::TooLong {
            len: NodePath::MAX_LEN + 1,
            max: NodePath::MAX_LEN,
        })
    );
}

// -- Navigation ----------------------------------------------------------------

#[test]
fn parent_walks_up_one_level_at_a_time() {
    let p = path("TICKETS/AB/ABCDEF");
    let parent = p.parent().expect("has a parent");
    assert_eq!(parent.to_string(), "TICKETS/AB");
    let grandparent = parent.parent().expect("has a parent");
    assert_eq!(grandparent.to_string(), "TICKETS");
    assert!(grandparent.parent().is_none());
}

#[test]
fn a_root_level_node_has_no_parent() {
    // Not an empty path: the store root is not itself a node, so there is nothing for
    // an empty path to name.
    assert!(path("TICKETS").parent().is_none());
}

#[test]
fn child_extends_a_path_by_one_component() {
    let p = path("TICKETS").child("ab").expect("valid child");
    assert_eq!(p.to_string(), "TICKETS/AB");
    assert_eq!(p.depth(), 2);
}

#[test]
fn child_validates_the_component_and_the_resulting_bounds() {
    assert!(matches!(
        path("TICKETS").child("a/b"),
        Err(NodePathError::InvalidCharacter { .. })
    ));
    assert!(matches!(
        path("TICKETS").child(""),
        Err(NodePathError::EmptyComponent { .. })
    ));

    // A child that would breach the depth limit is refused by the same bound the
    // constructors apply, rather than silently exceeding it.
    let components: Vec<String> = (0..NodePath::MAX_DEPTH).map(|i| format!("C{i}")).collect();
    let deepest = NodePath::new(&components).expect("at the depth limit");
    assert!(matches!(
        deepest.child("MORE"),
        Err(NodePathError::TooDeep { .. })
    ));
}

#[test]
fn child_and_parent_round_trip() {
    let parent = path("TICKETS/AB");
    let child = parent.child("ABCDEF").expect("valid child");
    assert_eq!(child.parent().as_ref(), Some(&parent));
}

#[test]
fn contains_is_component_wise_not_a_string_prefix() {
    let tickets = path("TICKETS");
    assert!(tickets.contains(&path("TICKETS")));
    assert!(tickets.contains(&path("TICKETS/AB")));
    assert!(tickets.contains(&path("TICKETS/AB/ABCDEF")));

    // The trap a rendered-string `starts_with` would fall into.
    assert!(!tickets.contains(&path("TICKETSX")));
    assert!(!tickets.contains(&path("TICKETSX/AB")));

    assert!(!path("TICKETS/AB").contains(&tickets));
    assert!(!path("SHIRE").contains(&path("TICKETS")));
}

#[test]
fn relative_path_is_one_directory_per_component() {
    let p = path("TICKETS/AB/ABCDEF");
    let expected: std::path::PathBuf = ["TICKETS", "AB", "ABCDEF"].iter().collect();
    assert_eq!(p.relative_path(), expected);
}

// -- Ordering ------------------------------------------------------------------

#[test]
fn ordering_is_component_wise_so_a_sort_groups_siblings_under_their_parent() {
    let mut paths = [
        path("TICKETS/AB/B"),
        path("SHIRE"),
        path("TICKETS"),
        path("TICKETSX"),
        path("TICKETS/AB"),
        path("TICKETS/AB/A"),
        path("TICKETS/AA"),
    ];
    paths.sort();
    let rendered: Vec<String> = paths.iter().map(NodePath::to_string).collect();
    assert_eq!(
        rendered,
        [
            "SHIRE",
            "TICKETS",
            "TICKETS/AA",
            "TICKETS/AB",
            "TICKETS/AB/A",
            "TICKETS/AB/B",
            "TICKETSX",
        ]
    );
}

// -- Borrowed conversion -------------------------------------------------------

#[test]
fn a_borrowed_path_satisfies_an_into_bound() {
    // `From<&NodePath>` is what lets the store's `impl Into<NodePath>` signatures take
    // a borrow, so a caller need not choose between two spellings or write the clone.
    // `BG-4` removed the `NodeName` conversions this sat beside; this one survives them.
    fn takes(path: impl Into<NodePath>) -> NodePath {
        path.into()
    }

    let original = path("TICKETS/AB");
    assert_eq!(takes(&original), original);
    assert_eq!(takes(original.clone()), original);
}

// -- Display -------------------------------------------------------------------

#[test]
fn error_display_text_names_the_offending_component() {
    assert_eq!(NodePathError::Empty.to_string(), "node path is empty");
    assert_eq!(
        NodePathError::EmptyComponent { index: 2 }.to_string(),
        "node path component 2 is empty (a leading, trailing, or doubled '/')"
    );
    assert!(
        NodePathError::RelativeComponent {
            index: 1,
            component: "..".to_owned(),
        }
        .to_string()
        .contains("no traversal")
    );
    assert!(
        NodePathError::TooDeep { depth: 17, max: 16 }
            .to_string()
            .contains("17 components deep")
    );
}
