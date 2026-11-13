// Copyright (c) Michael Grier.

//! `ring::Doorbell`: every interleaving of a few producers and the consumer, step by step as
//! `spec/Doorbell.tla` names them over a condition-variable signal, checked against
//! `spec/DoorbellContract.tla`; the same checker finding the lost wake of each of the spec's
//! controls; and the doorbell over the platform's own signal, pumped and waited on natively.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use acadia::{CondvarSignal, PlatformSignal};

use crate::ring::Doorbell;

/// The order of the consumer's steps, or the rule a control breaks, as `Doorbell.tla` has them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Protocol {
    Correct,
    ClearBeforeReset,
    SkipCheck,
    CheckBeforeClear,
    TestBeforePublish,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Producer {
    Publish,
    Test,
    PublishAfterTest { raise: bool },
    Raise,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Consumer {
    Take,
    Reset,
    Clear,
    Check,
    Retake,
    Waiting,
}

/// One state of the scripted run: the doorbell and its queue's counts, and where each actor is.
struct Run {
    protocol: Protocol,
    doorbell: Doorbell<CondvarSignal>,
    published: usize,
    taken: usize,
    producers: Vec<Producer>,
    consumer: Consumer,
    rounds: usize,
}

/// Who moves next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Actor {
    Producer(usize),
    Consumer,
}

/// How many times the consumer may wait and be woken in one schedule, which bounds it.
const ROUNDS: usize = 3;

impl Run {
    fn new(protocol: Protocol, producers: usize) -> Run {
        let first = if protocol == Protocol::TestBeforePublish {
            Producer::Test
        } else {
            Producer::Publish
        };
        Run {
            protocol,
            doorbell: Doorbell::new().unwrap(),
            published: 0,
            taken: 0,
            producers: vec![first; producers],
            consumer: Consumer::Take,
            rounds: 0,
        }
    }

    fn raised(&self) -> bool {
        self.doorbell.wait(Duration::ZERO).unwrap()
    }

    /// The actors with a step to take: a producer not done, and the consumer unless it waits
    /// with the signal lowered or has used every round.
    fn enabled(&self) -> Vec<Actor> {
        let mut actors: Vec<Actor> = (0..self.producers.len())
            .filter(|&p| self.producers[p] != Producer::Done)
            .map(Actor::Producer)
            .collect();
        let woken = self.consumer == Consumer::Waiting && self.raised() && self.rounds < ROUNDS;
        if self.consumer != Consumer::Waiting || woken {
            actors.push(Actor::Consumer);
        }
        actors
    }

    fn step(&mut self, actor: Actor) {
        match actor {
            Actor::Producer(p) => self.producer_step(p),
            Actor::Consumer => self.consumer_step(),
        }
    }

    fn producer_step(&mut self, p: usize) {
        self.producers[p] = match self.producers[p] {
            Producer::Publish => {
                self.published += 1;
                Producer::Test
            }
            Producer::Test if self.protocol == Protocol::TestBeforePublish => {
                Producer::PublishAfterTest {
                    raise: self.doorbell.test(),
                }
            }
            Producer::Test => {
                if self.doorbell.test() {
                    Producer::Raise
                } else {
                    Producer::Done
                }
            }
            Producer::PublishAfterTest { raise } => {
                self.published += 1;
                if raise {
                    Producer::Raise
                } else {
                    Producer::Done
                }
            }
            Producer::Raise => {
                self.doorbell.raise().unwrap();
                Producer::Done
            }
            Producer::Done => unreachable!("a producer that is done takes no step"),
        };
    }

