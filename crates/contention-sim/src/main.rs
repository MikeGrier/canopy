// Copyright (c) Michael Grier.

//! Runs canopy's contention experiments against the agreed wait budget and writes the report to
//! the path given as the first argument, or `.scratch/contention-sim.txt`.
//!
//! The workloads, the budget and the hold times are canopy's DESIGN-NOTES, "How long the
//! contract makes a caller wait". Run it optimised:
//!
//! ```text
//! cargo run -p contention-sim --release
//! ```

use contention_sim::baseline;
use contention_sim::dist::Dist;
use contention_sim::exclusion::{self, Arrivals, Discipline};
use contention_sim::gate;
use contention_sim::report::{self, Sink, Tee};
use contention_sim::rng::SplitMix64;
use contention_sim::snapshot;
use contention_sim::stats::{Replicated, fraction_above, percentile};

const SEED: u64 = 0x0C0F_FEE5;
const REPLICATIONS: u64 = 10;

/// A write's hold of exclusion with one pointer replica, fitted to the throughput measurement
/// on the development machine: median 37 ms, a 99th percentile about 1.6 times that, and one
/// write in about three hundred taking about eight times as long.
const HOLD: Dist = Dist::Spiky {
    median: 0.037,
    sigma: 0.2,
    spike_prob: 0.003,
    spike_factor: 8.0,
};
/// The same with recovery folded in: one hold in a hundred runs twenty times as long.
const HOLD_WITH_RECOVERY: Dist = Dist::Spiky {
    median: 0.037,
    sigma: 0.2,
    spike_prob: 0.01,
    spike_factor: 20.0,
};
/// How long a woken thread takes to run and take a lock handed to it.
const HANDOFF: f64 = 0.000_05;

/// The budget: a service write waits at most these multiples of the median hold at the 99th
/// and 99.9th percentiles.
const SERVICE_P99_HOLDS: f64 = 10.0;
const SERVICE_P999_HOLDS: f64 = 100.0;
/// A cooperative database's burst member waits at most this long at the 99th percentile.
const BURST_P99: f64 = 1.0;
/// A metadata store's snapshot fails with at most this probability.
const SNAPSHOT_FAILURE: f64 = 1e-6;
/// A monthly upgrade reaches its body within this long.
const UPGRADE_WITHIN: f64 = 60.0;

/// The processors of the target machine.
const CPUS: usize = 16;

struct Disc {
    name: &'static str,
    discipline: Discipline,
    barge: f64,
}

const DISCIPLINES: &[Disc] = &[
    Disc {
        name: "unordered",
        discipline: Discipline::Unordered,
        barge: 0.0,
    },
    Disc {
        name: "unordered+barge",
        discipline: Discipline::Unordered,
        barge: 0.5,
    },
    Disc {
        name: "fifo-in-process",
        discipline: Discipline::FifoWithinProcess,
        barge: 0.5,
    },
    Disc {
        name: "bypass<=4",
        discipline: Discipline::BoundedBypass { limit: 4 },
        barge: 0.5,
    },
    Disc {
        name: "fifo",
        discipline: Discipline::FifoGlobal,
        barge: 0.0,
    },
];

/// One configuration's metrics over every replication.
#[derive(Default)]
struct Summary {
    mean: Replicated,
    p99: Replicated,
    p999: Replicated,
    passed_p99: Replicated,
    passed_max: Replicated,
    over: Replicated,
    unserved: Replicated,
}

fn run(cfg: &exclusion::Config, over: f64) -> Summary {
    let mut summary = Summary::default();
    for r in 0..REPLICATIONS {
        let mut outcome = exclusion::simulate(cfg, SplitMix64::stream(SEED, r).next_u64());
        let n = outcome.waits.len().max(1) as f64;
        summary.mean.push(outcome.waits.iter().sum::<f64>() / n);
        summary.over.push(fraction_above(&outcome.waits, over));
        summary.p99.push(percentile(&mut outcome.waits, 99.0));
        summary.p999.push(percentile(&mut outcome.waits, 99.9));
        summary
            .passed_p99
            .push(percentile(&mut outcome.passed_over, 99.0));
        summary
            .passed_max
            .push(outcome.passed_over.iter().copied().fold(0.0, f64::max));
        summary.unserved.push(outcome.longest_unserved);
    }
    summary
}

fn ms(seconds: f64) -> String {
    format!("{:.0}", seconds * 1000.0)
}

fn with_ci(r: &Replicated) -> String {
    format!("{}+-{}", ms(r.mean()), ms(r.half_width()))
}

