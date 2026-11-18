// Copyright (c) Michael Grier.

//! Raising the count, which only the holder of writer exclusion does.

use crate::coord::{CoordError, CoordResult, ModificationEnd};

use super::super::FileCoordination;
use super::super::restart::Standing;
use super::format;

fn encode(value: u64, standing: Standing, boot: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(format::MAGIC.len() + 1 + format::VALUE_WIDTH + boot.len());
    bytes.extend_from_slice(&format::MAGIC);
    bytes.push(match standing {
        Standing::Untrusted => format::UNTRUSTED,
        Standing::Vouched => format::VOUCHED,
    });
    bytes.extend_from_slice(&value.to_le_bytes());
    bytes.extend_from_slice(boot);
    bytes
}

/// The modifications open under one hold of writer exclusion.
///
/// Only the outermost raises the count, so an operation nested under a hold never restores even
/// parity early.
#[derive(Debug, Default)]
pub(crate) struct Modifications {
    depth: u32,
    /// The odd value and standing the outermost raised the count to, which it must close.
    raised: Option<(u64, Standing)>,
    unresolved: bool,
}

impl Modifications {
    pub(crate) fn is_open(&self) -> bool {
        self.depth > 0
    }
}

impl FileCoordination {
    /// Publishes `value` with `standing` in this boot, replacing the published record whole. A
    /// vouched count names its boot, so it is refused where the boot cannot be identified.
    pub(crate) fn publish_count(&self, value: u64, standing: Standing) -> CoordResult<()> {
        self.alive()?;
        let boot = match (self.current_boot()?, standing) {
            (Some(boot), _) => boot,
            (None, Standing::Untrusted) => Vec::new(),
            (None, Standing::Vouched) => {
                return Err(crate::VfsError::new(
                    crate::VfsErrorKind::Unsupported,
                    "boot_identity",
                    "this filesystem cannot identify a boot",
                )
                .into());
            }
        };
        let layout = self.layout();
        self.vfs().create_dir_all(&layout.state_dir())?;
        self.replace_whole(
            &layout.count_staging(&super::super::staging_suffix()),
            &layout.count(),
            &encode(value, standing, &boot),
        )
    }

    /// Sets the fence, which only a completed recovery clears.
    pub(crate) fn set_fence(&self) -> CoordResult<()> {
        self.alive()?;
        let layout = self.layout();
        self.vfs().create_dir_all(&layout.state_dir())?;
        self.publishing(|vfs| vfs.write(&layout.fence(), &[]))
    }

    /// Begins a modification. The outermost raises an even count to odd before the caller's
    /// first effect; a count already odd -- left by a dead holder, or untrusted -- stays as it is.
    /// A failed raise leaves the count unchanged and no modification open.
    pub(crate) fn begin_modification(&self, open: &mut Modifications) -> CoordResult<()> {
        self.alive()?;
        let next_depth = open.depth.checked_add(1).ok_or(CoordError::Exhausted {
            what: "modification nesting",
        })?;
        if open.depth == 0 {
            let published = self.read_count()?;
            if published.value == u64::MAX {
                return Err(CoordError::Exhausted {
                    what: "modification count",
                });
            }
            open.raised = None;
            open.unresolved = false;
            if published.sample().is_settled() {
                published
                    .value
                    .checked_add(2)
                    .ok_or(CoordError::Exhausted {
                        what: "modification count",
                    })?;
                let odd = published.value + 1;
                self.publish_count(odd, published.standing)?;
                open.raised = Some((odd, published.standing));
            }
        }
        open.depth = next_depth;
        Ok(())
    }

    /// Any unresolved end fences the logical modification and preserves odd parity until recovery.
    /// Only an outermost settled end with no unresolved inner end may publish the closing raise.
    pub(crate) fn end_modification(
        &self,
        open: &mut Modifications,
        end: ModificationEnd,
    ) -> CoordResult<()> {
        self.alive()?;
        if open.depth == 0 {
            return Err(CoordError::NoModificationOpen);
        }
        if end == ModificationEnd::Unresolved {
            open.unresolved = true;
            self.set_fence()?;
        }
        open.depth -= 1;
        if open.depth > 0 || open.unresolved {
            return Ok(());
        }
        match open.raised.take() {
            None => Ok(()),
            Some((odd, standing)) => odd
                .checked_add(1)
                .ok_or(CoordError::Exhausted {
                    what: "modification count",
                })
                .and_then(|even| self.publish_count(even, standing))
                .inspect_err(|_| {
                    let _ = self.set_fence();
                }),
        }
    }
}