    fn consumer_step(&mut self) {
        use Consumer::*;
        let protocol = self.protocol;
        self.consumer = match (self.consumer, protocol) {
            (Waiting, _) => {
                self.rounds += 1;
                Take
            }
            (Take, _) => {
                self.taken = self.published;
                if protocol == Protocol::ClearBeforeReset {
                    Clear
                } else {
                    Reset
                }
            }
            (Reset, Protocol::ClearBeforeReset) => {
                self.doorbell.reset().unwrap();
                Check
            }
            (Reset, Protocol::CheckBeforeClear) => {
                self.doorbell.reset().unwrap();
                Check
            }
            (Reset, _) => {
                self.doorbell.reset().unwrap();
                Clear
            }
            (Clear, Protocol::ClearBeforeReset) => {
                self.doorbell.clear();
                Reset
            }
            (Clear, Protocol::SkipCheck | Protocol::CheckBeforeClear) => {
                self.doorbell.clear();
                Waiting
            }
            (Clear, _) => {
                self.doorbell.clear();
                Check
            }
            (Check, _) if self.taken < self.published => Retake,
            (Check, Protocol::CheckBeforeClear) => Clear,
            (Check, _) => Waiting,
            (Retake, _) => {
                self.taken = self.published;
                Check
            }
        };
    }

    /// At the end of a schedule, the contract's two guarantees: a waiter with work to take is
    /// woken, and once woken it takes every item.
    fn contract_holds(&self) -> bool {
        if self.consumer != Consumer::Waiting || self.taken == self.published {
            return true;
        }
        self.raised()
    }
}

/// Everything that distinguishes one state of a run from another: the doorbell's record and
/// signal as read from it, the queue's counts, and where each actor is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct State {
    rung: bool,
    raised: bool,
    published: usize,
    taken: usize,
    producers: Vec<Producer>,
    consumer: Consumer,
    rounds: usize,
}

impl Run {
    fn state(&self) -> State {
        State {
            rung: self.doorbell.is_rung(),
            raised: self.raised(),
            published: self.published,
            taken: self.taken,
            producers: self.producers.clone(),
            consumer: self.consumer,
            rounds: self.rounds,
        }
    }

    /// A run in `state`, its doorbell brought there by the doorbell's own steps.
    fn at(protocol: Protocol, state: &State) -> Run {
        let doorbell = Doorbell::<CondvarSignal>::new().unwrap();
        if state.rung {
            assert!(doorbell.test());
        }
        if state.raised {
            doorbell.raise().unwrap();
        }
        Run {
            protocol,
            doorbell,
            published: state.published,
            taken: state.taken,
            producers: state.producers.clone(),
            consumer: state.consumer,
            rounds: state.rounds,
        }
    }
}

/// Explores every state `producers` producers and the consumer can reach, and returns how many
/// there were and the first final state that broke the contract, if any.
fn explore(protocol: Protocol, producers: usize) -> (usize, Option<State>) {
    let initial = Run::new(protocol, producers).state();
    let mut seen = HashSet::from([initial.clone()]);
    let mut frontier = vec![initial];
    while let Some(state) = frontier.pop() {
        let run = Run::at(protocol, &state);
        let enabled = run.enabled();
        if enabled.is_empty() && !run.contract_holds() {
            return (seen.len(), Some(state));
        }
        for actor in enabled {
            let mut next = Run::at(protocol, &state);
            next.step(actor);
            let next = next.state();
            if seen.insert(next.clone()) {
                frontier.push(next);
            }
        }
    }
    (seen.len(), None)
}

#[test]
fn every_interleaving_of_one_producer_keeps_the_contract() {
    let (states, broken) = explore(Protocol::Correct, 1);
    assert_eq!(broken, None);
    assert!(states > 10, "{states}");
}

#[test]
fn every_interleaving_of_two_producers_keeps_the_contract() {
    let (states, broken) = explore(Protocol::Correct, 2);
    assert_eq!(broken, None);
    assert!(states > 100, "{states}");
}

#[test]
fn every_interleaving_of_three_producers_keeps_the_contract() {
    let (states, broken) = explore(Protocol::Correct, 3);
    assert_eq!(broken, None);
    assert!(states > 1_000, "{states}");
}

