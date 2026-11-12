// Copyright (c) Michael Grier.

//! One exclusive lock -- writer exclusion -- and the waits its discipline makes.
//!
//! A request either finds the lock free and takes it, or waits. When the holder releases, the
//! discipline chooses one waiter, and the lock is handed to it after `handoff`, the time a woken
//! thread takes to run. A request that arrives during that handoff barges ahead of the chosen
//! waiter with probability `barge`, as the operating system's locks allow; first-come-first-served
//! never lets it. A waiter is passed over each time the lock goes to a request that arrived after
//! it.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::dist::Dist;
use crate::rng::SplitMix64;

/// Who is served when the lock frees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Discipline {
    /// A waiter chosen uniformly at random: the order the contract allows.
    Unordered,
    /// The longest waiter, and no barging.
    FifoGlobal,
    /// A process chosen uniformly among those with waiters, then that process's longest waiter.
    FifoWithinProcess,
    /// As unordered, except that a waiter passed over `limit` times is served next, and no
    /// request barges ahead of it.
    BoundedBypass { limit: u32 },
}

/// Where requests come from.
#[derive(Clone, Copy, Debug)]
pub enum Arrivals {
    /// An open Poisson stream at `rate`, each request from a process chosen uniformly.
    Poisson { rate: f64 },
    /// Each of `processes * threads` threads thinks, requests, holds, and thinks again.
    Closed { think: Dist },
    /// Every `period`, `size` threads -- spread across the processes in turn -- each make
    /// `writes` requests, thinking before each.
    Bursts {
        period: f64,
        size: usize,
        writes: usize,
        think: Dist,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub processes: usize,
    pub threads: usize,
    pub arrivals: Arrivals,
    pub hold: Dist,
    pub discipline: Discipline,
    pub barge: f64,
    pub handoff: f64,
    pub horizon: f64,
    pub warmup: f64,
}

/// What one run observed, for requests that arrived after the warm-up.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    /// Each served request's wait.
    pub waits: Vec<f64>,
    /// How many times each served request was passed over.
    pub passed_over: Vec<f64>,
    /// The longest a request still waiting at the horizon had waited.
    pub longest_unserved: f64,
    /// The most times a request still waiting at the horizon had been passed over.
    pub most_passed_unserved: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    PoissonArrival,
    BurstStart,
    Arrive { client: usize },
    Release,
    Handoff { client: usize },
}

#[derive(Clone, Copy, Debug)]
struct Event {
    time: f64,
    seq: u64,
    kind: Kind,
}

