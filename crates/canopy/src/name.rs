// Copyright (c) Michael Grier.

//! Name value types for the canopy namespace (D40/D54/D58/D222).
//!
//! canopy does **not** mint names, and derives no structure from them: the caller
//! supplies every path, and canopy validates and normalizes what it is given (D54).
//! canopy neither knows nor cares why those names happen to be uniformly distributed --
//! that uniformity is the caller's contribution (for example keyed-permutation IDs), and
//! D222 retired the shard tier canopy used to synthesize from a name's
//! leading characters, which was the one place the store relied on a property it
//! declined to know about.
//!
//! A [`NodePath`] is a validated sequence of components addressing one node --
//! `TICKETS/AB/ABCDEF0123456` -- and is how the hierarchy D40 and D54 specified is
//! expressed. Its [`relative_path`](NodePath::relative_path) maps it onto the on-disk
//! directory hierarchy within a partition subtree, one directory per component.
//!
//! A path's components are validated individually, and a one-component path is what the
//! pre-D222 flat namespace called a node name; that type is gone, and so is the shard
//! prefix it derived.
//!
//! A [`DocumentName`] names one of the caller-supplied documents a node directory
//! holds (D40: e.g. a record document and an engine-blob document at the same node).
//! It is validated and normalized on the same principles as a path component, but --
//! being a file leaf rather than a directory -- it additionally permits an interior `.`
//! so callers may keep an extension (`record.json`).
//!
//! A [`Nonce`] is a caller-supplied token naming one durable write's staging temps
//! (D40/D53). canopy mints no nonce of its own -- it has no RNG (D61) -- so the caller
//! passes a value unique to each write; the eventual D53 commit path supplies the
//! transaction id. It is validated as a safe path fragment so it can be embedded in a
//! `<document>.tmp.<nonce>` temp file name.
//!
//! A [`Nonce`] is a caller-supplied token naming one durable write's staging temps
//! (D40/D53). canopy mints no nonce of its own -- it has no RNG (D61) -- so the caller
//! passes a value unique to each write; the eventual D53 commit path supplies the
//! transaction id. It is validated as a safe path fragment so it can be embedded in a
//! `<document>.tmp.<nonce>` temp file name.

use std::fmt;
use std::path::PathBuf;

impl From<&NodePath> for NodePath {
    /// Lets a `&NodePath` satisfy an `impl Into<NodePath>` bound, so a caller may pass
    /// a borrow where the store takes a path by value without spelling the clone.
    fn from(path: &NodePath) -> NodePath {
        path.clone()
    }
}
/// A validated, normalized document name: one file leaf within a node directory (D40).
///
/// Construct one with [`DocumentName::new`]. A node directory holds one or more of
/// these -- caller-chosen document names canopy stores but never interprets (D54).
/// Validation mirrors a path component (ASCII letters folded to uppercase, digits, `-`,
/// `_`, bounded length, no reserved device stems) with one addition: an interior `.`
/// is accepted so a caller may carry a conventional extension. A leading or trailing
/// `.` is rejected -- a leading dot would collide with canopy's own dotfiles
/// (`.shire`, staging temps) and a trailing dot is silently stripped on Windows, so
/// both would break the "one name, one on-disk entry" guarantee.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocumentName {
    canonical: String,
}

impl DocumentName {
    /// Longest document name canopy accepts, in characters (also its byte length,
    /// since the accepted set is ASCII).
    ///
    /// Shares [`NodePath::MAX_COMPONENT_LEN`]'s value and rationale (`D222`): one
    /// limit for every name canopy validates, sized so a SHA-256 rendered as hex
    /// fits **exactly**. Lowering it would silently exclude that, so check what a
    /// smaller value would break before changing it.
    pub const MAX_LEN: usize = NodePath::MAX_COMPONENT_LEN;