fn service(out: &mut dyn Sink) {
    let median = HOLD.median();
    let (p99_bound, p999_bound) = (SERVICE_P99_HOLDS * median, SERVICE_P999_HOLDS * median);
    out.line("## Service: one process, open Poisson requests, writer exclusion");
    out.line(&format!(
        "budget: p99 <= {} ms, p99.9 <= {} ms (10x and 100x the median hold of {} ms)",
        ms(p99_bound),
        ms(p999_bound),
        ms(median)
    ));
    out.line(
        "util  discipline        mean ms(PK)    p99 ms        p99.9 ms      passed p99/max  budget",
    );
    for &utilization in &[0.3, 0.5, 0.6, 0.7, 0.8, 0.9, 0.95] {
        let rate = baseline::rate_for(utilization, &HOLD);
        let pk = baseline::mean_wait(rate, &HOLD).unwrap_or(f64::NAN);
        for d in DISCIPLINES {
            let cfg = exclusion::Config {
                processes: 1,
                threads: 1,
                arrivals: Arrivals::Poisson { rate },
                hold: HOLD,
                discipline: d.discipline,
                barge: d.barge,
                handoff: HANDOFF,
                horizon: 3_600.0,
                warmup: 60.0,
            };
            let s = run(&cfg, p99_bound);
            let meets = s.p99.mean() <= p99_bound && s.p999.mean() <= p999_bound;
            out.line(&format!(
                "{utilization:<5} {:<17} {:>5}({:>5})  {:<13} {:<13} {:>4.0}/{:<9.0} {}",
                d.name,
                ms(s.mean.mean()),
                ms(pk),
                with_ci(&s.p99),
                with_ci(&s.p999),
                s.passed_p99.mean(),
                s.passed_max.max(),
                if meets { "meets" } else { "BREACH" }
            ));
        }
    }
    out.line("");
    out.line("recovery folded into the holds (1% of holds 20x the median), utilization 0.7:");
    let rate = baseline::rate_for(0.7, &HOLD);
    for d in DISCIPLINES {
        let cfg = exclusion::Config {
            processes: 1,
            threads: 1,
            arrivals: Arrivals::Poisson { rate },
            hold: HOLD_WITH_RECOVERY,
            discipline: d.discipline,
            barge: d.barge,
            handoff: HANDOFF,
            horizon: 3_600.0,
            warmup: 60.0,
        };
        let s = run(&cfg, p99_bound);
        out.line(&format!(
            "  {:<17} p99 {} ms, p99.9 {} ms, utilization now {:.2}",
            d.name,
            with_ci(&s.p99),
            with_ci(&s.p999),
            rate * HOLD_WITH_RECOVERY.mean()
        ));
    }
    out.line("");
    out.line("a callback under exclusion lengthens every hold; arrivals fixed at utilization 0.7 without one:");
    for &callback in &[0.0, 0.005, 0.02] {
        let hold = Dist::Spiky {
            median: 0.037 + callback,
            sigma: 0.2,
            spike_prob: 0.003,
            spike_factor: 8.0,
        };
        let cfg = exclusion::Config {
            processes: 1,
            threads: 1,
            arrivals: Arrivals::Poisson { rate },
            hold,
            discipline: Discipline::Unordered,
            barge: 0.5,
            handoff: HANDOFF,
            horizon: 3_600.0,
            warmup: 60.0,
        };
        let s = run(&cfg, p99_bound);
        out.line(&format!(
            "  callback {} ms: utilization {:.2}, unordered+barge p99 {} ms, p99.9 {} ms",
            ms(callback),
            rate * hold.mean(),
            with_ci(&s.p99),
            with_ci(&s.p999)
        ));
    }
    out.line("");
}

fn cooperative(out: &mut dyn Sink) {
    out.line("## Cooperative database: bursts of writers across processes");
    out.line(&format!(
        "budget: a burst member's p99 wait <= {} ms; {CPUS} processes, a burst every 120 s, 4 writes each, 50 ms think",
        ms(BURST_P99)
    ));
    out.line("burst  discipline        mean ms       p99 ms          p99.9 ms        passed p99/max  P(>1s)   budget");
    for &size in &[4, CPUS, 4 * CPUS, CPUS * CPUS] {
        for d in DISCIPLINES {
            let cfg = exclusion::Config {
                processes: CPUS,
                threads: 1,
                arrivals: Arrivals::Bursts {
                    period: 120.0,
                    size,
                    writes: 4,
                    think: Dist::Exponential { mean: 0.05 },
                },
                hold: HOLD,
                discipline: d.discipline,
                barge: d.barge,
                handoff: HANDOFF,
                horizon: 7_200.0,
                warmup: 0.0,
            };
            let s = run(&cfg, BURST_P99);
            out.line(&format!(
                "{size:<6} {:<17} {:<13} {:<15} {:<15} {:>4.0}/{:<9.0} {:<8.4} {}",
                d.name,
                with_ci(&s.mean),
                with_ci(&s.p99),
                with_ci(&s.p999),
                s.passed_p99.mean(),
                s.passed_max.max(),
                s.over.mean(),
                if s.p99.mean() <= BURST_P99 {
                    "meets"
                } else {
                    "BREACH"
                }
            ));
        }
    }
    out.line("");
}

