// Copyright (c) Michael Grier.

#![expect(
    clippy::disallowed_methods,
    reason = "benchmark pacing and subprocess watchdogs use real host time"
)]

use super::{
    clock, metrics,
    worker::{self, Event, Job, Reporter},
};
use acadia::{OsFs, Vfs};
use canopy::{
    DocumentBody, DocumentName, GenesisConfig, NodePath, SchemaLayer, Store, StoreId, Swap,
    UpgradeRequest,
};
use contention_sim::{dist::Dist, rng::SplitMix64};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

const SEED: u64 = 0x0C0F_FEE5;
const PROCESSES: usize = 16;

#[derive(Serialize, Deserialize)]
struct ResultRow {
    scenario: String,
    replication: usize,
    notification: String,
    processes: usize,
    configured_utilization: Option<f64>,
    configured_rate: Option<f64>,
    configured_lifetime: Option<f64>,
    burst_period_seconds: Option<f64>,
    observation_seconds: f64,
    requested: usize,
    completed_holds: usize,
    completed_requests: usize,
    unfinished: usize,
    achieved_utilization: f64,
    mean_wait: f64,
    p99_wait: f64,
    p999_wait: f64,
    p99_passed: f64,
    max_passed: f64,
    median_hold: f64,
    median_lock_wait: f64,
    opening_p50: f64,
    opening_p99: f64,
    closing_p50: f64,
    closing_p99: f64,
    snapshot_reads: usize,
    snapshot_failures: usize,
    snapshot_mean_wait: f64,
    snapshot_p99_wait: f64,
    snapshot_failure_upper_95_if_zero: Option<f64>,
    upgrade_wait: Option<f64>,
    upgrade_censored: bool,
    notices: Vec<f64>,
    p999_evidence_sufficient: bool,
    samples: Vec<metrics::Sample>,
}

#[path = "campaign/analysis.rs"]
mod analysis;

struct Process {
    child: Child,
    input: ChildStdin,
    output: mpsc::Receiver<io::Result<Event>>,
    stopped: bool,
}