    /// Validates and normalizes `raw` into a canonical document name.
    ///
    /// The accepted character set is ASCII letters (folded to uppercase), ASCII
    /// digits, the separators `-` and `_`, and an interior `.`. The empty name, a
    /// leading or trailing `.`, any other character (path separators, whitespace,
    /// control bytes, non-ASCII), an over-long name, and a Windows reserved device
    /// stem are rejected.
    pub fn new(raw: &str) -> Result<DocumentName, DocumentNameError> {
        if raw.is_empty() {
            return Err(DocumentNameError::Empty);
        }
        if raw.starts_with('.') {
            return Err(DocumentNameError::LeadingDot);
        }
        if raw.ends_with('.') {
            return Err(DocumentNameError::TrailingDot);
        }
        let mut canonical = String::with_capacity(raw.len());
        for (byte_offset, ch) in raw.char_indices() {
            let normalized = match ch {
                'a'..='z' => ch.to_ascii_uppercase(),
                'A'..='Z' | '0'..='9' | '-' | '_' | '.' => ch,
                _ => {
                    return Err(DocumentNameError::InvalidCharacter {
                        character: ch,
                        byte_offset,
                    });
                }
            };
            canonical.push(normalized);
        }
        if canonical.len() > Self::MAX_LEN {
            return Err(DocumentNameError::TooLong {
                len: canonical.len(),
                max: Self::MAX_LEN,
            });
        }
        let stem = canonical.split('.').next().unwrap_or(&canonical);
        if is_reserved_name(stem) {
            return Err(DocumentNameError::Reserved);
        }
        Ok(DocumentName { canonical })
    }

    /// The canonical (normalized) document name.
    pub fn as_str(&self) -> &str {
        &self.canonical
    }
}

impl fmt::Display for DocumentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical)
    }
}

/// What replaces the first character of a nonce that is exactly a Windows reserved device
/// name, where it forms a whole path component on Windows: U+00A9, the copyright sign.
///
/// Outside the nonce character set, so a substituted name never equals a real nonce, and
/// without a Unicode decomposition, so normalizing filesystems store it unchanged. Changing
/// this value is a breaking change to the on-disk layout on Windows.
pub(crate) const RESERVED_NONCE_SUBSTITUTE: char = '\u{00A9}';

