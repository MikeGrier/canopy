// Copyright (c) Michael Grier.

//! In-process sessions, queue pairs, and the store's asynchronous command engine.

use std::cell::Cell;
use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use acadia::Signal;
use acadia::coord::Notice;

use super::{Doorbell, Watch};
use crate::{
    Children, Commit, DocumentName, NodePath, Read, Store, StoreError, Transaction,
    TransactionOptions, WriteOutcome,
};

/// The longest a parked command relies on its count notification before rechecking it.
const NOTIFICATION_RECHECK: Duration = Duration::from_secs(1);

/// A pair-scoped identity assigned when a command is accepted.
///
/// This is local correlation, not a durable receipt. It is unique only for the lifetime of one
/// queue pair, and resubmitting the command does not deduplicate an earlier uncertain effect
/// (`SubmissionOutcomesCoherent`, `AtMostOneDelivery`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(u64);

impl RequestId {
    /// The numeric identity, unique for the lifetime of its queue pair.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Why a local submission was refused before a request identity was assigned.
///
/// Refusal accepts no command and has no store effect. `CompletionQueueFull` asks the caller to
/// free a slot; `PairClosed` requires a new pair and therefore a new request identity. Both are
/// covered by `SubmissionOutcomesCoherent` and `CompletionCapacityReserved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmissionRefusal {
    /// The pair or session has closed.
    PairClosed,
    /// Every completion slot reserved by this pair is occupied.
    CompletionQueueFull,
}

impl fmt::Display for SubmissionRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SubmissionRefusal::PairClosed => formatter.write_str("the queue pair is closed"),
            SubmissionRefusal::CompletionQueueFull => {
                formatter.write_str("the queue pair has no free completion slot")
            }
        }
    }
}

impl std::error::Error for SubmissionRefusal {}

/// A fixed read/snapshot command whose result can cross the asynchronous boundary.
#[derive(Debug, Clone)]
pub enum SnapshotCommand {
    /// Read one document through [`Store::snapshot`].
    ReadDocument(NodePath, DocumentName),
    /// List one node through [`Store::snapshot`].
    ListChildren(NodePath),
    /// Enumerate all documents through [`Store::snapshot`].
    ListDocuments,
}

/// A command accepted into a session queue.
pub enum Command {
    /// Submit an opaque transaction once.
    Transaction(Transaction, TransactionOptions),
    /// Read one document without retrying contention.
    ReadDocument(NodePath, DocumentName),
    /// List the immediate children of one node.
    ListChildren(NodePath),
    /// Enumerate all documents.
    ListDocuments,
    /// Run one fixed snapshot operation.
    Snapshot(SnapshotCommand),
}

impl fmt::Debug for Command {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Command::Transaction(_, _) => formatter.write_str("Command::Transaction(..)"),
            Command::ReadDocument(_, _) => formatter.write_str("Command::ReadDocument(..)"),
            Command::ListChildren(_) => formatter.write_str("Command::ListChildren(..)"),
            Command::ListDocuments => formatter.write_str("Command::ListDocuments"),
            Command::Snapshot(command) => formatter
                .debug_tuple("Command::Snapshot")
                .field(command)
                .finish(),
        }
    }
}

/// The typed value of a completed command.
#[derive(Debug)]
pub enum CommandValue {
    /// The one-shot transaction outcome.
    Transaction(WriteOutcome<Commit>),
    /// A direct document read.
    Read(Read),
    /// One node's children.
    Children(Children),
    /// The sorted document enumeration.
    Documents(Vec<(NodePath, DocumentName)>),
    /// A command run through the undisturbed snapshot.
    Snapshot(SnapshotValue),
}

/// A fixed snapshot's returned value.
#[derive(Debug)]
pub enum SnapshotValue {
    /// One document read.
    Read(Read),
    /// One node's children.
    Children(Children),
    /// The sorted document enumeration.
    Documents(Vec<(NodePath, DocumentName)>),
}