impl PartialEq for Event {
    fn eq(&self, other: &Event) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Event {}

impl PartialOrd for Event {
    fn partial_cmp(&self, other: &Event) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Event {
    /// Reversed, so the heap yields the earliest event first, and among simultaneous events the
    /// one scheduled first.
    fn cmp(&self, other: &Event) -> Ordering {
        other
            .time
            .total_cmp(&self.time)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

#[derive(Clone, Copy, Debug)]
struct Waiter {
    client: usize,
    process: usize,
    arrived: f64,
    passed: u32,
}

struct Sim {
    cfg: Config,
    rng: SplitMix64,
    events: BinaryHeap<Event>,
    seq: u64,
    now: f64,
    holder: Option<usize>,
    pending: Option<usize>,
    waiting: Vec<Waiter>,
    process_of: Vec<usize>,
    remaining: Vec<usize>,
    outcome: Outcome,
}

/// Runs one replication of `cfg` from `seed`.
pub fn simulate(cfg: &Config, seed: u64) -> Outcome {
    let mut sim = Sim {
        cfg: *cfg,
        rng: SplitMix64::new(seed),
        events: BinaryHeap::new(),
        seq: 0,
        now: 0.0,
        holder: None,
        pending: None,
        waiting: Vec::new(),
        process_of: Vec::new(),
        remaining: Vec::new(),
        outcome: Outcome::default(),
    };
    sim.start();
    while let Some(event) = sim.events.pop() {
        if event.time > sim.cfg.horizon {
            break;
        }
        sim.now = event.time;
        sim.handle(event.kind);
    }
    sim.finish();
    sim.outcome
}

impl Sim {
    fn schedule(&mut self, delay: f64, kind: Kind) {
        self.seq += 1;
        self.events.push(Event {
            time: self.now + delay,
            seq: self.seq,
            kind,
        });
    }

    fn new_client(&mut self, process: usize, writes: usize) -> usize {
        self.process_of.push(process);
        self.remaining.push(writes);
        self.process_of.len() - 1
    }

    fn start(&mut self) {
        match self.cfg.arrivals {
            Arrivals::Poisson { rate } => {
                let gap = Dist::Exponential { mean: 1.0 / rate }.sample(&mut self.rng);
                self.schedule(gap, Kind::PoissonArrival);
            }
            Arrivals::Closed { think } => {
                for t in 0..self.cfg.processes * self.cfg.threads {
                    let client = self.new_client(t % self.cfg.processes, usize::MAX);
                    let delay = think.sample(&mut self.rng);
                    self.schedule(delay, Kind::Arrive { client });
                }
            }
            Arrivals::Bursts { .. } => self.schedule(0.0, Kind::BurstStart),
        }
    }

    fn handle(&mut self, kind: Kind) {
        match kind {
            Kind::PoissonArrival => {
                if let Arrivals::Poisson { rate } = self.cfg.arrivals {
                    let process = self.rng.below(self.cfg.processes);
                    let client = self.new_client(process, 1);
                    self.arrive(client);
                    let gap = Dist::Exponential { mean: 1.0 / rate }.sample(&mut self.rng);
                    self.schedule(gap, Kind::PoissonArrival);
                }
            }
            Kind::BurstStart => {
                if let Arrivals::Bursts {
                    period,
                    size,
                    writes,
                    think,
                } = self.cfg.arrivals
                {
                    for i in 0..size {
                        let client = self.new_client(i % self.cfg.processes, writes);
                        let delay = think.sample(&mut self.rng);
                        self.schedule(delay, Kind::Arrive { client });
                    }
                    self.schedule(period, Kind::BurstStart);
                }
            }
            Kind::Arrive { client } => self.arrive(client),
            Kind::Release => self.release(),
            Kind::Handoff { client } => {
                if self.pending == Some(client) && self.holder.is_none() {
                    self.pending = None;
                    if let Some(index) = self.waiting.iter().position(|w| w.client == client) {
                        self.grant_waiter(index);
                    }
                }
            }
        }
    }

    fn arrive(&mut self, client: usize) {
        let process = self.process_of[client];
        if self.holder.is_none() && self.pending.is_none() && self.waiting.is_empty() {
            self.grant(client, self.now, 0);
            return;
        }
        if self.holder.is_none() && self.pending.is_some() && self.may_barge() {
            self.pending = None;
            for waiter in &mut self.waiting {
                waiter.passed += 1;
            }
            self.grant(client, self.now, 0);
            return;
        }
        self.waiting.push(Waiter {
            client,
            process,
            arrived: self.now,
            passed: 0,
        });
    }

    fn may_barge(&mut self) -> bool {
        match self.cfg.discipline {
            Discipline::FifoGlobal => false,
            Discipline::BoundedBypass { limit }
                if self.waiting.iter().any(|w| w.passed >= limit) =>
            {
                false
            }
            _ => self.rng.chance(self.cfg.barge),
        }
    }

    fn release(&mut self) {
        let client = self.holder.take().expect("a release has a holder");
        self.next_request(client);
        if self.waiting.is_empty() {
            return;
        }
        let index = self.choose();
        if self.cfg.handoff > 0.0 {
            let chosen = self.waiting[index].client;
            self.pending = Some(chosen);
            self.schedule(self.cfg.handoff, Kind::Handoff { client: chosen });
        } else {
            self.grant_waiter(index);
        }
    }

    fn next_request(&mut self, client: usize) {
        let think = match self.cfg.arrivals {
            Arrivals::Poisson { .. } => return,
            Arrivals::Closed { think } => think,
            Arrivals::Bursts { think, .. } => {
                self.remaining[client] -= 1;
                if self.remaining[client] == 0 {
                    return;
                }
                think
            }
        };
        let delay = think.sample(&mut self.rng);
        self.schedule(delay, Kind::Arrive { client });
    }

    /// The index in `waiting`, which is in order of arrival, of the waiter served next.
    fn choose(&mut self) -> usize {
        match self.cfg.discipline {
            Discipline::FifoGlobal => 0,
            Discipline::Unordered => self.rng.below(self.waiting.len()),
            Discipline::FifoWithinProcess => {
                let mut processes: Vec<usize> = self.waiting.iter().map(|w| w.process).collect();
                processes.sort_unstable();
                processes.dedup();
                let process = processes[self.rng.below(processes.len())];
                self.waiting
                    .iter()
                    .position(|w| w.process == process)
                    .expect("the chosen process has a waiter")
            }
            Discipline::BoundedBypass { limit } => {
                match self.waiting.iter().position(|w| w.passed >= limit) {
                    Some(index) => index,
                    None => self.rng.below(self.waiting.len()),
                }
            }
        }
    }

    fn grant_waiter(&mut self, index: usize) {
        let waiter = self.waiting.remove(index);
        for other in &mut self.waiting {
            if other.arrived < waiter.arrived {
                other.passed += 1;
            }
        }
        self.grant(waiter.client, waiter.arrived, waiter.passed);
    }

    fn grant(&mut self, client: usize, arrived: f64, passed: u32) {
        if arrived >= self.cfg.warmup {
            self.outcome.waits.push(self.now - arrived);
            self.outcome.passed_over.push(f64::from(passed));
        }
        self.holder = Some(client);
        let hold = self.cfg.hold.sample(&mut self.rng);
        self.schedule(hold, Kind::Release);
    }

    fn finish(&mut self) {
        let horizon = self.cfg.horizon;
        for waiter in &self.waiting {
            if waiter.arrived >= self.cfg.warmup {
                self.outcome.longest_unserved =
                    self.outcome.longest_unserved.max(horizon - waiter.arrived);
                self.outcome.most_passed_unserved =
                    self.outcome.most_passed_unserved.max(waiter.passed);
            }
        }
    }
}