/// Reports whether `name` (already uppercased) is a Windows reserved device name.
///
/// Because `.` is not an accepted character, only the bare stems can collide.
fn is_reserved_name(name: &str) -> bool {
    matches!(
        name,
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

/// A validated path in the canopy namespace: an ordered sequence of components (`D222`).
///
/// canopy is a hierarchical store, and a path is how a caller addresses a node in it --
/// `TICKETS/AB/ABCDEF0123456` names a node three levels down, exactly as a filesystem
/// path names a directory. canopy validates and stores the structure it is **given**; it
/// derives no structure of its own from a name (`D222` retired the shard tier that used
/// to be synthesized from a name's leading characters, because that only existed to
/// compensate for having no hierarchy at all).
///
/// # Bounds
///
/// Three limits, each with a distinct job:
///
/// * [`MAX_COMPONENT_LEN`](NodePath::MAX_COMPONENT_LEN) (64) bounds one component, and is
///   shared with [`DocumentName::MAX_LEN`] so every name canopy validates obeys one rule.
/// * [`MAX_DEPTH`](NodePath::MAX_DEPTH) (16) bounds how deeply a hierarchy may nest.
/// * [`MAX_LEN`](NodePath::MAX_LEN) (512) bounds the rendered path, separators included.
///
/// The last matters because the on-disk path is the store root, plus a partition
/// directory, plus this path, plus a document name -- and the classic Windows limit is
/// 260 characters for the whole thing. canopy cannot see the store root when it validates
/// a path, so it bounds what it *can* see and leaves the caller enough budget: a path at
/// this limit still fits under a store root of moderate length on a long-path-enabled
/// system, and any realistic path is far shorter.
///
/// # Ordering
///
/// [`Ord`] is component-wise, so sorting a set of paths groups siblings under their
/// parent and orders parents before children. That is what makes an enumeration's output
/// a stable, walk-order-independent sequence.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodePath {
    components: Vec<String>,
}

impl NodePath {
    /// The separator between components in a rendered path.
    pub const SEPARATOR: char = '/';

    /// Longest single component, in characters (also its byte length, since the
    /// accepted set is ASCII).
    ///
    /// 64 rather than a filesystem's own maximum: a component is one tier of a path
    /// whose *total* length is what actually has to fit, so a generous per-component
    /// limit would buy nothing and cost depth. It is sized to admit hash-derived
    /// identifiers -- a SHA-256 rendered as hex is **exactly** 64 characters, so this
    /// limit must not be lowered without checking what that would exclude.
    pub const MAX_COMPONENT_LEN: usize = 64;

    /// Deepest path canopy accepts, in components.
    pub const MAX_DEPTH: usize = 16;

    /// Longest rendered path, in characters, separators included.
    pub const MAX_LEN: usize = 512;

    /// Validates and normalizes a `/`-separated path.
    ///
    /// Each component is validated the same way throughout -- ASCII letters folded
    /// to uppercase, digits, `-`, `_`; no `.`; no Windows reserved device stem -- so a
    /// component is always one safe on-disk entry. The empty path, an empty component (a
    /// leading, trailing, or doubled separator), and any `.` or `..` component are
    /// rejected: canopy has no relative addressing and no traversal, so a path always
    /// means exactly one node.
    ///
    /// Note that `.` and `..` are already excluded by the character rule, since `.` is
    /// not in the accepted set. They are named as their own error anyway, because a
    /// caller who writes `../SIBLING` has made a *conceptual* mistake -- assuming
    /// traversal exists -- and "invalid character '.'" would send them looking for a
    /// quoting bug instead.
    pub fn parse(raw: &str) -> Result<NodePath, NodePathError> {
        if raw.is_empty() {
            return Err(NodePathError::Empty);
        }
        let mut components = Vec::new();
        for (index, part) in raw.split(Self::SEPARATOR).enumerate() {
            components.push(validate_component(part, index)?);
        }
        NodePath::from_components(components)
    }

    /// Builds a path from already-validated component strings.
    ///
    /// The components are re-validated: a caller assembling a path from pieces has the
    /// same opportunity to supply a bad one as a caller writing it out, and validating in
    /// exactly one place is what keeps the two constructors from diverging.
    pub fn new<I, S>(components: I) -> Result<NodePath, NodePathError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let validated = components
            .into_iter()
            .enumerate()
            .map(|(index, part)| validate_component(part.as_ref(), index))
            .collect::<Result<Vec<String>, NodePathError>>()?;
        NodePath::from_components(validated)
    }

    /// Applies the whole-path bounds to already-validated components.
    fn from_components(components: Vec<String>) -> Result<NodePath, NodePathError> {
        if components.is_empty() {
            return Err(NodePathError::Empty);
        }
        if components.len() > Self::MAX_DEPTH {
            return Err(NodePathError::TooDeep {
                depth: components.len(),
                max: Self::MAX_DEPTH,
            });
        }
        // Components plus one separator between each adjacent pair.
        let len = components.iter().map(String::len).sum::<usize>() + components.len() - 1;
        if len > Self::MAX_LEN {
            return Err(NodePathError::TooLong {
                len,
                max: Self::MAX_LEN,
            });
        }
        Ok(NodePath { components })
    }

    /// The path's components, outermost first.
    pub fn components(&self) -> impl ExactSizeIterator<Item = &str> {
        self.components.iter().map(String::as_str)
    }

    /// How many components the path has. Always at least one.
    pub fn depth(&self) -> usize {
        self.components.len()
    }

    /// The last component -- the name of the node this path addresses.
    pub fn name(&self) -> &str {
        self.components
            .last()
            .map(String::as_str)
            .expect("a path always has at least one component")
    }

    /// The path of this node's parent, or `None` when it is a root-level node.
    ///
    /// A root-level node has no parent *path* rather than an empty one, because the
    /// empty path is not representable: the store root is not itself a node.
    pub fn parent(&self) -> Option<NodePath> {
        if self.components.len() == 1 {
            return None;
        }
        Some(NodePath {
            components: self.components[..self.components.len() - 1].to_vec(),
        })
    }

    /// This path extended by one component.
    pub fn child(&self, component: &str) -> Result<NodePath, NodePathError> {
        let validated = validate_component(component, self.components.len())?;
        let mut components = self.components.clone();
        components.push(validated);
        NodePath::from_components(components)
    }

    /// Whether `other` is this path or lies beneath it.
    ///
    /// Component-wise, never a string prefix test: `TICKETS` must not be reported as an
    /// ancestor of `TICKETSX`, which a `starts_with` on the rendered form would get
    /// wrong.
    pub fn contains(&self, other: &NodePath) -> bool {
        other.components.len() >= self.components.len()
            && self
                .components
                .iter()
                .zip(&other.components)
                .all(|(ours, theirs)| ours == theirs)
    }

    /// The path this node occupies within a partition subtree.
    pub fn relative_path(&self) -> PathBuf {
        self.components.iter().collect()
    }
}