/// One accepted command's completion while its pair remains live.
///
/// A pair delivers at most one completion for this request, and completions from that pair are
/// request-ordered (`AtMostOneDelivery`, `PerPairCompletionOrder`). If the pair is lost, the caller
/// receives no fabricated completion; the corresponding client-contract state is
/// `CompletionUnavailable` and its effect is unknown (`LostCompletionIsUnknown`).
#[derive(Debug)]
pub struct Completion {
    request: RequestId,
    outcome: Result<CommandValue, StoreError>,
}

impl Completion {
    /// The pair-local request this completion answers.
    #[must_use]
    pub fn request(&self) -> RequestId {
        self.request
    }

    /// The command's store-visible outcome.
    ///
    /// A [`StoreError::RecoveryRequired`] does not prove that an earlier uncertain request failed.
    /// Recovery can finish durable work already past its commit point; the request retains uncertain
    /// effect until application-level evidence establishes the fact (`RecoveryReportsUncertainEffect`).
    pub fn outcome(&self) -> &Result<CommandValue, StoreError> {
        &self.outcome
    }

    /// Separates correlation from the command outcome.
    pub fn into_parts(self) -> (RequestId, Result<CommandValue, StoreError>) {
        (self.request, self.outcome)
    }
}

struct Queued {
    id: RequestId,
    command: Command,
}

struct PairState<S: Signal> {
    live: bool,
    next_request: u64,
    outstanding: usize,
    capacity: usize,
    queued: VecDeque<Queued>,
    completions: VecDeque<Completion>,
    in_flight: bool,
    parked: bool,
    notice: Option<Box<dyn Notice>>,
    completion_bell: Arc<Doorbell<S>>,
}

struct PairInner<S: Signal> {
    id: u64,
    engine: Weak<Engine<S>>,
    state: Mutex<PairState<S>>,
}

impl<S: Signal> PairInner<S> {
    fn try_push(&self, command: Command) -> Result<RequestId, SubmissionRefusal> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.live {
            return Err(SubmissionRefusal::PairClosed);
        }
        if state.outstanding >= state.capacity {
            return Err(SubmissionRefusal::CompletionQueueFull);
        }
        let request = RequestId(state.next_request);
        state.next_request = state.next_request.saturating_add(1);
        state.outstanding += 1;
        state.queued.push_back(Queued {
            id: request,
            command,
        });
        Ok(request)
    }

    fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.live {
            return;
        }
        state.live = false;
        state.outstanding -= state.queued.len() + state.completions.len();
        state.queued.clear();
        state.completions.clear();
        state.notice.take();
    }

    fn pop_completion(&self) -> Option<Completion> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let completion = state.completions.pop_front();
        if completion.is_some() {
            state.outstanding = state.outstanding.saturating_sub(1);
        }
        completion
    }

    fn take_completions(&self) -> VecDeque<Completion> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let count = state.completions.len();
        state.outstanding = state.outstanding.saturating_sub(count);
        std::mem::take(&mut state.completions)
    }

    fn put_completions_front(&self, completions: VecDeque<Completion>) {
        if completions.is_empty() {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.outstanding += completions.len();
        let existing = std::mem::take(&mut state.completions);
        state.completions = completions.into_iter().chain(existing).collect();
    }

    fn completion_bell(&self) -> Arc<Doorbell<S>> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(&state.completion_bell)
    }

    fn check_engine_failure(&self) -> io::Result<()> {
        if let Some(engine) = self.engine.upgrade()
            && let Some(failure) = engine
                .failure
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        {
            return Err(io::Error::other(failure));
        }
        Ok(())
    }
}

/// The mutable producer end of a queue pair.
///
/// It is `Send` but not `Sync`; all producer mutation takes `&mut self`.
///
/// ```compile_fail
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<canopy::ring::SubmissionQueue<'static>>();
/// ```
pub struct SubmissionQueue<'session, S: Signal = acadia::PlatformSignal> {
    pair: Arc<PairInner<S>>,
    submission_bell: Arc<Doorbell<S>>,
    _not_sync: PhantomData<Cell<&'session ()>>,
}