/// Each control of `Doorbell.tla` breaks one rule, and the checker finds the lost wake.
#[test]
fn each_broken_rule_loses_a_wake() {
    for protocol in [
        Protocol::ClearBeforeReset,
        Protocol::SkipCheck,
        Protocol::CheckBeforeClear,
        Protocol::TestBeforePublish,
    ] {
        let (_, broken) = explore(protocol, 2);
        assert!(broken.is_some(), "{protocol:?} kept the contract");
    }
}

#[test]
fn a_ring_while_rung_skips_its_raise_until_the_consumer_rearms() {
    let doorbell = Doorbell::<CondvarSignal>::new().unwrap();
    assert!(doorbell.test(), "the first ring raises");
    assert!(!doorbell.test(), "a ring while rung does not");
    doorbell.rearm().unwrap();
    assert!(doorbell.test(), "the first ring after a rearm raises again");
}

// -- Pumping, over every signal --------------------------------------------------------------

/// A queue the tests publish into: a count of items, taken all at once by the one consumer.
#[derive(Default)]
struct Queue {
    published: AtomicUsize,
}

impl Queue {
    fn publish(&self) {
        self.published.fetch_add(1, Ordering::SeqCst);
    }

    /// The consumer's take: everything published since `taken`.
    fn take(&self, taken: &mut usize) -> bool {
        let published = self.published.load(Ordering::SeqCst);
        let found = published > *taken;
        *taken = published;
        found
    }
}

const LONG: Duration = Duration::from_secs(30);

fn pump_until_all_taken<S: acadia::Signal + 'static>(producers: usize, items: usize) {
    let doorbell = Arc::new(Doorbell::<S>::new().unwrap());
    let queue = Arc::new(Queue::default());
    let total = producers * items;
    std::thread::scope(|scope| {
        for _ in 0..producers {
            let (doorbell, queue) = (Arc::clone(&doorbell), Arc::clone(&queue));
            scope.spawn(move || {
                for _ in 0..items {
                    queue.publish();
                    doorbell.ring().unwrap();
                }
            });
        }
        let mut taken = 0;
        while taken < total {
            assert!(
                doorbell.pump(LONG, || queue.take(&mut taken)).unwrap(),
                "a pump waited out its bound with {taken} of {total} taken"
            );
        }
        assert_eq!(taken, total);
    });
}

#[test]
fn a_pump_takes_everything_many_producers_publish() {
    pump_until_all_taken::<CondvarSignal>(4, 500);
    pump_until_all_taken::<PlatformSignal>(4, 500);
}

#[test]
fn a_pump_with_nothing_published_returns_at_its_bound() {
    let doorbell = Doorbell::<PlatformSignal>::new().unwrap();
    let bound = Duration::from_millis(20);
    let started = std::time::Instant::now();
    assert!(!doorbell.pump(bound, || false).unwrap());
    assert!(started.elapsed() >= bound);
    assert!(!doorbell.pump(Duration::ZERO, || false).unwrap());
}

#[test]
fn a_pump_after_a_ring_takes_at_once() {
    let doorbell = Doorbell::<PlatformSignal>::new().unwrap();
    let queue = Queue::default();
    queue.publish();
    doorbell.ring().unwrap();
    let mut taken = 0;
    assert!(
        doorbell
            .pump(Duration::ZERO, || queue.take(&mut taken))
            .unwrap()
    );
    assert_eq!(taken, 1);
    assert!(
        !doorbell.wait(Duration::ZERO).unwrap(),
        "the pump's rearm lowered the signal"
    );
}

// -- The platform's own wait ------------------------------------------------------------------

#[cfg(windows)]
fn native_wait(doorbell: &Doorbell<PlatformSignal>, timeout: Duration) -> bool {
    use std::os::windows::io::AsHandle;
    // SAFETY: the handle is only waited on.
    let handle = unsafe { doorbell.native_wait_handle() }.unwrap();
    acadia::wait_native(handle.as_handle(), timeout).unwrap()
}