impl fmt::Display for NodePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, component) in self.components.iter().enumerate() {
            if index > 0 {
                f.write_str("/")?;
            }
            f.write_str(component)?;
        }
        Ok(())
    }
}

/// Validates one path component, mapping a name-level rejection onto a path-level one.
///
/// `index` is the component's position, carried into the error so a caller told a
/// component is bad also learns *which* one -- a byte offset into the whole path would
/// otherwise be the only clue.
fn validate_component(raw: &str, index: usize) -> Result<String, NodePathError> {
    if raw.is_empty() {
        return Err(NodePathError::EmptyComponent { index });
    }
    if raw == "." || raw == ".." {
        return Err(NodePathError::RelativeComponent {
            index,
            component: raw.to_owned(),
        });
    }
    let mut canonical = String::with_capacity(raw.len());
    for (byte_offset, ch) in raw.char_indices() {
        let normalized = match ch {
            'a'..='z' => ch.to_ascii_uppercase(),
            'A'..='Z' | '0'..='9' | '-' | '_' => ch,
            _ => {
                return Err(NodePathError::InvalidCharacter {
                    index,
                    character: ch,
                    byte_offset,
                });
            }
        };
        canonical.push(normalized);
    }
    if canonical.len() > NodePath::MAX_COMPONENT_LEN {
        return Err(NodePathError::ComponentTooLong {
            index,
            len: canonical.len(),
            max: NodePath::MAX_COMPONENT_LEN,
        });
    }
    if is_reserved_name(&canonical) {
        return Err(NodePathError::ReservedComponent { index });
    }
    Ok(canonical)
}

/// Why a caller-supplied path was rejected by [`NodePath::parse`] or [`NodePath::new`].
///
/// Every component-level variant carries the component's `index`, so a rejection points
/// at the offending tier rather than at the path as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodePathError {
    /// The path was empty, or had no components.
    Empty,
    /// A component was empty -- a leading, trailing, or doubled separator.
    EmptyComponent {
        /// The empty component's position.
        index: usize,
    },
    /// A component was `.` or `..`; canopy has no relative addressing or traversal.
    RelativeComponent {
        /// The offending component's position.
        index: usize,
        /// The component as written.
        component: String,
    },
    /// A component contained a character outside the accepted set.
    InvalidCharacter {
        /// The offending component's position.
        index: usize,
        /// The offending character.
        character: char,
        /// Its byte offset within that component.
        byte_offset: usize,
    },
    /// A component exceeded [`NodePath::MAX_COMPONENT_LEN`] characters.
    ComponentTooLong {
        /// The offending component's position.
        index: usize,
        /// The rejected component's length, in characters.
        len: usize,
        /// The permitted maximum.
        max: usize,
    },
    /// A component is a Windows reserved device name (e.g. `CON`, `NUL`).
    ReservedComponent {
        /// The offending component's position.
        index: usize,
    },
    /// The path had more than [`NodePath::MAX_DEPTH`] components.
    TooDeep {
        /// The rejected path's depth.
        depth: usize,
        /// The permitted maximum.
        max: usize,
    },
    /// The rendered path exceeded [`NodePath::MAX_LEN`] characters.
    TooLong {
        /// The rejected path's rendered length, in characters.
        len: usize,
        /// The permitted maximum.
        max: usize,
    },
}