impl<S: Signal> SubmissionQueue<'_, S> {
    /// Reserves a completion slot and queues a command without ringing the engine.
    pub fn push(&mut self, command: Command) -> Result<RequestId, SubmissionRefusal> {
        self.pair.try_push(command)
    }

    /// Rings the submission doorbell once for one or more queued commands.
    pub fn notify(&mut self) -> io::Result<()> {
        self.pair.check_engine_failure()?;
        self.submission_bell.ring()
    }

    /// Queues one command and rings the submission doorbell.
    pub fn submit(&mut self, command: Command) -> Result<RequestId, SubmitError> {
        let request = self.push(command)?;
        self.notify()?;
        Ok(request)
    }
}

impl<S: Signal> Drop for SubmissionQueue<'_, S> {
    fn drop(&mut self) {
        self.pair.close();
    }
}

/// The mutable consumer end of a queue pair.
///
/// It is `Send` but not `Sync`; all completion mutation takes `&mut self`.
///
/// ```compile_fail
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<canopy::ring::CompletionQueue<'static>>();
/// ```
pub struct CompletionQueue<'session, S: Signal = acadia::PlatformSignal> {
    pair: Arc<PairInner<S>>,
    _not_sync: PhantomData<Cell<&'session ()>>,
}

impl<S: Signal + 'static> CompletionQueue<'_, S> {
    /// Takes one completion if one is already available.
    #[must_use]
    pub fn try_pop(&mut self) -> Option<Completion> {
        self.pair.pop_completion()
    }

    /// Waits until a completion is available or `timeout` passes.
    ///
    /// `None` is only a wait bound: it does not withdraw, cancel or otherwise classify accepted
    /// work. A completion may still arrive on a later take or wait (`AtMostOneDelivery`).
    pub fn wait(&mut self, timeout: Duration) -> io::Result<Option<Completion>> {
        let mut drained = VecDeque::new();
        let bell = self.pair.completion_bell();
        let found = bell.pump(timeout, || {
            let Some(taken) = self.pair.pop_completion() else {
                return false;
            };
            drained.push_back(taken);
            true
        })?;
        if !found {
            self.pair.check_engine_failure()?;
            return Ok(None);
        }
        let completion = drained.pop_front();
        self.pair.put_completions_front(drained);
        Ok(completion)
    }

    /// Registers the completion readiness with the platform's thread pool.
    pub fn watch<T, F>(&mut self, mut take: T, failed: F) -> io::Result<Watch>
    where
        T: FnMut(Completion) + Send + 'static,
        F: Fn(io::Error) + Send + Sync + 'static,
    {
        let pair = Arc::clone(&self.pair);
        let bell = pair.completion_bell();
        bell.watch(
            move || {
                let completions = pair.take_completions();
                let found = !completions.is_empty();
                for completion in completions {
                    take(completion);
                }
                found
            },
            failed,
        )
    }
}

impl<S: Signal> Drop for CompletionQueue<'_, S> {
    fn drop(&mut self) {
        self.pair.close();
    }
}

/// A local submission or doorbell error.
#[derive(Debug)]
pub enum SubmitError {
    /// The command was not accepted locally.
    Refused(SubmissionRefusal),
    /// The submission doorbell could not be rung.
    Doorbell(io::Error),
}

impl fmt::Display for SubmitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SubmitError::Refused(error) => error.fmt(formatter),
            SubmitError::Doorbell(error) => {
                write!(formatter, "submission doorbell failed: {error}")
            }
        }
    }
}

impl std::error::Error for SubmitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SubmitError::Refused(error) => Some(error),
            SubmitError::Doorbell(error) => Some(error),
        }
    }
}

impl From<SubmissionRefusal> for SubmitError {
    fn from(error: SubmissionRefusal) -> Self {
        SubmitError::Refused(error)
    }
}

impl From<io::Error> for SubmitError {
    fn from(error: io::Error) -> Self {
        SubmitError::Doorbell(error)
    }
}

