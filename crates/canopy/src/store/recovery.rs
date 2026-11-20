// Copyright (c) Michael Grier.

//! Resolving and recovering (canopy's DESIGN-NOTES.md, "Recovery is a run the caller drives").
//!
//! To *resolve* is to finish the unfinished work a writer left in the store's own data: the
//! transactions of the commit log, which the log alone decides (a prepared one rolls forward, a
//! begun one rolls back, and what each superseded is collected), the nonce claims of operations that
//! died, and the two things replaying the log must come after -- a pending document publication, and
//! the directories whose names were not yet made durable. To *recover* is the general case:
//! resolving, and making the coordination state trusted, which a fence alone does not need and a
//! host restart does. Opening does neither; it reports what it found, and both are the caller's.

use super::coordinated::Coordinated;
use super::surface::CLAIMS_DIR;
use super::*;

/// What recovery does, in the order it does it.
///
/// The order is the design's: the store's own pending work and the commit log first, the claims
/// of operations that died next, and the coordination state last -- the count and the trusted state
/// published together, and the fence cleared as the very last act -- so that a stop or a death at
/// any step leaves the fence set and the next recovery starts from what the commit log says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RecoveryPhase {
    /// Syncing the parent of each directory an earlier open may have created without making its
    /// name durable, then emptying the durable record of them.
    DirectorySyncDebt,
    /// Finishing the document publication a failed write left pending.
    Publication,
    /// Settling each transaction of the commit log: rolling a prepared one forward through its
    /// swaps and its end record, and a begun one back by sweeping its staging.
    Transactions,
    /// Collecting what each transaction superseded, which waits until every one is settled.
    Collection,
    /// Releasing the nonce claims of operations that died.
    Claims,
    /// Resuming caller-supplied migration units after interrupted journal work has settled.
    UpgradeBody,
    /// Publishing the even count and the trusted state, and clearing the fence last.
    Completion,
}

impl RecoveryPhase {
    fn next(self) -> Option<Self> {
        match self {
            RecoveryPhase::DirectorySyncDebt => Some(RecoveryPhase::Publication),
            RecoveryPhase::Publication => Some(RecoveryPhase::Transactions),
            RecoveryPhase::Transactions => Some(RecoveryPhase::Collection),
            RecoveryPhase::Collection => Some(RecoveryPhase::Claims),
            RecoveryPhase::Claims => Some(RecoveryPhase::UpgradeBody),
            RecoveryPhase::UpgradeBody => Some(RecoveryPhase::Completion),
            RecoveryPhase::Completion => None,
        }
    }
}

/// Where a [`Recovery`] is, as of its last step.
///
/// The units are bounded pieces of work -- one directory synced, one transaction settled -- and the
/// total is what the run knows of so far: a phase's units are counted when it is entered, so the
/// total grows as the run reaches phases whose work it had not yet measured, which
/// [`total_grew`](RecoveryProgress::total_grew) reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RecoveryProgress {
    phase: RecoveryPhase,
    done: u64,
    known: u64,
    total_grew: bool,
    finished: bool,
}

impl RecoveryProgress {
    /// The phase the next step belongs to, or the last phase once the run is finished.
    #[must_use]
    pub const fn phase(&self) -> RecoveryPhase {
        self.phase
    }

    /// Units of work done so far.
    #[must_use]
    pub const fn done(&self) -> u64 {
        self.done
    }

    /// Units of work known so far, those done included.
    #[must_use]
    pub const fn known(&self) -> u64 {
        self.known
    }

    /// Whether the known total grew in the step that reported this.
    #[must_use]
    pub const fn total_grew(&self) -> bool {
        self.total_grew
    }

    /// Whether recovery has completed: the count is published, the state trusted and the fence
    /// cleared.
    #[must_use]
    pub const fn finished(&self) -> bool {
        self.finished
    }
}