#[cfg(target_os = "linux")]
fn native_wait(doorbell: &Doorbell<PlatformSignal>, timeout: Duration) -> bool {
    // SAFETY: the descriptor is only waited on.
    acadia::wait_native(unsafe { doorbell.native_fd() }, timeout).unwrap()
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn the_platforms_wait_on_the_doorbell_sees_it_rung_until_it_is_rearmed() {
    let doorbell = Doorbell::<PlatformSignal>::new().unwrap();
    assert!(!native_wait(&doorbell, Duration::ZERO));
    doorbell.ring().unwrap();
    assert!(native_wait(&doorbell, Duration::ZERO));
    assert!(
        native_wait(&doorbell, Duration::ZERO),
        "a native wait lowers nothing"
    );
    doorbell.rearm().unwrap();
    assert!(!native_wait(&doorbell, Duration::ZERO));
}

/// A consumer thread waits with the platform's own wait on the doorbell, and runs the consumer's
/// side as a pump that does not wait once the doorbell is seen rung.
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn a_thread_waiting_natively_takes_everything_producers_publish() {
    let doorbell = Arc::new(Doorbell::<PlatformSignal>::new().unwrap());
    let queue = Arc::new(Queue::default());
    let (producers, items) = (3, 300);
    let total = producers * items;
    std::thread::scope(|scope| {
        let pool = {
            let (doorbell, queue) = (Arc::clone(&doorbell), Arc::clone(&queue));
            scope.spawn(move || {
                let mut taken = 0;
                while taken < total {
                    assert!(native_wait(&doorbell, LONG), "the pool's wait timed out");
                    doorbell
                        .pump(Duration::ZERO, || queue.take(&mut taken))
                        .unwrap();
                }
                taken
            })
        };
        for _ in 0..producers {
            let (doorbell, queue) = (Arc::clone(&doorbell), Arc::clone(&queue));
            scope.spawn(move || {
                for _ in 0..items {
                    queue.publish();
                    doorbell.ring().unwrap();
                }
            });
        }
        assert_eq!(pool.join().unwrap(), total);
    });
}

// -- A thread pool's wait ---------------------------------------------------------------------

/// The consumer of a thread pool's wait on a doorbell: what each callback pumps, and where the
/// last one reports how much it took.
#[cfg(any(windows, target_os = "linux"))]
struct PoolConsumer {
    doorbell: Arc<Doorbell<PlatformSignal>>,
    queue: Arc<Queue>,
    total: usize,
    taken: std::sync::Mutex<usize>,
    done: std::sync::mpsc::Sender<std::io::Result<usize>>,
}

#[cfg(any(windows, target_os = "linux"))]
impl PoolConsumer {
    /// One callback's work: a pump that does not wait. Returns whether the wait must be
    /// registered again; when it need not be, the result has been sent.
    fn on_rung(&self) -> bool {
        let mut taken = self.taken.lock().unwrap();
        let pumped = self
            .doorbell
            .pump(Duration::ZERO, || self.queue.take(&mut taken));
        match pumped {
            Ok(_) if *taken < self.total => true,
            Ok(_) => {
                let _ = self.done.send(Ok(*taken));
                false
            }
            Err(e) => {
                let _ = self.done.send(Err(e));
                false
            }
        }
    }
}

/// Producers publishing `items` each into `queue` and ringing `doorbell`, on threads of `scope`.
#[cfg(any(windows, target_os = "linux"))]
fn produce<'scope>(
    scope: &'scope std::thread::Scope<'scope, '_>,
    doorbell: &Arc<Doorbell<PlatformSignal>>,
    queue: &Arc<Queue>,
    producers: usize,
    items: usize,
) {
    for _ in 0..producers {
        let (doorbell, queue) = (Arc::clone(doorbell), Arc::clone(queue));
        scope.spawn(move || {
            for _ in 0..items {
                queue.publish();
                doorbell.ring().unwrap();
            }
        });
    }
}