struct Engine<S: Signal> {
    store: Store,
    submission_bell: Arc<Doorbell<S>>,
    pairs: Mutex<Vec<Arc<PairInner<S>>>>,
    cursor: Mutex<usize>,
    failure: Mutex<Option<String>>,
}

impl<S: Signal + 'static> Engine<S> {
    fn ring(&self) {
        let _ = self.submission_bell.ring();
    }

    fn wake_pair(&self, pair: u64) {
        let pair = self
            .pairs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|candidate| candidate.id == pair)
            .cloned();
        if let Some(pair) = pair {
            let mut state = pair.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.live {
                state.parked = false;
                state.notice.take();
            }
        }
        self.ring();
    }

    fn remember_failure(&self, error: io::Error) {
        *self.failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(error.to_string());
    }

    fn try_take(&self) -> Option<(Arc<PairInner<S>>, Queued)> {
        let pairs = self.pairs.lock().unwrap_or_else(PoisonError::into_inner);
        if pairs.is_empty() {
            return None;
        }
        let mut cursor = self.cursor.lock().unwrap_or_else(PoisonError::into_inner);
        for offset in 0..pairs.len() {
            let index = (*cursor + offset) % pairs.len();
            let pair = Arc::clone(&pairs[index]);
            let queued = {
                let mut state = pair.state.lock().unwrap_or_else(PoisonError::into_inner);
                if state.live && !state.parked && !state.in_flight && !state.queued.is_empty() {
                    let queued = state.queued.pop_front().expect("non-empty queue");
                    state.in_flight = true;
                    Some(queued)
                } else {
                    None
                }
            };
            if let Some(queued) = queued {
                *cursor = (index + 1) % pairs.len();
                return Some((pair, queued));
            }
        }
        None
    }

    fn park(self: &Arc<Self>, pair: &Arc<PairInner<S>>, queued: Queued) -> Result<(), StoreError> {
        let weak = Arc::downgrade(self);
        let pair_id = pair.id;
        {
            let mut state = pair.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.queued.push_front(queued);
            state.in_flight = false;
            state.parked = true;
        }
        let deadline = Instant::now()
            .checked_add(NOTIFICATION_RECHECK)
            .unwrap_or_else(Instant::now);
        let notice = match self.store.notify_next_modification(
            deadline,
            Box::new(move || {
                if let Some(engine) = weak.upgrade() {
                    engine.wake_pair(pair_id);
                }
            }),
        ) {
            Ok(notice) => notice,
            Err(error) => {
                let mut state = pair.state.lock().unwrap_or_else(PoisonError::into_inner);
                state.parked = false;
                state.queued.pop_front();
                return Err(error);
            }
        };
        let mut state = pair.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.parked {
            state.notice = Some(notice);
        }
        Ok(())
    }

    fn complete(
        &self,
        pair: &Arc<PairInner<S>>,
        request: RequestId,
        outcome: Result<CommandValue, StoreError>,
    ) {
        let bell = {
            let mut state = pair.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.in_flight = false;
            if state.live {
                state.completions.push_back(Completion { request, outcome });
                Some(Arc::clone(&state.completion_bell))
            } else {
                state.outstanding = state.outstanding.saturating_sub(1);
                None
            }
        };
        if let Some(bell) = bell {
            let _ = bell.ring();
        }
    }

    fn execute(
        self: &Arc<Self>,
        pair: &Arc<PairInner<S>>,
        queued: Queued,
    ) -> Result<(), StoreError> {
        let command = match &queued.command {
            Command::Transaction(transaction, options) => {
                let Some(outcome) = self.store.try_transact(transaction, *options)? else {
                    self.park(pair, queued)?;
                    return Ok(());
                };
                CommandValue::Transaction(outcome)
            }
            Command::ReadDocument(node, document) => {
                CommandValue::Read(self.store.read_document(node.clone(), document)?)
            }
            Command::ListChildren(path) => {
                CommandValue::Children(self.store.list_children(path.clone())?)
            }
            Command::ListDocuments => CommandValue::Documents(self.store.list_documents()?),
            Command::Snapshot(command) => CommandValue::Snapshot(match command {
                SnapshotCommand::ReadDocument(node, document) => SnapshotValue::Read(
                    self.store
                        .snapshot(|view| view.read_document(node.clone(), document))??,
                ),
                SnapshotCommand::ListChildren(node) => SnapshotValue::Children(
                    self.store
                        .snapshot(|view| view.list_children(node.clone()))??,
                ),
                SnapshotCommand::ListDocuments => {
                    SnapshotValue::Documents(self.store.snapshot(|view| view.list_documents())??)
                }
            }),
        };
        self.complete(pair, queued.id, Ok(command));
        Ok(())
    }

    fn drain_once(self: &Arc<Self>) -> bool {
        let Some((pair, queued)) = self.try_take() else {
            return false;
        };
        let request = queued.id;
        if let Err(error) = self.execute(&pair, queued) {
            self.complete(&pair, request, Err(error));
        }
        true
    }
}

