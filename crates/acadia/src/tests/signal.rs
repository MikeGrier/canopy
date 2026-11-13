// Copyright (c) Michael Grier.

//! `Signal`: every implementation is a level that is raised, lowered and waited on, and the
//! platform's own wait on its native object sees the same level.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex, mpsc};
use std::time::{Duration, Instant};

use test_timers::mpsc::recv_timeout;
use test_timers::thread::sleep;

use crate::{CondvarSignal, PlatformSignal, Signal, Watch};

/// Long enough that a wait the test expects to be woken never reaches it.
const LONG: Duration = Duration::from_secs(30);

/// Runs `check` against every implementation this target has.
fn on_every_signal(check: fn(&dyn Fn() -> Box<dyn Probe>)) {
    check(&|| Box::new(CondvarSignal::new().unwrap()));
    check(&|| Box::new(PlatformSignal::new().unwrap()));
}

/// `Signal` is not object-safe, so the tests reach every implementation through this.
trait Probe: Send + Sync {
    fn raise(&self);
    fn lower(&self);
    fn wait(&self, timeout: Duration) -> bool;
    fn watch(&self, callback: Box<dyn Fn() + Send + Sync>) -> Watch;
}

impl<S: Signal> Probe for S {
    fn raise(&self) {
        Signal::raise(self).unwrap();
    }
    fn lower(&self) {
        Signal::lower(self).unwrap();
    }
    fn wait(&self, timeout: Duration) -> bool {
        Signal::wait(self, timeout).unwrap()
    }
    fn watch(&self, callback: Box<dyn Fn() + Send + Sync>) -> Watch {
        Signal::watch(self, callback).unwrap()
    }
}

/// Long enough for a watch that should run to have run, so that one that has not is not going to.
const SETTLE: Duration = Duration::from_millis(30);

#[test]
fn a_new_signal_is_lowered() {
    on_every_signal(|make| {
        let signal = make();
        assert!(!signal.wait(Duration::ZERO));
    });
}

#[test]
fn a_raised_signal_stays_raised_through_any_number_of_waits() {
    on_every_signal(|make| {
        let signal = make();
        signal.raise();
        for _ in 0..3 {
            assert!(
                signal.wait(Duration::ZERO),
                "a wait never lowers the signal"
            );
        }
    });
}

#[test]
fn a_raise_before_the_wait_returns_at_once() {
    on_every_signal(|make| {
        let signal = make();
        signal.raise();
        let started = Instant::now();
        assert!(signal.wait(LONG));
        assert!(started.elapsed() < LONG / 2);
    });
}

#[test]
fn lowering_ends_the_raise() {
    on_every_signal(|make| {
        let signal = make();
        signal.raise();
        signal.lower();
        assert!(!signal.wait(Duration::ZERO));
    });
}

#[test]
fn it_is_a_level_and_not_a_count() {
    // Two raises and one lower leave it lowered: an `eventfd` read takes the whole counter.
    on_every_signal(|make| {
        let signal = make();
        signal.raise();
        signal.raise();
        signal.lower();
        assert!(!signal.wait(Duration::ZERO));
    });
}

#[test]
fn lowering_a_lowered_signal_and_raising_a_raised_one_change_nothing() {
    on_every_signal(|make| {
        let signal = make();
        signal.lower();
        signal.lower();
        assert!(!signal.wait(Duration::ZERO));
        signal.raise();
        signal.raise();
        assert!(signal.wait(Duration::ZERO));
    });
}

#[test]
fn a_bounded_wait_expires_no_sooner_than_its_bound() {
    on_every_signal(|make| {
        let signal = make();
        let bound = Duration::from_millis(30);
        let started = Instant::now();
        assert!(!signal.wait(bound));
        assert!(started.elapsed() >= bound, "{:?}", started.elapsed());
    });
}

#[test]
fn a_raise_on_another_thread_wakes_the_waiter() {
    on_every_signal(|make| {
        let signal = make();
        let ready = Barrier::new(2);
        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| {
                ready.wait();
                let started = Instant::now();
                (signal.wait(LONG), started.elapsed())
            });
            ready.wait();
            signal.raise();
            let (raised, waited) = waiter.join().unwrap();
            assert!(raised);
            assert!(waited < LONG / 2, "woken by the raise, not the bound");
        });
    });
}

