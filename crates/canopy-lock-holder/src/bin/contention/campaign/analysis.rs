// Copyright (c) Michael Grier.

use super::*;
use contention_sim::stats::Replicated;
use std::collections::BTreeMap;

#[cfg(test)]
#[path = "analysis/tests.rs"]
mod tests;

fn mean(metric: &Replicated, count: usize) -> Option<f64> {
    (count > 0).then(|| metric.mean())
}

fn interval(metric: &Replicated, count: usize) -> Option<f64> {
    (count >= 2).then(|| metric.half_width())
}

fn summarize(rows: &[ResultRow]) -> serde_json::Value {
    let mut p99 = Replicated::new();
    let mut p999 = Replicated::new();
    let mut opening = Replicated::new();
    let mut closing = Replicated::new();
    let mut hold = Replicated::new();
    let mut bypass = Replicated::new();
    let mut snapshot_wait = Replicated::new();
    let mut snapshot_p99 = Replicated::new();
    let mut achieved = Replicated::new();
    let mut notice = Replicated::new();
    let mut upgrade = Replicated::new();
    let mut normalized_p99 = Replicated::new();
    let mut normalized_p999 = Replicated::new();
    let mut samples = 0;
    let mut write_reps = 0;
    let mut read_reps = 0;
    let mut notice_reps = 0;
    let mut upgrade_reps = 0;
    let mut requested = 0;
    let mut completed = 0;
    let mut unfinished = 0;
    let mut reads = 0;
    let mut failures = 0;
    let mut censored = 0;
    for row in rows {
        samples += row.samples.len();
        requested += row.requested;
        completed += row.completed_requests;
        unfinished += row.unfinished;
        reads += row.snapshot_reads;
        failures += row.snapshot_failures;
        censored += usize::from(row.upgrade_censored);
        if !row.samples.is_empty() {
            write_reps += 1;
            p99.push(row.p99_wait);
            p999.push(row.p999_wait);
            hold.push(row.median_hold);
            opening.push(row.opening_p50);
            closing.push(row.closing_p50);
            bypass.push(row.p99_passed);
            achieved.push(row.achieved_utilization);
            if row.median_hold > 0.0 {
                normalized_p99.push(row.p99_wait / row.median_hold);
                normalized_p999.push(row.p999_wait / row.median_hold);
            }
        }
        if row.snapshot_reads > 0 {
            read_reps += 1;
            snapshot_wait.push(row.snapshot_mean_wait);
            snapshot_p99.push(row.snapshot_p99_wait);
        }
        if !row.notices.is_empty() {
            notice_reps += 1;
            notice.push(row.notices.iter().sum::<f64>() / row.notices.len() as f64);
        }
        if let Some(wait) = row.upgrade_wait {
            upgrade_reps += 1;
            upgrade.push(wait);
        }
    }
    let service = rows
        .first()
        .is_some_and(|row| row.scenario.starts_with("service"));
    let burst = rows
        .first()
        .is_some_and(|row| row.scenario.starts_with("burst"));
    let observed = if service && write_reps > 0 {
        Some(normalized_p99.mean() > 10.0)
    } else if burst && write_reps > 0 {
        Some(p99.mean() > 1.0)
    } else {
        None
    };
    let all_tail_samples = !rows.is_empty() && rows.iter().all(|row| row.samples.len() >= 10000);
    let contradiction = service
        && rows.iter().all(|row| {
            row.configured_utilization.is_some_and(|util| util <= 0.6)
                && row.achieved_utilization <= 0.6
                && row.observation_seconds >= 60.0
                && row.samples.len() >= 10000
        })
        && write_reps >= 10
        && unfinished == 0
        && normalized_p99.mean() - normalized_p99.half_width() > 10.0;
    serde_json::json!({
        "replications": rows.len(), "observed_write_replications": write_reps,
        "completed_samples": samples, "requested": requested, "completed_requests": completed,
        "unfinished_requests": unfinished, "p99_mean_seconds": mean(&p99, write_reps), "p99_halfwidth_95": interval(&p99, write_reps),
        "p999_mean_seconds": mean(&p999, write_reps), "p999_halfwidth_95": interval(&p999, write_reps),
        "p999_sample_threshold_met": all_tail_samples, "stationary_tail_budget_established": false,
        "median_hold_mean_seconds": mean(&hold, write_reps), "opening_median_mean_seconds": mean(&opening, write_reps), "closing_median_mean_seconds": mean(&closing, write_reps),
        "passed_over_p99_mean": mean(&bypass, write_reps), "achieved_utilization_mean": mean(&achieved, write_reps),
        "service_p99_in_median_holds": if service { mean(&normalized_p99, write_reps) } else { None },
        "service_p999_in_median_holds": if service { mean(&normalized_p999, write_reps) } else { None },
        "p99_budget_median_holds": if service { Some(10) } else { None },
        "p999_budget_median_holds": if service { Some(100) } else { None },
        "burst_p99_budget_seconds": if burst { Some(1.0) } else { None },
        "observed_p99_over_budget": observed, "reopen_fairness_decision": contradiction,
        "snapshot_reads": reads, "snapshot_failures": failures,
        "snapshot_zero_failure_upper_95_iid": if failures == 0 { metrics::zero_failure_bound(reads) } else { None },
        "snapshot_failure_budget": 1e-6, "snapshot_budget_established": false,
        "snapshot_mean_elapsed_seconds": mean(&snapshot_wait, read_reps), "snapshot_p99_elapsed_seconds": mean(&snapshot_p99, read_reps),
        "snapshot_deadline_seconds": 1.0, "snapshot_simulated_wait_budget_seconds": 0.5,
        "armed_notice_mean_seconds": mean(&notice, notice_reps),
        "upgrade_observed_or_lower_bound_mean_seconds": mean(&upgrade, upgrade_reps), "upgrade_censored_replications": censored,
        "upgrade_budget_seconds": 60.0,
        "comparison": if service { "CQ-1 meets below about 0.6, breaches from about 0.7; achieved load and empirical hold shape condition any comparison" }
            else if burst { "CQ-1 unordered bursts near processor count breach; oversized bursts breach under every order" }
            else { "compare measured elapsed and censoring against the named budgets, not a claimed stationary proof" },
        "evidence": "finite window, empty start, no warm-up; normal-approximation intervals of replication estimates; censored and unobserved tails are unknown"
    })
}

pub(super) fn run(path: &Path, reporter: &Reporter) -> io::Result<()> {
    let summary_path = path.with_extension("summary.ndjson");
    let summary_reporter = Reporter {
        out: std::sync::Mutex::new(Box::new(std::fs::File::create(&summary_path)?)),
        err: std::sync::Mutex::new(Box::new(io::sink())),
    };
    let mut groups: BTreeMap<String, Vec<ResultRow>> = BTreeMap::new();
    for line in BufReader::new(std::fs::File::open(path)?).lines() {
        let row: ResultRow = serde_json::from_str(&line?)?;
        let key = format!(
            "{}:{}:util={:?}:rate={:?}:life={:?}",
            row.scenario,
            row.notification,
            row.configured_utilization,
            row.configured_rate,
            row.configured_lifetime
        );
        groups.entry(key).or_default().push(row);
    }
    for (scenario, rows) in groups {
        let mut summary = summarize(&rows);
        summary["case"] = scenario.into();
        summary_reporter.emit(&summary)?;
        reporter.emit(&summary)?;
    }
    reporter.emit(&serde_json::json!({"summary": summary_path}))?;
    Ok(())
}