/// Whether recovery is required, and why, and whether writers left work to resolve.
///
/// It reports abandoned work and never work in progress: a live writer's transaction is not
/// reported, and a recovery that has not finished is, since the fence stays set until it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RecoveryNeed {
    fenced: bool,
    state_untrusted: bool,
    holder_died: bool,
    unresolved_work: Option<bool>,
}

impl RecoveryNeed {
    pub(super) fn set_unresolved_work(&mut self, work: Option<bool>) {
        self.unresolved_work = work;
    }

    pub(super) fn observed(need: acadia::coord::RecoveryNeed) -> Self {
        Self {
            fenced: need.fenced(),
            state_untrusted: need.untrusted().is_some(),
            holder_died: need.holder_died(),
            unresolved_work: None,
        }
    }

    /// Whether recovery is required, for any reason.
    #[must_use]
    pub const fn required(&self) -> bool {
        self.fenced || self.state_untrusted || self.holder_died
    }

    /// Whether the fence is set: a transaction failed unresolved, a writer died holding exclusion
    /// and another acquired it, or a recovery began and has not finished.
    #[must_use]
    pub const fn fenced(&self) -> bool {
        self.fenced
    }

    /// Whether the coordination state cannot be vouched for: it was never made trusted by a
    /// recovery, or the host has restarted since.
    #[must_use]
    pub const fn state_untrusted(&self) -> bool {
        self.state_untrusted
    }

    /// Whether a writer died holding exclusion: nobody has acquired it since, or, as
    /// [`open`](Store::open) reports it, the acquisition that opened this handle was the first.
    #[must_use]
    pub const fn holder_died(&self) -> bool {
        self.holder_died
    }

    /// Whether writers left work on the store root's work list for [`resolve`](Store::resolve) --
    /// directories whose names are not yet durable, a pending document publication, records in the
    /// commit log, or nonce claims -- or `None` when this handle cannot tell work left behind from a
    /// live operation's: exclusion was occupied during the observation.
    ///
    /// Work left behind does not by itself require recovery: a write settles a pending publication
    /// and the directory names before it acts, and a claim refuses only its own nonce.
    #[must_use]
    pub const fn unresolved_work(&self) -> Option<bool> {
        self.unresolved_work
    }
}

/// The units the run knows before it measures anything: the publication, the claims and the
/// completion each count one.
const FIXED_UNITS: u64 = 3;

type RecoveryUnit<'callback> =
    dyn FnMut(&super::MigrationUnit<'_>) -> Result<(), StoreError> + 'callback;

/// A recovery of one store, run a step at a time.
///
/// Every step does bounded, idempotent work from what is durable, so a run that is dropped part-way
/// leaves the store as it was found or further along, and a later run -- from this process or, after
/// a death, another -- is the same recovery from the start: it reads the commit log and never the
/// coordination state, and does again what is already done at no cost. Dropping a run changes no
/// state of the handle: it still needs recovery until a run reaches its last step.
pub struct Recovery {
    store: Store,
    phase: RecoveryPhase,
    /// What the current phase holds, once measured on entering it.
    units: Option<u64>,
    done_in_phase: u64,
    done: u64,
    known: u64,
    grew: bool,
    finished: bool,
    /// The directories still to sync, last first.
    dirs: Vec<PathBuf>,
    /// Whether the durable record of directories holds anything to empty once they are synced.
    dir_record: bool,
    /// The commit log as it was when the current phase was entered.
    records: Vec<LogRecord>,
    completed_txns: Vec<Nonce>,
    completed_dirs: Vec<PathBuf>,
    observed_count: u64,
}

/// Makes the modifications a step begins recovery's, which a state that requires recovery does
/// not refuse, and ordinary again when the step ends.
struct RecoveringScope<'a>(&'a Coordinated, bool);

impl<'a> RecoveringScope<'a> {
    fn enter(coordination: &'a Coordinated) -> Self {
        let previous = coordination.is_recovering();
        coordination.set_recovering(true);
        RecoveringScope(coordination, previous)
    }
}