#[test]
fn one_raise_wakes_every_waiter() {
    on_every_signal(|make| {
        let signal = make();
        let ready = Barrier::new(5);
        std::thread::scope(|scope| {
            let waiters: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        ready.wait();
                        signal.wait(LONG)
                    })
                })
                .collect();
            ready.wait();
            signal.raise();
            for waiter in waiters {
                assert!(waiter.join().unwrap());
            }
        });
    });
}

#[test]
fn many_raises_and_lowers_keep_the_level_exact() {
    on_every_signal(|make| {
        let signal = make();
        for round in 0..500u32 {
            let raises = round % 3;
            for _ in 0..raises {
                signal.raise();
            }
            assert_eq!(signal.wait(Duration::ZERO), raises > 0, "round {round}");
            signal.lower();
            assert!(!signal.wait(Duration::ZERO), "round {round}");
        }
    });
}

#[test]
fn a_signal_raised_by_one_thread_is_lowered_by_another() {
    on_every_signal(|make| {
        let signal = make();
        std::thread::scope(|scope| {
            scope.spawn(|| signal.raise()).join().unwrap();
            assert!(signal.wait(Duration::ZERO));
            scope.spawn(|| signal.lower()).join().unwrap();
            assert!(!signal.wait(Duration::ZERO));
        });
    });
}

#[cfg(windows)]
#[test]
fn the_platform_wait_on_the_event_sees_the_same_level() {
    use std::os::windows::io::AsHandle;
    use win_sync_sys::{WaitOutcome, wait_for_single_object};

    let signal = crate::EventSignal::new().unwrap();
    let wait = |millis| {
        wait_for_single_object(signal.as_handle(), Some(Duration::from_millis(millis))).unwrap()
    };
    assert_eq!(wait(0), WaitOutcome::TimedOut);
    Signal::raise(&signal).unwrap();
    assert_eq!(wait(0), WaitOutcome::Signalled);
    assert_eq!(
        wait(0),
        WaitOutcome::Signalled,
        "the platform's wait does not lower it either"
    );
    Signal::lower(&signal).unwrap();
    assert_eq!(wait(0), WaitOutcome::TimedOut);

    std::thread::scope(|scope| {
        let raiser = scope.spawn(|| Signal::raise(&signal).unwrap());
        assert_eq!(wait(30_000), WaitOutcome::Signalled);
        raiser.join().unwrap();
    });

    signal.event().set().unwrap();
    assert!(
        Signal::wait(&signal, Duration::ZERO).unwrap(),
        "the event lent is the signal's own"
    );
}

