// Copyright (c) Michael Grier.

//! Each wrapper does what the call it stands in for does: it waits at least as long as asked, and a
//! wait that can end early ends early for the same reasons.

use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use super::{mpsc, thread, tokio as timers};

const SHORT: Duration = Duration::from_millis(20);

/// A runtime to drive the `tokio` wrappers. As with `tokio` itself, a sleep or interval is made inside
/// the runtime, so each test builds its future inside the `async` block it hands to `block_on`.
fn current_thread() -> ::tokio::runtime::Runtime {
    ::tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a current-thread runtime")
}

fn paused_current_thread() -> ::tokio::runtime::Runtime {
    ::tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .expect("a paused current-thread runtime")
}

#[test]
fn a_tokio_sleep_waits_at_least_as_long_as_asked() {
    let started = Instant::now();
    current_thread().block_on(async { timers::sleep(SHORT).await });
    assert!(started.elapsed() >= SHORT);
}

#[test]
fn a_tokio_sleep_of_nothing_is_ready_at_once() {
    // On a paused clock no time passes unless something waits for it, so "at once" is exact.
    paused_current_thread().block_on(async {
        let started = ::tokio::time::Instant::now();
        timers::sleep(Duration::ZERO).await;
        assert_eq!(started.elapsed(), Duration::ZERO);
    });
}

#[test]
fn a_tokio_sleep_can_be_raced_and_loses_to_what_is_ready() {
    let won = current_thread().block_on(async {
        ::tokio::select! {
            () = timers::sleep(Duration::from_secs(30)) => "sleep",
            () = std::future::ready(()) => "ready",
        }
    });
    assert_eq!(won, "ready");
}

#[test]
fn a_tokio_sleep_wins_a_race_against_what_never_finishes() {
    let won = current_thread().block_on(async {
        ::tokio::select! {
            () = timers::sleep(SHORT) => "sleep",
            () = std::future::pending::<()>() => "pending",
        }
    });
    assert_eq!(won, "sleep");
}

#[test]
fn an_interval_ticks_at_once_and_then_after_its_period() {
    // A paused clock only moves when every task waits, so no wall-clock bound is involved.
    paused_current_thread().block_on(async {
        let mut ticker = timers::interval(SHORT);
        let started = ::tokio::time::Instant::now();
        ticker.tick().await;
        assert_eq!(
            started.elapsed(),
            Duration::ZERO,
            "the first tick is immediate"
        );
        ticker.tick().await;
        assert!(started.elapsed() >= SHORT, "the second waits a period");
    });
}

#[test]
fn an_interval_ticks_repeatedly() {
    // Paused, the clock advances exactly to each deadline: four ticks are three periods.
    paused_current_thread().block_on(async {
        let period = Duration::from_millis(5);
        let mut ticker = timers::interval(period);
        let started = ::tokio::time::Instant::now();
        for _ in 0..4 {
            ticker.tick().await;
        }
        assert_eq!(started.elapsed(), period * 3);
    });
}

#[test]
fn a_thread_sleep_waits_at_least_as_long_as_asked() {
    let started = Instant::now();
    thread::sleep(SHORT);
    assert!(started.elapsed() >= SHORT);
}

#[test]
fn a_thread_sleep_of_nothing_returns() {
    thread::sleep(Duration::ZERO);
}

#[test]
fn a_park_with_a_timeout_returns_by_itself() {
    let started = Instant::now();
    thread::park_timeout(SHORT);
    // A park may wake spuriously, so all that is promised is that it returns; it must not hang.
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_park_with_a_timeout_ends_early_when_unparked() {
    let parked = std::thread::spawn(|| {
        let started = Instant::now();
        thread::park_timeout(Duration::from_secs(30));
        started.elapsed()
    });
    // Unparking before the park is fine too: the token is kept and the park returns at once.
    parked.thread().unpark();
    let waited = parked.join().expect("the parked thread");
    assert!(waited < Duration::from_secs(5), "waited {waited:?}");
}

#[test]
fn a_receive_with_a_timeout_returns_what_was_sent() {
    let (sender, receiver) = channel();
    sender.send(7u32).expect("send");
    assert_eq!(mpsc::recv_timeout(&receiver, SHORT), Ok(7));
}

#[test]
fn a_receive_with_a_timeout_returns_values_in_order() {
    let (sender, receiver) = channel();
    for value in 1..=3u32 {
        sender.send(value).expect("send");
    }
    let got: Vec<_> = (0..3)
        .map(|_| mpsc::recv_timeout(&receiver, SHORT).expect("a value"))
        .collect();
    assert_eq!(got, [1, 2, 3]);
}

#[test]
fn a_receive_with_a_timeout_times_out_when_nothing_is_sent() {
    let (_sender, receiver) = channel::<u32>();
    let started = Instant::now();
    assert_eq!(
        mpsc::recv_timeout(&receiver, SHORT),
        Err(RecvTimeoutError::Timeout)
    );
    assert!(started.elapsed() >= SHORT);
}

#[test]
fn a_receive_with_a_timeout_reports_a_dropped_sender() {
    let (sender, receiver) = channel::<u32>();
    drop(sender);
    assert_eq!(
        mpsc::recv_timeout(&receiver, Duration::from_secs(30)),
        Err(RecvTimeoutError::Disconnected)
    );
}

#[test]
fn a_receive_with_a_timeout_wakes_for_a_value_sent_while_it_waits() {
    let (sender, receiver) = channel();
    let sending = std::thread::spawn(move || {
        thread::sleep(SHORT);
        sender.send("late").expect("send");
    });
    assert_eq!(
        mpsc::recv_timeout(&receiver, Duration::from_secs(30)),
        Ok("late")
    );
    sending.join().expect("the sender");
}