/// An in-process asynchronous store session.
///
/// A session owns the engine watch and is borrowed by each queue end, so no end can outlive it.
pub struct Session<S: Signal = acadia::PlatformSignal> {
    engine: Arc<Engine<S>>,
    _watch: Watch,
    next_pair: Mutex<u64>,
}

impl<S: Signal + 'static> Session<S> {
    /// Starts a session on `store` over the selected sealed signal.
    pub fn new(store: Store) -> io::Result<Self> {
        let submission_bell = Arc::new(Doorbell::<S>::new()?);
        let engine = Arc::new(Engine {
            store,
            submission_bell: Arc::clone(&submission_bell),
            pairs: Mutex::new(Vec::new()),
            cursor: Mutex::new(0),
            failure: Mutex::new(None),
        });
        let worker = Arc::downgrade(&engine);
        let failed = Arc::downgrade(&engine);
        let watch = submission_bell.watch(
            move || worker.upgrade().is_some_and(|engine| engine.drain_once()),
            move |error| {
                if let Some(engine) = failed.upgrade() {
                    engine.remember_failure(error);
                }
            },
        )?;
        Ok(Self {
            engine,
            _watch: watch,
            next_pair: Mutex::new(1),
        })
    }

    /// Creates one submission/completion queue pair with fixed completion capacity.
    pub fn queue_pair(
        &self,
        capacity: usize,
    ) -> io::Result<(SubmissionQueue<'_, S>, CompletionQueue<'_, S>)> {
        if capacity == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a queue pair must reserve at least one completion slot",
            ));
        }
        let id = {
            let mut next = self
                .next_pair
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let id = *next;
            *next = next.saturating_add(1);
            id
        };
        let pair = Arc::new(PairInner {
            id,
            engine: Arc::downgrade(&self.engine),
            state: Mutex::new(PairState {
                live: true,
                next_request: 1,
                outstanding: 0,
                capacity,
                queued: VecDeque::new(),
                completions: VecDeque::new(),
                in_flight: false,
                parked: false,
                notice: None,
                completion_bell: Arc::new(Doorbell::<S>::new()?),
            }),
        });
        self.engine
            .pairs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::clone(&pair));
        Ok((
            SubmissionQueue {
                pair: Arc::clone(&pair),
                submission_bell: Arc::clone(&self.engine.submission_bell),
                _not_sync: PhantomData,
            },
            CompletionQueue {
                pair,
                _not_sync: PhantomData,
            },
        ))
    }

    /// Submits one command to a temporary pair and waits for its completion.
    pub fn submit_and_wait(
        &self,
        command: Command,
        timeout: Duration,
    ) -> Result<(RequestId, Result<CommandValue, StoreError>), SessionError> {
        let (mut submissions, mut completions) = self.queue_pair(1)?;
        let request = submissions.submit(command)?;
        let completion = completions.wait(timeout)?.ok_or(SessionError::Timeout)?;
        debug_assert_eq!(completion.request(), request);
        Ok(completion.into_parts())
    }

    /// Reads one document through a one-command queue pair.
    pub fn read_document(
        &self,
        node: NodePath,
        document: DocumentName,
        timeout: Duration,
    ) -> Result<(RequestId, Read), SessionError> {
        let (request, outcome) =
            self.submit_and_wait(Command::ReadDocument(node, document), timeout)?;
        match outcome? {
            CommandValue::Read(read) => Ok((request, read)),
            _ => unreachable!("a read command returns a read value"),
        }
    }

    /// Lists one node through a one-command queue pair.
    pub fn list_children(
        &self,
        node: NodePath,
        timeout: Duration,
    ) -> Result<(RequestId, Children), SessionError> {
        let (request, outcome) = self.submit_and_wait(Command::ListChildren(node), timeout)?;
        match outcome? {
            CommandValue::Children(children) => Ok((request, children)),
            _ => unreachable!("a list-children command returns children"),
        }
    }

    /// Enumerates all documents through a one-command queue pair.
    pub fn list_documents(
        &self,
        timeout: Duration,
    ) -> Result<(RequestId, Vec<(NodePath, DocumentName)>), SessionError> {
        let (request, outcome) = self.submit_and_wait(Command::ListDocuments, timeout)?;
        match outcome? {
            CommandValue::Documents(documents) => Ok((request, documents)),
            _ => unreachable!("a list-documents command returns documents"),
        }
    }

    /// Runs one fixed snapshot command and waits for it.
    pub fn snapshot(
        &self,
        command: SnapshotCommand,
        timeout: Duration,
    ) -> Result<(RequestId, SnapshotValue), SessionError> {
        let (request, outcome) = self.submit_and_wait(Command::Snapshot(command), timeout)?;
        match outcome? {
            CommandValue::Snapshot(value) => Ok((request, value)),
            _ => unreachable!("a snapshot command returns a snapshot value"),
        }
    }

    /// Submits an opaque transaction once and waits for its bounded command outcome.
    ///
    /// This helper does not retry a rejected transaction. A timeout or pair loss does not justify
    /// replaying the transaction unless the application supplies an idempotency token or equivalent
    /// durable evidence. After an ambiguous post-commit failure, a later read is not universal proof
    /// of the request's effect (`LostCompletionIsUnknown`).
    pub fn transact(
        &self,
        transaction: Transaction,
        timeout: Duration,
    ) -> Result<(RequestId, WriteOutcome<Commit>), SessionError> {
        let (request, outcome) = self.submit_and_wait(
            Command::Transaction(transaction, TransactionOptions),
            timeout,
        )?;
        match outcome? {
            CommandValue::Transaction(outcome) => Ok((request, outcome)),
            _ => unreachable!("a transaction command returns a transaction value"),
        }
    }
}

/// Failure creating, submitting through, or waiting on a session.
#[derive(Debug)]
pub enum SessionError {
    /// A temporary pair could not be created or a wait doorbell failed.
    Io(io::Error),
    /// Local queue refusal while submitting.
    Submission(SubmitError),
    /// The bounded synchronous wait expired.
    ///
    /// The accepted command was not cancelled and may have completed or may still be running.
    Timeout,
    /// The command completed with a store error.
    Store(StoreError),
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::Io(error) => write!(formatter, "session I/O failed: {error}"),
            SessionError::Submission(error) => error.fmt(formatter),
            SessionError::Timeout => formatter.write_str("the session command timed out"),
            SessionError::Store(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SessionError::Io(error) => Some(error),
            SessionError::Submission(error) => Some(error),
            SessionError::Timeout => None,
            SessionError::Store(error) => Some(error),
        }
    }
}

impl From<io::Error> for SessionError {
    fn from(error: io::Error) -> Self {
        SessionError::Io(error)
    }
}

impl From<SubmitError> for SessionError {
    fn from(error: SubmitError) -> Self {
        SessionError::Submission(error)
    }
}

impl From<StoreError> for SessionError {
    fn from(error: StoreError) -> Self {
        SessionError::Store(error)
    }
}