fn metadata(out: &mut dyn Sink) {
    out.line("## Metadata store: snapshots under occasional writes");
    out.line(&format!(
        "budget: a snapshot fails with probability <= {SNAPSHOT_FAILURE:e}; a read takes 1 ms"
    ));
    out.line("writes/s     policy                  failure (sim)    failure (approx)  mean ms  longest ms  budget");
    let policies = [
        (
            "spin 16 x yield",
            snapshot::Policy::Spin {
                attempts: 16,
                gap: 0.000_02,
            },
        ),
        (
            "wait-even <= 1 s",
            snapshot::Policy::WaitEven { deadline: 1.0 },
        ),
    ];
    for &rate in &[1.0 / 3_600.0, 1.0 / 60.0, 1.0, 10.0] {
        for (name, policy) in policies {
            let mut failures = Replicated::new();
            let mut latency = Replicated::new();
            let mut longest = Replicated::new();
            for r in 0..REPLICATIONS {
                let cfg = snapshot::Config {
                    write_rate: rate,
                    hold: HOLD,
                    read: 0.001,
                    policy,
                    reads: 100_000,
                    horizon: 36_000.0,
                };
                let o = snapshot::simulate(&cfg, SplitMix64::stream(SEED, r).next_u64());
                failures.push(o.failure_rate());
                latency.push(o.mean_latency);
                longest.push(o.longest_latency);
            }
            let approx = match policy {
                snapshot::Policy::Spin { .. } => {
                    format!("{:.2e}", snapshot::spin_failure_approx(rate, &HOLD, 0.001))
                }
                snapshot::Policy::WaitEven { .. } => "-".to_string(),
            };
            let bound = failures.mean().max(match policy {
                snapshot::Policy::Spin { .. } => snapshot::spin_failure_approx(rate, &HOLD, 0.001),
                snapshot::Policy::WaitEven { .. } => 0.0,
            });
            out.line(&format!(
                "{rate:<12.5} {name:<23} {:<16.2e} {approx:<17} {:<8.2} {:<11.0} {}",
                failures.mean(),
                latency.mean() * 1000.0,
                longest.max() * 1000.0,
                if bound <= SNAPSHOT_FAILURE {
                    "meets"
                } else {
                    "BREACH"
                }
            ));
        }
    }
    out.line(&format!(
        "spinning meets the budget only below {:.1e} writes/s, about one write every {:.1} hours",
        SNAPSHOT_FAILURE / (HOLD.mean() + 0.001),
        (HOLD.mean() + 0.001) / SNAPSHOT_FAILURE / 3_600.0
    ));
    out.line("");

    out.line("## Metadata store: a monthly upgrade behind attaching handles");
    out.line(&format!(
        "budget: the upgrade body begins within {} s; gate held shared 2 ms per attach; 2000 upgrades each",
        UPGRADE_WITHIN
    ));
    out.line("attach/s  lifetime s  policy           gate p99 s   body p50 s   body p99 s   P(body>60s)  shared busy s  budget");
    for &rate in &[1.0, 10.0, 100.0, 1_000.0, 2_000.0] {
        for &lifetime in &[1.0, 60.0, 3_600.0] {
            for policy in [gate::Policy::Contract, gate::Policy::ExclusiveFirst] {
                let cfg = gate::Config {
                    attach_rate: rate,
                    gate_hold: 0.002,
                    mean_lifetime: lifetime,
                    policy,
                };
                let (mut gates, mut bodies): (Vec<f64>, Vec<f64>) = (0..2_000)
                    .map(|i| {
                        let o = gate::simulate(&cfg, SplitMix64::stream(SEED, i).next_u64());
                        (o.gate_wait, o.body_wait)
                    })
                    .unzip();
                let late = fraction_above(&bodies, UPGRADE_WITHIN);
                out.line(&format!(
                    "{rate:<9} {lifetime:<11} {:<16} {:<12.3} {:<12.1} {:<12.1} {:<12.3} {:<14.3} {}",
                    match policy {
                        gate::Policy::Contract => "contract",
                        gate::Policy::ExclusiveFirst => "exclusive-first",
                    },
                    percentile(&mut gates, 99.0),
                    percentile(&mut bodies, 50.0),
                    percentile(&mut bodies, 99.0),
                    late,
                    gate::mean_shared_busy_period(rate, 0.002),
                    if late <= 0.01 { "meets" } else { "BREACH" }
                ));
            }
        }
    }
    out.line("");
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| ".scratch/contention-sim.txt".to_string());
    let mut out = Tee::create(std::path::Path::new(&path))
        .unwrap_or_else(|error| report::fail(&format!("cannot write {path}: {error}")));
    out.line(&format!(
        "contention-sim: seed {SEED:#x}, {REPLICATIONS} replications, 95% intervals as +-"
    ));
    out.line("");
    service(&mut out);
    cooperative(&mut out);
    metadata(&mut out);
}