#[cfg(windows)]
#[test]
fn a_wait_only_handle_sees_the_signals_level() {
    use std::os::windows::io::{AsHandle, AsRawHandle};
    use win_sync_sys::{WaitOutcome, wait_for_single_object};

    let signal = crate::EventSignal::new().unwrap();
    let handle: crate::WaitOnlyEvent = signal.wait_only().unwrap();
    let wait = |millis| {
        wait_for_single_object(handle.as_handle(), Some(Duration::from_millis(millis))).unwrap()
    };
    assert_ne!(
        handle.as_handle().as_raw_handle(),
        signal.as_handle().as_raw_handle(),
        "a handle of its own, so its access is its own"
    );
    assert_eq!(wait(0), WaitOutcome::TimedOut);
    Signal::raise(&signal).unwrap();
    assert_eq!(wait(0), WaitOutcome::Signalled);
    assert_eq!(wait(0), WaitOutcome::Signalled, "waiting does not lower it");
    Signal::lower(&signal).unwrap();
    assert_eq!(wait(0), WaitOutcome::TimedOut);

    std::thread::scope(|scope| {
        let raiser = scope.spawn(|| Signal::raise(&signal).unwrap());
        assert_eq!(wait(30_000), WaitOutcome::Signalled);
        raiser.join().unwrap();
    });

    drop(signal);
    assert_eq!(
        wait(0),
        WaitOutcome::Signalled,
        "the handle keeps the event open after the signal is gone"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn the_platform_wait_on_the_descriptor_sees_the_same_level() {
    use std::os::fd::{AsFd, AsRawFd};

    let signal = crate::EventFdSignal::new().unwrap();
    let fd = signal.as_fd().as_raw_fd();
    let poll = |millis| {
        let mut ready = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid `pollfd`, for the signal's descriptor, which outlives every poll.
        let n = unsafe { libc::poll(&mut ready, 1, millis) };
        assert!(n >= 0, "{}", std::io::Error::last_os_error());
        n == 1 && ready.revents & libc::POLLIN != 0
    };
    assert!(!poll(0));
    Signal::raise(&signal).unwrap();
    assert!(poll(0));
    assert!(poll(0), "the platform's wait does not lower it either");
    Signal::lower(&signal).unwrap();
    assert!(!poll(0));

    std::thread::scope(|scope| {
        let raiser = scope.spawn(|| Signal::raise(&signal).unwrap());
        assert!(poll(30_000));
        raiser.join().unwrap();
    });
}

#[test]
fn the_platform_signal_is_the_platforms_own() {
    let name = std::any::type_name::<PlatformSignal>();
    if cfg!(windows) {
        assert!(name.ends_with("EventSignal"), "{name}");
    } else if cfg!(target_os = "linux") {
        assert!(name.ends_with("EventFdSignal"), "{name}");
    } else {
        assert!(name.ends_with("CondvarSignal"), "{name}");
    }
}

/// Watches `signal` with a callback that lowers it, then counts the run and reports the count.
fn watch_counting(signal: &Arc<dyn Probe>) -> (Watch, Arc<AtomicUsize>, mpsc::Receiver<usize>) {
    let runs = Arc::new(AtomicUsize::new(0));
    let (report, reports) = mpsc::channel();
    let watch = {
        let lowering = Arc::clone(signal);
        let runs = Arc::clone(&runs);
        signal.watch(Box::new(move || {
            lowering.lower();
            let _ = report.send(runs.fetch_add(1, Ordering::SeqCst) + 1);
        }))
    };
    (watch, runs, reports)
}

#[test]
fn a_raise_runs_the_watch() {
    on_every_signal(|make| {
        let signal: Arc<dyn Probe> = Arc::from(make());
        let (_watch, _, reports) = watch_counting(&signal);
        signal.raise();
        assert_eq!(recv_timeout(&reports, LONG), Ok(1));
    });
}

#[test]
fn a_lowered_signal_does_not_run_the_watch() {
    on_every_signal(|make| {
        let signal: Arc<dyn Probe> = Arc::from(make());
        let (_watch, runs, reports) = watch_counting(&signal);
        sleep(SETTLE);
        assert_eq!(runs.load(Ordering::SeqCst), 0);
        signal.raise();
        assert_eq!(recv_timeout(&reports, LONG), Ok(1));
        sleep(SETTLE);
        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "one raise, lowered by the run, is one run"
        );
    });
}

#[test]
fn a_signal_raised_before_the_watch_runs_it() {
    on_every_signal(|make| {
        let signal: Arc<dyn Probe> = Arc::from(make());
        signal.raise();
        let (_watch, _, reports) = watch_counting(&signal);
        assert_eq!(recv_timeout(&reports, LONG), Ok(1));
    });
}

#[test]
fn a_callback_that_leaves_the_signal_raised_runs_again() {
    on_every_signal(|make| {
        let signal: Arc<dyn Probe> = Arc::from(make());
        let runs = Arc::new(AtomicUsize::new(0));
        let (report, reports) = mpsc::channel();
        let _watch = {
            let lowering = Arc::clone(&signal);
            let runs = Arc::clone(&runs);
            signal.watch(Box::new(move || {
                let run = runs.fetch_add(1, Ordering::SeqCst) + 1;
                if run == 3 {
                    lowering.lower();
                    let _ = report.send(run);
                }
            }))
        };
        signal.raise();
        assert_eq!(recv_timeout(&reports, LONG), Ok(3));
        sleep(SETTLE);
        assert_eq!(
            runs.load(Ordering::SeqCst),
            3,
            "lowered, it is not run again"
        );
    });
}

#[test]
fn runs_of_one_watch_never_overlap() {
    on_every_signal(|make| {
        let signal: Arc<dyn Probe> = Arc::from(make());
        let inside = AtomicBool::new(false);
        let overlapped = Arc::new(AtomicBool::new(false));
        let runs = AtomicUsize::new(0);
        let (report, reports) = mpsc::channel();
        let _watch = {
            let lowering = Arc::clone(&signal);
            let overlapped = Arc::clone(&overlapped);
            signal.watch(Box::new(move || {
                if inside.swap(true, Ordering::SeqCst) {
                    overlapped.store(true, Ordering::SeqCst);
                }
                sleep(Duration::from_millis(1));
                inside.store(false, Ordering::SeqCst);
                if runs.fetch_add(1, Ordering::SeqCst) + 1 == 20 {
                    lowering.lower();
                    let _ = report.send(());
                }
            }))
        };
        signal.raise();
        assert_eq!(recv_timeout(&reports, LONG), Ok(()));
        assert!(!overlapped.load(Ordering::SeqCst), "two runs at once");
    });
}

#[test]
fn many_raises_from_many_threads_each_lead_to_a_run_that_finds_them() {
    const THREADS: usize = 8;
    const EACH: usize = 100;
    on_every_signal(|make| {
        let signal: Arc<dyn Probe> = Arc::from(make());
        let published = Arc::new(AtomicUsize::new(0));
        let (report, reports) = mpsc::channel();
        let _watch = {
            let lowering = Arc::clone(&signal);
            let published = Arc::clone(&published);
            signal.watch(Box::new(move || {
                // Lowered before the work is read, so a raise after the read runs it again.
                lowering.lower();
                if published.load(Ordering::SeqCst) == THREADS * EACH {
                    let _ = report.send(());
                }
            }))
        };
        std::thread::scope(|scope| {
            for _ in 0..THREADS {
                scope.spawn(|| {
                    for _ in 0..EACH {
                        published.fetch_add(1, Ordering::SeqCst);
                        signal.raise();
                    }
                });
            }
        });
        assert_eq!(recv_timeout(&reports, LONG), Ok(()));
    });
}

#[test]
fn dropping_the_watch_waits_for_a_running_callback() {
    on_every_signal(|make| {
        let signal: Arc<dyn Probe> = Arc::from(make());
        let done = Arc::new(AtomicBool::new(false));
        let (entered, entries) = mpsc::channel();
        let watch = {
            let lowering = Arc::clone(&signal);
            let done = Arc::clone(&done);
            signal.watch(Box::new(move || {
                lowering.lower();
                let _ = entered.send(());
                sleep(Duration::from_millis(50));
                done.store(true, Ordering::SeqCst);
            }))
        };
        signal.raise();
        assert_eq!(recv_timeout(&entries, LONG), Ok(()));
        drop(watch);
        assert!(
            done.load(Ordering::SeqCst),
            "the drop returned before the run did"
        );
    });
}

#[test]
fn a_dropped_watch_is_not_run() {
    on_every_signal(|make| {
        let signal: Arc<dyn Probe> = Arc::from(make());
        let (watch, runs, reports) = watch_counting(&signal);
        signal.raise();
        assert_eq!(recv_timeout(&reports, LONG), Ok(1));
        drop(watch);
        signal.raise();
        sleep(SETTLE);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert!(signal.wait(Duration::ZERO), "nothing lowered the raise");
    });
}

#[test]
fn a_watch_dropped_by_its_own_callback_returns_and_is_not_run_again() {
    on_every_signal(|make| {
        let signal: Arc<dyn Probe> = Arc::from(make());
        let slot: Arc<Mutex<Option<Watch>>> = Arc::new(Mutex::new(None));
        let runs = Arc::new(AtomicUsize::new(0));
        let (report, reports) = mpsc::channel();
        let watch = {
            let slot = Arc::clone(&slot);
            let runs = Arc::clone(&runs);
            signal.watch(Box::new(move || {
                runs.fetch_add(1, Ordering::SeqCst);
                let watch = slot.lock().unwrap().take();
                drop(watch);
                let _ = report.send(());
            }))
        };
        *slot.lock().unwrap() = Some(watch);
        signal.raise();
        assert_eq!(recv_timeout(&reports, LONG), Ok(()));
        sleep(SETTLE);
        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "the signal is still raised, but the watch is gone"
        );
    });
}

#[test]
fn many_watches_of_many_signals_each_run() {
    const SIGNALS: usize = 16;
    on_every_signal(|make| {
        let signals: Vec<Arc<dyn Probe>> = (0..SIGNALS).map(|_| Arc::from(make())).collect();
        let watched: Vec<_> = signals.iter().map(watch_counting).collect();
        for signal in &signals {
            signal.raise();
        }
        for (_watch, _, reports) in &watched {
            assert_eq!(recv_timeout(reports, LONG), Ok(1));
        }
    });
}