impl fmt::Display for NodePathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodePathError::Empty => f.write_str("node path is empty"),
            NodePathError::EmptyComponent { index } => write!(
                f,
                "node path component {index} is empty (a leading, trailing, or doubled '/')"
            ),
            NodePathError::RelativeComponent { index, component } => write!(
                f,
                "node path component {index} is {component:?}; canopy paths are absolute and have \
                 no traversal"
            ),
            NodePathError::InvalidCharacter {
                index,
                character,
                byte_offset,
            } => write!(
                f,
                "node path component {index} contains an invalid character {character:?} at byte \
                 offset {byte_offset}"
            ),
            NodePathError::ComponentTooLong { index, len, max } => write!(
                f,
                "node path component {index} is {len} characters, exceeding the {max}-character \
                 limit"
            ),
            NodePathError::ReservedComponent { index } => {
                write!(f, "node path component {index} is a reserved device name")
            }
            NodePathError::TooDeep { depth, max } => write!(
                f,
                "node path is {depth} components deep, exceeding the {max}-component limit"
            ),
            NodePathError::TooLong { len, max } => write!(
                f,
                "node path is {len} characters, exceeding the {max}-character limit"
            ),
        }
    }
}

impl std::error::Error for NodePathError {}

/// Why a caller-supplied name was rejected by [`DocumentName::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentNameError {
    /// The name was the empty string.
    Empty,
    /// The name exceeded [`DocumentName::MAX_LEN`] characters.
    TooLong {
        /// The rejected name's length, in characters.
        len: usize,
        /// The permitted maximum, [`DocumentName::MAX_LEN`].
        max: usize,
    },
    /// The name contained a character outside the accepted set.
    InvalidCharacter {
        /// The offending character.
        character: char,
        /// Its byte offset within the original name.
        byte_offset: usize,
    },
    /// The name began with `.`, which is reserved for canopy's own dotfiles.
    LeadingDot,
    /// The name ended with `.`, which Windows silently strips.
    TrailingDot,
    /// The name's stem (up to the first `.`) is a Windows reserved device name.
    Reserved,
}

impl fmt::Display for DocumentNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DocumentNameError::Empty => f.write_str("document name is empty"),
            DocumentNameError::TooLong { len, max } => {
                write!(
                    f,
                    "document name is {len} characters, exceeding the {max}-character limit"
                )
            }
            DocumentNameError::InvalidCharacter {
                character,
                byte_offset,
            } => write!(
                f,
                "document name contains an invalid character {character:?} at byte offset {byte_offset}"
            ),
            DocumentNameError::LeadingDot => f.write_str("document name must not begin with '.'"),
            DocumentNameError::TrailingDot => f.write_str("document name must not end with '.'"),
            DocumentNameError::Reserved => {
                f.write_str("document name stem is a reserved device name")
            }
        }
    }
}

impl std::error::Error for DocumentNameError {}

/// A caller-supplied token naming one durable write's staging temps (D40/D53).
///
/// canopy does not mint the nonce: it has no RNG (D61), so the caller supplies a value
/// unique to each write -- the D53 transaction id in the eventual commit path -- so a
/// write's staging temps (`<document>.tmp.<nonce>`) never collide with another
/// write's and crash-recovery can attribute an orphaned temp to the write that left
/// it. It is a short opaque token, validated only as a safe single path fragment:
/// ASCII letters (folded to uppercase for cross-filesystem determinism, like the name
/// types), ASCII digits, and the separators `-` and `_`. A `.` is rejected so the
/// `.tmp.<nonce>` temp suffix stays unambiguous. Because the nonce is folded, callers
/// must treat two nonces differing only in case as the same value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Nonce {
    canonical: String,
}

