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
}

impl Modifications {
    pub(crate) fn is_open(&self) -> bool {
        self.depth > 0
    }
}

impl FileCoordination {
    /// Publishes `value` with `standing` in this boot, replacing the published record whole.
    pub(crate) fn publish_count(&self, value: u64, standing: Standing) -> CoordResult<()> {
        self.alive()?;
        let boot = self.current_boot()?.unwrap_or_default();
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
        self.vfs().write(&layout.fence(), &[])?;
        Ok(())
    }

    /// Begins a modification. The outermost raises an even count to odd before the caller's
    /// first effect; a count already odd -- left by a dead holder, or untrusted -- stays as it is.
    /// A failed raise leaves the count unchanged and no modification open.
    pub(crate) fn begin_modification(&self, open: &mut Modifications) -> CoordResult<()> {
        self.alive()?;
        if open.depth == 0 {
            let published = self.read_count()?;
            open.raised = None;
            if published.sample().is_settled() {
                let odd = published.value + 1;
                self.publish_count(odd, published.standing)?;
                open.raised = Some((odd, published.standing));
            }
        }
        open.depth += 1;
        Ok(())
    }

    /// Ends the innermost modification. The outermost one that raised the count closes it even
    /// for a settled end; an unresolved end sets the fence and leaves the count odd, as does a
    /// closing raise that fails, whose failure is returned.
    pub(crate) fn end_modification(
        &self,
        open: &mut Modifications,
        end: ModificationEnd,
    ) -> CoordResult<()> {
        self.alive()?;
        if open.depth == 0 {
            return Err(CoordError::NoModificationOpen);
        }
        open.depth -= 1;
        if open.depth > 0 {
            return Ok(());
        }
        match (end, open.raised.take()) {
            (ModificationEnd::Unresolved, _) => self.set_fence(),
            (ModificationEnd::Settled, None) => Ok(()),
            (ModificationEnd::Settled, Some((odd, standing))) => {
                self.publish_count(odd + 1, standing).inspect_err(|_| {
                    let _ = self.set_fence();
                })
            }
        }
    }
}