impl Drop for RecoveringScope<'_> {
    fn drop(&mut self) {
        self.0.set_recovering(self.1);
    }
}

impl Recovery {
    fn begin(store: &Store) -> Result<Recovery, StoreError> {
        store.refuse_if_read_only()?;
        let mut run = Recovery {
            store: store.clone(),
            phase: RecoveryPhase::DirectorySyncDebt,
            units: None,
            done_in_phase: 0,
            done: 0,
            known: FIXED_UNITS,
            grew: false,
            finished: false,
            dirs: Vec::new(),
            dir_record: false,
            records: Vec::new(),
            completed_txns: Vec::new(),
            completed_dirs: Vec::new(),
            observed_count: 0,
        };
        let _writer = store.enter_exclusion()?;
        if store.has_unresolved_work()? && !store.writer.is_unresolved() {
            let _recovering = RecoveringScope::enter(&store.coordination);
            store.coordination.begin(&store.root)?;
            store.coordination.fence_modification()?;
            store.writer.mark_unresolved();
        }
        run.observed_count = store.modifications()?;
        run.settle_position()?;
        Ok(run)
    }

    /// Where the run is as of its last step.
    #[must_use]
    pub fn progress(&self) -> RecoveryProgress {
        RecoveryProgress {
            phase: self.phase,
            done: self.done,
            known: self.known,
            total_grew: self.grew,
            finished: self.finished,
        }
    }

    /// Does one unit of work, reporting progress and an independent stamp result in [`WriteOutcome`].
    ///
    /// A step that fails leaves its unit to be done again by the next, and a step on a finished run
    /// does nothing.
    ///
    /// # Errors
    ///
    /// The failure of the step's own work, which a later step retries.
    pub fn step(&mut self) -> Result<WriteOutcome<RecoveryProgress>, StoreError> {
        self.step_with_unit(None)
    }