/// The Windows thread pool waits on the doorbell through the handle it lends that can only wait:
/// each callback pumps without waiting and arms the one-shot wait again, so no thread of the
/// test's own waits for a ring.
#[cfg(windows)]
#[test]
fn a_thread_pool_wait_on_the_doorbell_takes_everything_producers_publish() {
    use windows_threadpool_sys::wait::ThreadpoolWait;

    let doorbell = Arc::new(Doorbell::<PlatformSignal>::new().unwrap());
    let queue = Arc::new(Queue::default());
    let (producers, items) = (3, 300);
    let (done, finished) = std::sync::mpsc::channel();
    let consumer = PoolConsumer {
        doorbell: Arc::clone(&doorbell),
        queue: Arc::clone(&queue),
        total: producers * items,
        taken: std::sync::Mutex::new(0),
        done,
    };
    let wait = ThreadpoolWait::new(
        // SAFETY: the thread pool only waits on the handle, and the callback never reaches it.
        unsafe { doorbell.native_wait_handle() }.unwrap().into(),
        move |activation| {
            if consumer.on_rung() {
                activation.rearm(None);
            }
        },
        None,
    )
    .unwrap();
    wait.arm(None);

    std::thread::scope(|scope| produce(scope, &doorbell, &queue, producers, items));
    let outcome = test_timers::mpsc::recv_timeout(&finished, LONG);
    wait.stop_and_drain();
    drop(wait);
    let taken = outcome
        .expect("the pool's callbacks did not take everything")
        .unwrap();
    assert_eq!(taken, producers * items);
}

/// On Linux a thread pool waits on an `epoll` set: the doorbell's descriptor is registered
/// level-triggered, and a pool thread that sees it ready pumps without waiting.
#[cfg(target_os = "linux")]
#[test]
fn a_thread_pool_wait_on_the_doorbell_takes_everything_producers_publish() {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    let doorbell = Arc::new(Doorbell::<PlatformSignal>::new().unwrap());
    let queue = Arc::new(Queue::default());
    let (producers, items) = (3, 300);

    // SAFETY: plain system calls; the descriptor returned is owned at once.
    let epoll = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    assert!(epoll >= 0, "{}", std::io::Error::last_os_error());
    // SAFETY: `epoll` is a descriptor this test owns and nothing else closes.
    let epoll = unsafe { OwnedFd::from_raw_fd(epoll) };
    let mut interest = libc::epoll_event {
        events: libc::EPOLLIN as u32,
        u64: 0,
    };
    // SAFETY: the descriptor is only registered with `epoll`, which waits on it.
    let fd = unsafe { doorbell.native_fd() }.as_raw_fd();
    // SAFETY: both descriptors are open, and `interest` is a valid event.
    let added =
        unsafe { libc::epoll_ctl(epoll.as_raw_fd(), libc::EPOLL_CTL_ADD, fd, &mut interest) };
    assert_eq!(added, 0, "{}", std::io::Error::last_os_error());

    let (done, finished) = std::sync::mpsc::channel();
    let consumer = PoolConsumer {
        doorbell: Arc::clone(&doorbell),
        queue: Arc::clone(&queue),
        total: producers * items,
        taken: std::sync::Mutex::new(0),
        done,
    };
    std::thread::scope(|scope| {
        let epoll = &epoll;
        let consumer = &consumer;
        scope.spawn(move || {
            let timeout = i32::try_from(LONG.as_millis()).unwrap();
            loop {
                let mut ready = [libc::epoll_event { events: 0, u64: 0 }];
                // SAFETY: `ready` has room for the one event asked for.
                let n =
                    unsafe { libc::epoll_wait(epoll.as_raw_fd(), ready.as_mut_ptr(), 1, timeout) };
                if n < 0 {
                    let e = std::io::Error::last_os_error();
                    if e.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    let _ = consumer.done.send(Err(e));
                    return;
                }
                assert_eq!(n, 1, "the pool's wait timed out");
                if !consumer.on_rung() {
                    return;
                }
            }
        });
        produce(scope, &doorbell, &queue, producers, items);
    });
    let taken = test_timers::mpsc::recv_timeout(&finished, LONG)
        .expect("the pool thread did not take everything")
        .unwrap();
    assert_eq!(taken, producers * items);
}