impl Process {
    fn spawn(root: &Path, id: usize, role: &str, polled: bool, callback: f64) -> io::Result<Self> {
        let mut child = Command::new(std::env::current_exe()?)
            .arg("worker")
            .arg(root)
            .arg(id.to_string())
            .arg(role)
            .arg(if polled { "polled" } else { "default" })
            .arg(callback.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("worker stdin"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("worker stdout"))?;
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let event =
                    line.and_then(|line| serde_json::from_str(&line).map_err(io::Error::other));
                if sender.send(event).is_err() {
                    break;
                }
            }
        });
        let process = Self {
            child,
            input,
            output: receiver,
            stopped: false,
        };
        if !matches!(
            process
                .output
                .recv_timeout(Duration::from_secs(30))
                .map_err(io::Error::other)??,
            Event::Ready
        ) {
            return Err(io::Error::other("worker did not become ready"));
        }
        Ok(process)
    }
    fn send(&mut self, job: &Job) -> io::Result<()> {
        if matches!(job, Job::Stop) && self.stopped {
            return Ok(());
        }
        serde_json::to_writer(&mut self.input, job)?;
        writeln!(self.input)?;
        self.input.flush()?;
        if matches!(job, Job::Stop) {
            self.stopped = true;
        }
        Ok(())
    }
    fn finish(&mut self) -> io::Result<Vec<Event>> {
        self.send(&Job::Stop)?;
        let mut events = Vec::new();
        loop {
            let event = self
                .output
                .recv_timeout(Duration::from_secs(300))
                .map_err(io::Error::other)??;
            if let Event::Error { message } = &event {
                return Err(io::Error::other(message.clone()));
            }
            let done = matches!(event, Event::Done { .. });
            events.push(event);
            if done {
                break;
            }
        }
        if !self.child.wait()?.success() {
            return Err(io::Error::other("worker failed"));
        }
        Ok(events)
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Root(PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn seed(base: &Path, id: usize) -> io::Result<Root> {
    let root = Root(base.join(format!("root-{}-{id}", std::process::id())));
    std::fs::create_dir_all(&root.0)?;
    let vfs: Arc<dyn Vfs> = Arc::new(OsFs::new());
    let config = GenesisConfig::new(
        StoreId::new("contention-bench").map_err(io::Error::other)?,
        0,
    )
    .with_pointer_replica_count(1)
    .map_err(io::Error::other)?;
    let store = Store::init(
        vfs,
        &root.0,
        config,
        worker::options(0, true, Arc::default()),
    )
    .map_err(io::Error::other)?;
    let receipt = store
        .update_document(Swap::new(
            NodePath::parse("bench").unwrap(),
            DocumentName::new("seed").unwrap(),
            DocumentBody::new(vec![b'x'; 1024]),
        ))
        .map_err(io::Error::other)?;
    receipt.agent_stamp.map_err(io::Error::other)?;
    Ok(root)
}

fn finish(processes: &mut [Process]) -> io::Result<Vec<Event>> {
    for process in processes.iter_mut() {
        process.send(&Job::Stop)?;
    }
    let mut events = Vec::new();
    for process in processes {
        events.extend(process.finish()?);
    }
    Ok(events)
}

fn row(
    name: String,
    replication: usize,
    polled: bool,
    window: f64,
    requested: usize,
    events: Vec<Event>,
) -> io::Result<ResultRow> {
    let mut samples = Vec::new();
    let mut snapshots = Vec::new();
    let mut failures = 0;
    let mut notices = Vec::new();
    for event in events {
        match event {
            Event::Done { samples: measured } => samples.extend(measured),
            Event::Snapshot {
                elapsed, success, ..
            } => {
                snapshots.push(elapsed);
                failures += usize::from(!success);
            }
            Event::Notice {
                elapsed,
                changed: true,
                ..
            } => notices.push(elapsed),
            Event::Notice { changed: false, .. } => {
                return Err(io::Error::other(
                    "notice fired without an observed count change",
                ));
            }
            Event::Error { message } => return Err(io::Error::other(message)),
            _ => {}
        }
    }
    if samples
        .iter()
        .any(|sample| !sample.released || sample.acquired < sample.request)
    {
        return Err(io::Error::other("invalid timing or failed release"));
    }
    let waits: Vec<_> = samples.iter().map(|sample| sample.wait_seconds).collect();
    let holds: Vec<_> = samples.iter().map(|sample| sample.hold_seconds).collect();
    let lock_waits: Vec<_> = samples
        .iter()
        .map(|sample| sample.lock_wait_seconds)
        .collect();
    let openings: Vec<_> = samples
        .iter()
        .map(|sample| sample.opening_seconds)
        .collect();
    let closings: Vec<_> = samples
        .iter()
        .map(|sample| sample.closing_seconds)
        .collect();
    let passed = metrics::passed_over(&samples);
    let completed = samples.len();
    let completed_requests = samples
        .iter()
        .map(|sample| (sample.worker, sample.request))
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let elapsed_span = samples
        .iter()
        .map(|sample| sample.acquired as f64 / 1e9 + sample.hold_seconds)
        .fold(0.0, f64::max)
        - samples
            .iter()
            .map(|sample| sample.request as f64 / 1e9)
            .fold(f64::INFINITY, f64::min);
    let exposure = if elapsed_span.is_finite() {
        elapsed_span.max(window)
    } else {
        window
    };
    Ok(ResultRow {
        scenario: name,
        replication,
        notification: if polled { "polled" } else { "default" }.into(),
        processes: PROCESSES,
        configured_utilization: None,
        configured_rate: None,
        configured_lifetime: None,
        burst_period_seconds: None,
        observation_seconds: window,
        requested,
        completed_holds: completed,
        completed_requests,
        unfinished: requested.saturating_sub(completed_requests),
        achieved_utilization: holds.iter().sum::<f64>() / exposure,
        mean_wait: waits.iter().sum::<f64>() / waits.len().max(1) as f64,
        p99_wait: metrics::percentile(&waits, 99.0),
        p999_wait: metrics::percentile(&waits, 99.9),
        p99_passed: metrics::percentile(&passed, 99.0),
        max_passed: passed.iter().copied().fold(0.0, f64::max),
        median_hold: metrics::percentile(&holds, 50.0),
        median_lock_wait: metrics::percentile(&lock_waits, 50.0),
        opening_p50: metrics::percentile(&openings, 50.0),
        opening_p99: metrics::percentile(&openings, 99.0),
        closing_p50: metrics::percentile(&closings, 50.0),
        closing_p99: metrics::percentile(&closings, 99.0),
        snapshot_reads: snapshots.len(),
        snapshot_failures: failures,
        snapshot_mean_wait: snapshots.iter().sum::<f64>() / snapshots.len().max(1) as f64,
        snapshot_p99_wait: metrics::percentile(&snapshots, 99.0),
        snapshot_failure_upper_95_if_zero: if failures == 0 {
            metrics::zero_failure_bound(snapshots.len())
        } else {
            None
        },
        upgrade_wait: None,
        upgrade_censored: false,
        notices,
        p999_evidence_sufficient: completed >= 10000,
        samples,
    })
}

fn poisson(rate: f64, until: f64, rng: &mut SplitMix64) -> Vec<f64> {
    let mut times = Vec::new();
    let mut time = 0.0;
    loop {
        time += Dist::Exponential { mean: 1.0 / rate }.sample(rng);
        if time >= until {
            break;
        }
        times.push(time);
    }
    times
}

fn scheduled(start: Instant, seconds: f64) {
    let until = start + Duration::from_secs_f64(seconds);
    if let Some(wait) = until.checked_duration_since(Instant::now()) {
        std::thread::sleep(wait);
    }
}

struct Service {
    rate: f64,
    utilization: f64,
    callback: f64,
    recovery: bool,
    polled: bool,
}

fn service(
    root: &Path,
    replication: usize,
    window: f64,
    config: Service,
    rng: &mut SplitMix64,
) -> io::Result<ResultRow> {
    let Service {
        rate,
        utilization,
        callback,
        recovery,
        polled,
    } = config;
    let mut processes: Vec<_> = (0..PROCESSES)
        .map(|id| Process::spawn(root, id + 1, "writer", polled, callback))
        .collect::<io::Result<_>>()?;
    if recovery {
        processes.push(Process::spawn(root, PROCESSES + 1, "writer", polled, 0.0)?);
    }
    let times = poisson(rate, window, rng);
    let started = Instant::now();
    for (id, &time) in times.iter().enumerate() {
        scheduled(started, time);
        let at = clock::now()?;
        if recovery && rng.chance(0.01) {
            processes[PROCESSES].send(&Job::Recover { at, id: id as u64 })?;
        } else {
            processes[id % PROCESSES].send(&Job::Write { at, id: id as u64 })?;
        }
    }
    scheduled(started, window);
    let mut row = row(
        format!("service-callback-{callback}-recovery-{recovery}"),
        replication,
        polled,
        window,
        times.len(),
        finish(&mut processes)?,
    )?;
    row.processes = processes.len();
    row.configured_rate = Some(rate);
    row.configured_utilization = Some(utilization);
    Ok(row)
}

fn burst(
    root: &Path,
    replication: usize,
    window: f64,
    size: usize,
    rng: &mut SplitMix64,
) -> io::Result<ResultRow> {
    let mut processes: Vec<_> = (0..PROCESSES)
        .map(|id| Process::spawn(root, id + 1, "writer", false, 0.0))
        .collect::<io::Result<_>>()?;
    for id in 0..size {
        processes[id % PROCESSES].send(&Job::Burst {
            at: clock::now()?,
            id: id as u64,
            seed: rng.next_u64(),
        })?;
    }
    std::thread::sleep(Duration::from_secs_f64(window));
    let mut result = row(
        format!("burst-{size}"),
        replication,
        false,
        window,
        size * 4,
        finish(&mut processes)?,
    )?;
    result.burst_period_seconds = Some(120.0);
    Ok(result)
}

fn snapshots(
    root: &Path,
    replication: usize,
    window: f64,
    rate: f64,
    polled: bool,
    rng: &mut SplitMix64,
) -> io::Result<ResultRow> {
    let mut processes = vec![
        Process::spawn(root, 1, "writer", polled, 0.0)?,
        Process::spawn(root, 2, "reader", polled, 0.0)?,
    ];
    let mut jobs: Vec<_> = poisson(rate, window, rng)
        .into_iter()
        .map(|time| (time, true))
        .collect();
    jobs.extend((0..(window * 500.0) as usize).map(|id| (id as f64 / 500.0, false)));
    jobs.sort_by(|left, right| left.0.total_cmp(&right.0));
    let started = Instant::now();
    let mut writes = 0;
    for (id, &(time, write)) in jobs.iter().enumerate() {
        scheduled(started, time);
        let at = clock::now()?;
        if write {
            writes += 1;
            processes[0].send(&Job::Write { at, id: id as u64 })?;
        } else {
            processes[1].send(&Job::Snapshot { at, id: id as u64 })?;
        }
    }
    scheduled(started, window);
    let mut row = row(
        "metadata-snapshot".into(),
        replication,
        polled,
        window,
        writes,
        finish(&mut processes)?,
    )?;
    row.processes = 2;
    row.configured_rate = Some(rate);
    Ok(row)
}

fn upgrade(
    root: &Path,
    replication: usize,
    window: f64,
    rate: f64,
    lifetime: f64,
    unsafe_readers: bool,
    rng: &mut SplitMix64,
) -> io::Result<ResultRow> {
    let mut clients: Vec<_> = (0..PROCESSES)
        .map(|id| Process::spawn(root, id + 1, "attacher", true, 0.0))
        .collect::<io::Result<_>>()?;
    let vfs: Arc<dyn Vfs> = Arc::new(OsFs::new());
    let (store, _) = Store::open(vfs, root, worker::options(0, true, Arc::default()))
        .map_err(io::Error::other)?;
    let mut arrivals = poisson(rate, window, rng);
    arrivals.insert(0, 0.0);
    let start = Instant::now();
    clients[0].send(&Job::Attach {
        at: clock::now()?,
        id: 0,
        lifetime: Dist::Exponential { mean: lifetime }.sample(rng),
        writable: !unsafe_readers,
    })?;
    let initial = clients[0]
        .output
        .recv_timeout(Duration::from_secs(30))
        .map_err(io::Error::other)??;
    if !matches!(initial, Event::Attachment { .. }) {
        return Err(io::Error::other("initial attachment did not complete"));
    }
    let requested = Instant::now();
    let conversion = store
        .drain_upgrade()
        .and_then(|drain| {
            drain.begin_migration(
                UpgradeRequest::new(SchemaLayer::Agent, 1, 2, if unsafe_readers { 2 } else { 1 })?,
                0,
            )
        })
        .map_err(io::Error::other)?;
    let mut ready = None;
    let mut sent = 1;
    while start.elapsed().as_secs_f64() < window {
        while sent < arrivals.len() && arrivals[sent] <= start.elapsed().as_secs_f64() {
            clients[sent % PROCESSES].send(&Job::Attach {
                at: clock::now()?,
                id: sent as u64,
                lifetime: Dist::Exponential { mean: lifetime }.sample(rng),
                writable: !unsafe_readers,
            })?;
            sent += 1;
        }
        if ready.is_none() && conversion.poll().map_err(io::Error::other)? {
            ready = Some(requested.elapsed().as_secs_f64());
        }
        if ready.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let waited = requested.elapsed().as_secs_f64();
    conversion.cancel().map_err(io::Error::other)?;
    let mut events = Vec::new();
    for client in &mut clients {
        client.send(&Job::Stop)?;
    }
    for client in &mut clients {
        loop {
            let event = client
                .output
                .recv_timeout(Duration::from_secs(300))
                .map_err(io::Error::other)??;
            let done = matches!(event, Event::Done { .. });
            if let Event::Error { message } = event {
                return Err(io::Error::other(message));
            }
            events.push(event);
            if done {
                break;
            }
        }
        if !client.child.wait()?.success() {
            return Err(io::Error::other("attachment worker failed"));
        }
    }
    let completed = 1 + events
        .iter()
        .filter(|event| matches!(event, Event::Attachment { .. }))
        .count();
    let mut result = row(
        format!("upgrade-{}", if unsafe_readers { "unsafe" } else { "safe" }),
        replication,
        true,
        window,
        sent,
        events,
    )?;
    result.completed_holds = completed;
    result.completed_requests = completed;
    result.unfinished = sent.saturating_sub(completed);
    result.configured_rate = Some(rate);
    result.configured_lifetime = Some(lifetime);
    result.upgrade_wait = ready.or(Some(waited));
    result.upgrade_censored = ready.is_none();
    Ok(result)
}

fn publication(root: &Path, replication: usize, polled: bool) -> io::Result<ResultRow> {
    let mut processes = vec![
        Process::spawn(root, 1, "writer", polled, 0.0)?,
        Process::spawn(root, 2, "reader", polled, 0.0)?,
    ];
    let mut notices = Vec::new();
    let begun = Instant::now();
    for id in 0..16 {
        processes[1].send(&Job::Arm { id })?;
        let event = processes[1]
            .output
            .recv_timeout(Duration::from_secs(30))
            .map_err(io::Error::other)??;
        if !matches!(event, Event::Armed { id: armed } if armed == id) {
            return Err(io::Error::other("notice did not arm"));
        }
        processes[0].send(&Job::Write {
            at: clock::now()?,
            id,
        })?;
        let event = processes[1]
            .output
            .recv_timeout(Duration::from_secs(30))
            .map_err(io::Error::other)??;
        if !matches!(event, Event::Notice { id: noticed, changed: true, .. } if noticed == id) {
            return Err(io::Error::other("armed notice did not observe publication"));
        }
        notices.push(event);
        let participant = acadia::coord::CoordinationFactory::open(
            &acadia::coord::file::FileFactory::new().polled(),
            Arc::new(OsFs::new()),
            root,
        )
        .map_err(io::Error::other)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        while !participant.sample().map_err(io::Error::other)?.is_settled() {
            if Instant::now() >= deadline {
                return Err(io::Error::other("publication never settled"));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    notices.extend(finish(&mut processes)?);
    let mut row = row(
        "publication-armed-notice".into(),
        replication,
        polled,
        begun.elapsed().as_secs_f64(),
        16,
        notices,
    )?;
    row.processes = 2;
    Ok(row)
}

pub fn run(args: &[String], reporter: &Reporter) -> io::Result<()> {
    if args.first().is_some_and(|arg| arg == "analyze") {
        let path = args
            .get(1)
            .ok_or_else(|| io::Error::other("analyze needs a report path"))?;
        return analysis::run(Path::new(path), reporter);
    }
    let path = PathBuf::from(
        args.first()
            .map(String::as_str)
            .unwrap_or(".scratch/contention-bench.ndjson"),
    );
    let window = args
        .get(1)
        .map(|value| value.parse::<f64>())
        .transpose()
        .map_err(io::Error::other)?
        .unwrap_or(0.25);
    let repetitions = args
        .get(2)
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(io::Error::other)?
        .unwrap_or(10);
    let smoke = args.get(3).is_some_and(|value| value == "smoke");
    if !window.is_finite() || window <= 0.0 || repetitions == 0 {
        return Err(io::Error::other(
            "positive window and replication count are required",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("report parent"))?;
    std::fs::create_dir_all(parent)?;
    let base = std::fs::canonicalize(parent)?.join("benchmark-roots");
    std::fs::create_dir_all(&base)?;
    let mut file = std::fs::File::create(&path)?;
    let calibrated = seed(&base, 0)?;
    let mut process = Process::spawn(&calibrated.0, 1, "writer", true, 0.0)?;
    for id in 0..32 {
        process.send(&Job::Write {
            at: clock::now()?,
            id,
        })?;
    }
    let calibration = row("calibration".into(), 0, true, 0.0, 32, process.finish()?)?;
    let hold = calibration
        .samples
        .iter()
        .map(|sample| sample.hold_seconds)
        .sum::<f64>()
        / calibration.samples.len().max(1) as f64;
    if hold <= 0.0 {
        return Err(io::Error::other("calibration recorded no holds"));
    }
    serde_json::to_writer(&mut file, &calibration)?;
    writeln!(file)?;
    let recovery_root = seed(&base, usize::MAX)?;
    let mut recovery = Process::spawn(&recovery_root.0, 1, "writer", true, 0.0)?;
    let begun = Instant::now();
    recovery.send(&Job::Recover {
        at: clock::now()?,
        id: 0,
    })?;
    let recovery_row = row(
        "recovery-calibration".into(),
        0,
        true,
        begun.elapsed().as_secs_f64(),
        1,
        recovery.finish()?,
    )?;
    serde_json::to_writer(&mut file, &recovery_row)?;
    writeln!(file)?;
    let mut sequence = 1;
    for replication in 0..repetitions {
        let mut rng = SplitMix64::new(SEED + replication as u64);
        let utilizations: &[f64] = if smoke {
            &[0.5]
        } else {
            &[0.3, 0.5, 0.6, 0.7, 0.8, 0.9, 0.95]
        };
        for &utilization in utilizations {
            for &callback in if utilization == 0.7 {
                &[0.0, 0.005, 0.02][..]
            } else {
                &[0.0][..]
            } {
                let root = seed(&base, sequence)?;
                sequence += 1;
                let result = service(
                    &root.0,
                    replication,
                    window,
                    Service {
                        rate: utilization / hold,
                        utilization,
                        callback,
                        recovery: false,
                        polled: false,
                    },
                    &mut rng,
                )?;
                serde_json::to_writer(&mut file, &result)?;
                writeln!(file)?;
                file.flush()?;
            }
        }
        let root = seed(&base, sequence)?;
        sequence += 1;
        let result = service(
            &root.0,
            replication,
            window,
            Service {
                rate: 0.6 / hold,
                utilization: 0.6,
                callback: 0.0,
                recovery: true,
                polled: false,
            },
            &mut rng,
        )?;
        serde_json::to_writer(&mut file, &result)?;
        writeln!(file)?;
        file.flush()?;
        for &size in if smoke {
            &[4][..]
        } else {
            &[4, 16, 64, 256][..]
        } {
            let root = seed(&base, sequence)?;
            sequence += 1;
            let result = burst(&root.0, replication, window, size, &mut rng)?;
            serde_json::to_writer(&mut file, &result)?;
            writeln!(file)?;
            file.flush()?;
        }
        for &rate in if smoke {
            &[10.0][..]
        } else {
            &[1.0 / 3600.0, 1.0 / 60.0, 1.0, 10.0][..]
        } {
            let paired_seed = rng.next_u64();
            for polled in [false, true] {
                let mut paired = SplitMix64::new(paired_seed);
                let root = seed(&base, sequence)?;
                sequence += 1;
                let result = snapshots(&root.0, replication, window, rate, polled, &mut paired)?;
                serde_json::to_writer(&mut file, &result)?;
                writeln!(file)?;
                file.flush()?;
            }
        }
        for polled in [false, true] {
            let root = seed(&base, sequence)?;
            sequence += 1;
            let result = publication(&root.0, replication, polled)?;
            serde_json::to_writer(&mut file, &result)?;
            writeln!(file)?;
            file.flush()?;
        }
        for &rate in if smoke {
            &[1.0][..]
        } else {
            &[1.0, 10.0, 100.0, 1000.0, 2000.0][..]
        } {
            for &lifetime in if smoke {
                &[1.0][..]
            } else {
                &[1.0, 60.0, 3600.0][..]
            } {
                for unsafe_readers in [false, true] {
                    let root = seed(&base, sequence)?;
                    sequence += 1;
                    let result = upgrade(
                        &root.0,
                        replication,
                        window,
                        rate,
                        lifetime,
                        unsafe_readers,
                        &mut rng,
                    )?;
                    serde_json::to_writer(&mut file, &result)?;
                    writeln!(file)?;
                    file.flush()?;
                }
            }
        }
        reporter.emit(&serde_json::json!({"replication_complete":replication,"report":path}))?;
    }
    analysis::run(&path, reporter)?;
    Ok(())
}