    /// Supplies the idempotent unit required when recovery reaches an interrupted upgrade body.
    pub fn step_upgrade(
        &mut self,
        mut unit: impl FnMut(&super::MigrationUnit<'_>) -> Result<(), StoreError>,
    ) -> Result<WriteOutcome<RecoveryProgress>, StoreError> {
        self.store.refuse_unit_control()?;
        self.step_with_unit(Some(&mut unit))
    }

    fn step_with_unit(
        &mut self,
        mut unit: Option<&mut RecoveryUnit<'_>>,
    ) -> Result<WriteOutcome<RecoveryProgress>, StoreError> {
        let store = self.store.clone();
        let _lease = store.operation_lease()?;
        if self.finished {
            return Ok(WriteOutcome::new(self.progress(), Ok(())));
        }
        let _writer = store.enter_exclusion()?;
        let mut changed = false;
        let known_before = self.known;
        self.grew = false;
        let count = store.modifications()?;
        if count != self.observed_count {
            self.phase = RecoveryPhase::DirectorySyncDebt;
            self.units = None;
            self.done_in_phase = 0;
            self.completed_txns.clear();
            self.completed_dirs.clear();
            self.known = self.done + FIXED_UNITS;
            self.observed_count = count;
        }
        if _writer.other_holder {
            if let Some(units) = self.units.take()
                && matches!(
                    self.phase,
                    RecoveryPhase::DirectorySyncDebt
                        | RecoveryPhase::Transactions
                        | RecoveryPhase::Collection
                        | RecoveryPhase::UpgradeBody
                )
            {
                self.known -= units.saturating_sub(self.done_in_phase);
            }
            self.done_in_phase = 0;
            if matches!(
                self.phase,
                RecoveryPhase::UpgradeBody | RecoveryPhase::Completion
            ) && store.claims_left()?
            {
                self.phase = RecoveryPhase::Claims;
                self.known += 1;
            }
        }
        self.settle_position()?;
        if !self.finished {
            let _recovering = RecoveringScope::enter(&store.coordination);
            if self.phase == RecoveryPhase::UpgradeBody {
                let callback = unit
                    .as_mut()
                    .ok_or_else(|| StoreError::UpgradeBodyRequired {
                        root: store.root.clone(),
                    })?;
                let outcome = store.run_upgrade_unit(|context| callback(context), true)?;
                self.done += 1;
                self.done_in_phase += 1;
                self.settle_position()?;
                self.grew = self.known > known_before;
                self.observed_count = store.modifications()?;
                return Ok(WriteOutcome::new(self.progress(), outcome.agent_stamp));
            }
            changed = self.run_unit(&store)?;
            self.done += 1;
            self.done_in_phase += 1;
        }
        let settled = self.settle_position();
        self.grew = self.known > known_before;
        settled?;
        self.observed_count = store.modifications()?;
        Ok(store.stamped(self.progress(), changed))
    }

    /// Enters the next phase while the current one has no unit left, measuring each as it is
    /// entered, and finishes the run past the last.
    fn settle_position(&mut self) -> Result<(), StoreError> {
        loop {
            if self.finished {
                return Ok(());
            }
            match self.units {
                None => self.measure()?,
                Some(units) if self.done_in_phase >= units => match self.phase.next() {
                    Some(next) => {
                        self.phase = next;
                        self.units = None;
                        self.done_in_phase = 0;
                        self.completed_txns.clear();
                    }
                    None => self.finished = true,
                },
                Some(_) => return Ok(()),
            }
        }
    }

    /// Counts the units of the phase just entered, from what is durable now.
    fn measure(&mut self) -> Result<(), StoreError> {
        let units = match self.phase {
            RecoveryPhase::DirectorySyncDebt => {
                let (mut dirs, record) = self.store.unsynced_dirs_to_discharge()?;
                dirs.retain(|dir| !self.completed_dirs.contains(dir));
                let units = dirs.len() as u64 + u64::from(record);
                self.dirs = dirs;
                self.dir_record = record;
                self.known += units;
                units
            }
            RecoveryPhase::Transactions | RecoveryPhase::Collection => {
                let mut records = self.store.commit_log().read()?;
                records.retain(|record| !self.completed_txns.contains(record.txn()));
                let units = group_by_transaction(&records).len() as u64;
                self.records = records;
                self.known += units;
                units
            }
            RecoveryPhase::Publication | RecoveryPhase::Claims | RecoveryPhase::Completion => 1,
            RecoveryPhase::UpgradeBody => {
                let remaining = self.store.upgrade_units_remaining()?;
                self.known = self.known.saturating_add(remaining);
                remaining
            }
        };
        self.units = Some(units);
        Ok(())
    }

    fn run_unit(&mut self, store: &Store) -> Result<bool, StoreError> {
        match self.phase {
            RecoveryPhase::DirectorySyncDebt => match self.dirs.last() {
                Some(dir) => {
                    store.sync_parent_of(dir)?;
                    self.completed_dirs.push(dir.clone());
                    self.dirs.pop();
                    Ok(true)
                }
                None => store.empty_dir_record().map(|_| true),
            },
            RecoveryPhase::Publication => store.settle_publication(),
            RecoveryPhase::Transactions => {
                let txns = group_by_transaction(&self.records);
                let txn = &txns[self.done_in_phase as usize];
                store.settle_transaction(txn)?;
                self.completed_txns.push(txn.id.clone());
                Ok(true)
            }
            RecoveryPhase::Collection => {
                let txns = group_by_transaction(&self.records);
                let txn = &txns[self.done_in_phase as usize];
                store.collect_transaction(txn)?;
                self.completed_txns.push(txn.id.clone());
                Ok(true)
            }
            RecoveryPhase::Claims => {
                let changed = store.claims_left()?;
                store.release_stale_claims()?;
                Ok(changed)
            }
            RecoveryPhase::Completion => {
                let changed = store.finish_schema_transition()?
                    || !self.records.is_empty()
                    || store.writer.is_unresolved();
                // Every transaction is settled and collected by now, so emptying the log is
                // best-effort, as at the end of a write: what it holds replays as a no-op.
                let _ = store.commit_log().reset();
                if store.writer.is_unresolved() {
                    store.coordination.complete_recovery()?;
                    store.writer.clear_unresolved();
                }
                Ok(changed)
            }
            RecoveryPhase::UpgradeBody => Err(StoreError::UpgradeBodyRequired {
                root: store.root.clone(),
            }),
        }
    }
}

impl Store {
    /// Resolves what writers left on the store root's work list, and, when the coordination state
    /// was vouched for and so a fence was all that required recovery, completes it -- publishes the
    /// even count and clears the fence -- which is the one part of recovery that follows from
    /// resolving alone.
    ///
    /// The directory names come first and the publication next, then the log, which is emptied once
    /// every transaction in it is settled and collected, then the claims: a
    /// write that creates a directory empties the durable record of names not yet synced, and an
    /// operation that acts on the namespace settles a pending publication before it acts, since a
    /// replay over a document whose replicas a failed write left disagreeing would strand its vote.
    /// Every claim found is released, which is sound because this handle's writer gate excludes its
    /// own operations and coordination exclusion every other writer.
    ///
    /// Anything else a recovery does is left to [`recover`](Store::recover): a store whose state
    /// cannot be vouched for stays needing it. A store with nothing to resolve is left as it was.
    ///
    /// # Errors
    ///
    /// [`StoreError::ReadOnly`] for a handle opened only to read, and the failure of the work, which
    /// a later call does again.
    pub fn resolve(&self) -> Result<WriteOutcome<()>, StoreError> {
        self.refuse_unit_control()?;
        let _lease = self.operation_lease()?;
        self.refuse_if_read_only()?;
        let _writer = self.enter_exclusion()?;
        let changed = self.has_unresolved_work()?
            || (self.writer.is_unresolved() && self.coordination.state_was_vouched());
        {
            let _writer = self.writer.enter();
            let _recovering = RecoveringScope::enter(&self.coordination);
            self.resolve_data_work()?;
            self.finish_schema_transition()?;
        }
        if self.writer.is_unresolved() && self.coordination.state_was_vouched() {
            self.coordination.complete_recovery()?;
            self.writer.clear_unresolved();
        }
        Ok(self.stamped((), changed))
    }