impl Nonce {
    /// Longest nonce canopy accepts, in characters (also its byte length, since the
    /// accepted set is ASCII). Shares [`NodePath::MAX_COMPONENT_LEN`]'s value and with
    /// it the one limit every name canopy validates obeys (`D222`).
    pub const MAX_LEN: usize = NodePath::MAX_COMPONENT_LEN;

    /// Validates and normalizes `raw` into a canonical nonce.
    ///
    /// The accepted character set is ASCII letters (folded to uppercase), ASCII
    /// digits, and the separators `-` and `_`. The empty token, a `.` (which would
    /// make the `.tmp.<nonce>` suffix ambiguous), any other character (path
    /// separators, whitespace, control bytes, non-ASCII), and an over-long token are
    /// rejected. A Windows reserved device name is accepted; see
    /// [`path_component`](Nonce::path_component).
    pub fn new(raw: &str) -> Result<Nonce, NonceError> {
        if raw.is_empty() {
            return Err(NonceError::Empty);
        }
        let mut canonical = String::with_capacity(raw.len());
        for (byte_offset, ch) in raw.char_indices() {
            let normalized = match ch {
                'a'..='z' => ch.to_ascii_uppercase(),
                'A'..='Z' | '0'..='9' | '-' | '_' => ch,
                _ => {
                    return Err(NonceError::InvalidCharacter {
                        character: ch,
                        byte_offset,
                    });
                }
            };
            canonical.push(normalized);
        }
        if canonical.len() > Self::MAX_LEN {
            return Err(NonceError::TooLong {
                len: canonical.len(),
                max: Self::MAX_LEN,
            });
        }
        Ok(Nonce { canonical })
    }

    /// The canonical (normalized) nonce.
    pub fn as_str(&self) -> &str {
        &self.canonical
    }

    /// The nonce as it appears where it forms a whole path component.
    ///
    /// On Windows a canonical nonce that is exactly a reserved device name (`CON`, `COM1`, ...)
    /// cannot name a directory, so its first character is replaced by U+00A9, the copyright
    /// sign. That
    /// character is outside the nonce set, so the result never equals another nonce's, and it
    /// keeps the length in characters. Nowhere else is a nonce altered: elsewhere it follows a
    /// `.`, where Windows does not apply the rule. Nothing reads a nonce back from a path.
    pub fn path_component(&self) -> std::borrow::Cow<'_, str> {
        if cfg!(windows) && is_reserved_name(&self.canonical) {
            let mut substituted = String::with_capacity(self.canonical.len() + 1);
            substituted.push(RESERVED_NONCE_SUBSTITUTE);
            substituted.push_str(&self.canonical[1..]);
            return std::borrow::Cow::Owned(substituted);
        }
        std::borrow::Cow::Borrowed(&self.canonical)
    }
}

impl fmt::Display for Nonce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical)
    }
}

/// Why a caller-supplied token was rejected by [`Nonce::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NonceError {
    /// The token was the empty string.
    Empty,
    /// The token exceeded [`Nonce::MAX_LEN`] characters.
    TooLong {
        /// The rejected token's length, in characters.
        len: usize,
        /// The permitted maximum, [`Nonce::MAX_LEN`].
        max: usize,
    },
    /// The token contained a character outside the accepted set (including `.`).
    InvalidCharacter {
        /// The offending character.
        character: char,
        /// Its byte offset within the original token.
        byte_offset: usize,
    },
}

impl fmt::Display for NonceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NonceError::Empty => f.write_str("nonce is empty"),
            NonceError::TooLong { len, max } => {
                write!(
                    f,
                    "nonce is {len} characters, exceeding the {max}-character limit"
                )
            }
            NonceError::InvalidCharacter {
                character,
                byte_offset,
            } => write!(
                f,
                "nonce contains an invalid character {character:?} at byte offset {byte_offset}"
            ),
        }
    }
}

impl std::error::Error for NonceError {}