    fn claims_left(&self) -> Result<bool, StoreError> {
        match self.vfs.read_dir(&self.root.join(CLAIMS_DIR)) {
            Ok(entries) => Ok(!entries.is_empty()),
            Err(error) if error.kind() == VfsErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn resolve_data_work(&self) -> Result<(), StoreError> {
        let (dirs, record) = self.unsynced_dirs_to_discharge()?;
        for dir in dirs.iter().rev() {
            self.sync_parent_of(dir)?;
        }
        if record {
            self.empty_dir_record()?;
        }
        self.settle_publication()?;
        let records = self.commit_log().read()?;
        let txns = group_by_transaction(&records);
        for txn in &txns {
            self.settle_transaction(txn)?;
        }
        for txn in &txns {
            self.collect_transaction(txn)?;
        }
        let _ = self.commit_log().reset();
        self.release_stale_claims()?;
        Ok(())
    }

    /// Whether recovery is required, and why, as one observation of the coordination state that
    /// changes nothing, so any handle -- one opened only to read included -- may ask.
    ///
    /// The observation is consistent: it reads the fence, the restart state and the ownership
    /// record while no writer can come between the readings, or reports from the fence and the
    /// restart state alone when a writer holds exclusion, so a live writer's transaction is never
    /// reported as abandoned while its holder acquires or releases. The work list is read inside
    /// that same observation only when exclusion is free, for read-only handles too.
    ///
    /// # Errors
    ///
    /// [`StoreError::Coordination`] if the coordination state cannot be observed, and the failure
    /// to read the work list.
    pub fn recovery_need(&self) -> Result<RecoveryNeed, StoreError> {
        let _lease = self.operation_lease()?;
        let mut work = None;
        let need = self.coordination.discover_with(&mut || {
            work = Some(Self::root_has_unresolved_work(&self.vfs, &self.root));
        })?;
        let unresolved_work = work.transpose()?;
        Ok(RecoveryNeed {
            fenced: need.fenced(),
            state_untrusted: need.untrusted().is_some(),
            holder_died: need.holder_died(),
            unresolved_work,
        })
    }

    /// What acquiring writer exclusion found when this handle was opened, with the work list as it
    /// stands: what [`open`](Store::open) reports.
    pub(super) fn found_on_opening(&self) -> Result<RecoveryNeed, StoreError> {
        self.recovery_need()
    }

    /// Whether the root's work list holds anything for [`resolve`](Store::resolve). It only reads:
    /// the directories this process remembers stay remembered.
    fn has_unresolved_work(&self) -> Result<bool, StoreError> {
        let remembered = !self
            .unsynced_dirs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .dirs
            .is_empty();
        Ok(remembered || Self::root_has_unresolved_work(&self.vfs, &self.root)?)
    }

    pub(super) fn root_has_unresolved_work(
        vfs: &Arc<dyn Vfs>,
        root: &Path,
    ) -> Result<bool, StoreError> {
        let holds = |name: &str| -> Result<bool, StoreError> {
            match vfs.read(&root.join(name)) {
                Ok(bytes) => Ok(!bytes.is_empty()),
                Err(error) if error.kind() == VfsErrorKind::NotFound => Ok(false),
                Err(error) => Err(StoreError::Vfs(error)),
            }
        };
        let claims = match vfs.read_dir(&root.join(CLAIMS_DIR)) {
            Ok(entries) => !entries.is_empty(),
            Err(error) if error.kind() == VfsErrorKind::NotFound => false,
            Err(error) => return Err(StoreError::Vfs(error)),
        };
        Ok(claims
            || holds(DIR_SYNC_DEBT_NAME)?
            || holds(PUBLICATION_RECORD_NAME)?
            || !CommitLog::new(Arc::clone(vfs), root).read()?.is_empty()
            || vfs.exists(&root.join(super::UPGRADE_INTENT_NAME))?
            || Self::read_genesis(vfs, root)?.metadata.transition.is_some())
    }

    /// Begins a recovery of this store, to be run a step at a time with [`Recovery::step`].
    ///
    /// Each unit acquires and releases exclusion; no hold remains across caller calls.
    ///
    /// # Errors
    ///
    /// [`StoreError::ReadOnly`] for a handle opened only to read, and the failure to measure the
    /// first phase's work.
    pub fn recovery(&self) -> Result<Recovery, StoreError> {
        self.refuse_unit_control()?;
        let _lease = self.operation_lease()?;
        Recovery::begin(self)
    }

    /// Recovers the store, running every step to the end.
    ///
    /// A store that needs none is left as it was found, apart from the work a clean store may
    /// still owe: directories whose names were not made durable, and a document publication a
    /// failed write left pending.
    ///
    /// # Errors
    ///
    /// The failure of a step, after which a later call resumes the same recovery.
    pub fn recover(&self) -> Result<WriteOutcome<()>, StoreError> {
        let _lease = self.operation_lease()?;
        let mut stamp = Ok(());
        let mut run = self.recovery()?;
        while !run.progress().finished() {
            let outcome = run.step()?;
            if stamp.is_ok() {
                stamp = outcome.agent_stamp;
            }
        }
        Ok(WriteOutcome::new((), stamp))
    }
}
