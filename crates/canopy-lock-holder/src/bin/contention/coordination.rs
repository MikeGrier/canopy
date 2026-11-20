// Copyright (c) Michael Grier.

#![expect(
    clippy::disallowed_methods,
    reason = "benchmark subprocess watchdogs measure real host time"
)]

use super::{clock, metrics, worker::Reporter};
use acadia::coord::{
    CoordError, Coordination, CoordinationFactory, CreationNonce, ModificationEnd, Purpose, Waiting,
};
use acadia::{OsFs, Vfs};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum Backend {
    File,
    Shared,
}

impl Backend {
    fn name(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Shared => "shared",
        }
    }
    fn parse(name: &str) -> io::Result<Self> {
        match name {
            "file" => Ok(Self::File),
            "shared" => Ok(Self::Shared),
            _ => Err(io::Error::other("backend must be file or shared")),
        }
    }
    fn factory(self) -> Box<dyn CoordinationFactory> {
        match self {
            Self::File => Box::new(acadia::coord::file::FileFactory::new()),
            Self::Shared => Box::new(acadia::coord::shared::SharedFactory::new()),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Turn {
    worker: usize,
    request: u64,
    acquired: u64,
    released: u64,
    generation: u64,
    count: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event")]
enum Event {
    Ready,
    Refused,
    Done { turns: Vec<Turn>, seconds: f64 },
}

struct Process {
    child: Child,
    input: ChildStdin,
    output: mpsc::Receiver<io::Result<Event>>,
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Process {
    fn spawn(root: &Path, backend: Backend, worker: usize, turns: usize) -> io::Result<Self> {
        let mut child = Command::new(std::env::current_exe()?)
            .args(["coordination-worker", backend.name()])
            .arg(root)
            .arg(worker.to_string())
            .arg(turns.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("worker stdin missing"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("worker stdout missing"))?;
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                if sender
                    .send(
                        line.and_then(|line| serde_json::from_str(&line).map_err(io::Error::other)),
                    )
                    .is_err()
                {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            input,
            output: receiver,
        })
    }
    fn event(&self) -> io::Result<Event> {
        self.output
            .recv_timeout(Duration::from_secs(120))
            .map_err(io::Error::other)?
    }
    fn go(&mut self) -> io::Result<()> {
        writeln!(self.input, "go")?;
        self.input.flush()
    }
    fn finish(&mut self) -> io::Result<()> {
        if !self.child.wait()?.success() {
            return Err(io::Error::other("coordination worker failed"));
        }
        Ok(())
    }
}

struct Root(PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open(root: &Path, backend: Backend) -> io::Result<Box<dyn Coordination>> {
    let participant = backend
        .factory()
        .open(Arc::new(OsFs::new()), root)
        .map_err(io::Error::other)?;
    participant
        .confirm_created("benchmark-marker")
        .map_err(io::Error::other)?;
    Ok(participant)
}

fn seed(root: &Path, backend: Backend) -> io::Result<Box<dyn Coordination>> {
    std::fs::create_dir_all(root)?;
    let fs = Arc::new(OsFs::new());
    let participant = backend
        .factory()
        .open(fs.clone(), root)
        .map_err(io::Error::other)?;
    participant
        .reserve(
            b"coordination benchmark",
            CreationNonce::from_bytes([1; 16]),
        )
        .map_err(io::Error::other)?;
    fs.write(&root.join("benchmark-marker"), b"coordination benchmark")
        .map_err(io::Error::other)?;
    participant
        .confirm_created("benchmark-marker")
        .map_err(io::Error::other)?;
    participant
        .acquire(Waiting::Wait)
        .and_then(|mut held| held.complete_recovery())
        .map_err(io::Error::other)?;
    Ok(participant)
}

fn worker(args: &[String], reporter: &Reporter) -> io::Result<()> {
    let backend = Backend::parse(
        args.first()
            .ok_or_else(|| io::Error::other("worker backend missing"))?,
    )?;
    let root = Path::new(
        args.get(1)
            .ok_or_else(|| io::Error::other("worker root missing"))?,
    );
    let worker: usize = args
        .get(2)
        .ok_or_else(|| io::Error::other("worker id missing"))?
        .parse()
        .map_err(io::Error::other)?;
    let iterations: usize = args
        .get(3)
        .ok_or_else(|| io::Error::other("worker iterations missing"))?
        .parse()
        .map_err(io::Error::other)?;
    let participant = backend
        .factory()
        .open(Arc::new(OsFs::new()), root)
        .map_err(io::Error::other)?;
    match participant.confirm_created("benchmark-marker") {
        Err(CoordError::ImplementationMismatch { .. }) => return reporter.emit(&Event::Refused),
        result => result.map_err(io::Error::other)?,
    }
    participant.sample().map_err(io::Error::other)?;
    reporter.emit(&Event::Ready)?;
    let mut go = String::new();
    if io::stdin().read_line(&mut go)? == 0 || go.trim() != "go" {
        return Err(io::Error::other("missing start command"));
    }
    let started = Instant::now();
    let mut turns = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let request = clock::now()?;
        let mut hold = participant
            .acquire(Waiting::Wait)
            .map_err(io::Error::other)?;
        let acquired = clock::now()?;
        if hold.acquired().recovery_required() {
            return Err(io::Error::other("unexpected benchmark recovery"));
        }
        let generation = hold.acquired().advanced_to().get();
        hold.begin_modification(Purpose::Ordinary)
            .map_err(io::Error::other)?;
        hold.end_modification(ModificationEnd::Settled)
            .map_err(io::Error::other)?;
        let count = participant.sample().map_err(io::Error::other)?.get();
        hold.release().map_err(io::Error::other)?;
        turns.push(Turn {
            worker,
            request,
            acquired,
            released: clock::now()?,
            generation,
            count,
        });
    }
    reporter.emit(&Event::Done {
        turns,
        seconds: started.elapsed().as_secs_f64(),
    })
}

fn validate_turns(
    turns: &[Turn],
    expected: usize,
    initial_generation: u64,
    initial_count: u64,
) -> io::Result<()> {
    if turns.len() != expected {
        return Err(io::Error::other("missing completed writer turns"));
    }
    let mut sorted: Vec<_> = turns.iter().collect();
    sorted.sort_by_key(|turn| turn.generation);
    for (index, turn) in sorted.iter().enumerate() {
        if turn.request > turn.acquired
            || turn.acquired > turn.released
            || turn.generation != initial_generation + index as u64 + 1
            || turn.count != initial_count + 2 * (index as u64 + 1)
        {
            return Err(io::Error::other(
                "nonmonotonic timing, generation or modification count",
            ));
        }
    }
    Ok(())
}

fn measure(
    root: &Path,
    backend: Backend,
    samples: usize,
    iterations: usize,
    processes: usize,
    replication: usize,
) -> io::Result<serde_json::Value> {
    let parent = seed(root, backend)?;
    let reader = open(root, backend)?;
    for _ in 0..32 {
        std::hint::black_box(reader.sample().map_err(io::Error::other)?);
    }
    let mut sample_batches = Vec::new();
    for _ in 0..10 {
        let started = Instant::now();
        for _ in 0..samples {
            std::hint::black_box(reader.sample().map_err(io::Error::other)?);
        }
        sample_batches.push(started.elapsed().as_secs_f64() / samples as f64);
    }
    let initial = parent.acquire(Waiting::Wait).map_err(io::Error::other)?;
    let initial_generation = initial.acquired().advanced_to().get();
    initial.release().map_err(io::Error::other)?;
    let initial_count = parent.sample().map_err(io::Error::other)?.get();
    let mut children = (0..processes)
        .map(|id| Process::spawn(root, backend, id, iterations))
        .collect::<io::Result<Vec<_>>>()?;
    for child in &children {
        if !matches!(child.event()?, Event::Ready) {
            return Err(io::Error::other("writer not ready"));
        }
    }
    let started = Instant::now();
    for child in &mut children {
        child.go()?;
    }
    let mut turns = Vec::new();
    for child in &mut children {
        match child.event()? {
            Event::Done {
                turns: measured, ..
            } => turns.extend(measured),
            _ => return Err(io::Error::other("writer did not finish")),
        }
        child.finish()?;
    }
    let elapsed = started.elapsed().as_secs_f64();
    validate_turns(
        &turns,
        processes * iterations,
        initial_generation,
        initial_count,
    )?;
    let final_count = parent.sample().map_err(io::Error::other)?.get();
    if final_count != initial_count + 2 * turns.len() as u64 {
        return Err(io::Error::other("final count lost a writer turn"));
    }
    let waits: Vec<_> = turns
        .iter()
        .map(|turn| (turn.acquired - turn.request) as f64 / 1e9)
        .collect();
    let holds: Vec<_> = turns
        .iter()
        .map(|turn| (turn.released - turn.acquired) as f64 / 1e9)
        .collect();
    Ok(
        serde_json::json!({"backend": backend.name(), "replication": replication, "os": std::env::consts::OS,
        "sample_batch_size": samples, "sample_seconds_per_call": sample_batches,
        "sample_p50_seconds": metrics::percentile(&sample_batches, 50.0), "sample_p99_seconds": metrics::percentile(&sample_batches, 99.0),
        "processes": processes, "turns": turns, "total_turns": waits.len(), "elapsed_seconds": elapsed,
        "turns_per_second": waits.len() as f64 / elapsed, "wait_p50_seconds": metrics::percentile(&waits, 50.0), "wait_p99_seconds": metrics::percentile(&waits, 99.0),
        "hold_p50_seconds": metrics::percentile(&holds, 50.0), "hold_p99_seconds": metrics::percentile(&holds, 99.0),
        "evidence": "raw coordination cost; no document IO; finite closed process loops; no stationary wait-budget claim"}),
    )
}

pub fn run(args: &[String], reporter: &Reporter) -> io::Result<()> {
    if args.first().is_some_and(|arg| arg == "coordination-worker") {
        return worker(&args[1..], reporter);
    }
    let path = PathBuf::from(
        args.get(1)
            .map(String::as_str)
            .unwrap_or(".scratch/coordination-benchmark.ndjson"),
    );
    let numeric = |index: usize, default: usize| -> io::Result<usize> {
        args.get(index)
            .map(|value| value.parse())
            .transpose()
            .map_err(io::Error::other)
            .map(|value| value.unwrap_or(default))
    };
    let samples = numeric(2, 1000)?;
    let iterations = numeric(3, 32)?;
    let repetitions = numeric(4, 10)?;
    if samples == 0 || iterations == 0 || repetitions == 0 {
        return Err(io::Error::other(
            "sample, turn and replication counts must be positive",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("report directory missing"))?;
    std::fs::create_dir_all(parent)?;
    let base = std::fs::canonicalize(parent)?;
    let output = Reporter {
        out: std::sync::Mutex::new(Box::new(std::fs::File::create(&path)?)),
        err: std::sync::Mutex::new(Box::new(io::sink())),
    };
    for replication in 0..repetitions {
        let backends = if replication % 2 == 0 {
            [Backend::File, Backend::Shared]
        } else {
            [Backend::Shared, Backend::File]
        };
        for backend in backends {
            let root = Root(base.join(format!(
                "coord-bench-{}-{replication}-{}",
                std::process::id(),
                backend.name()
            )));
            let result = measure(&root.0, backend, samples, iterations, 4, replication)?;
            output.emit(&result)?;
            reporter.emit(&serde_json::json!({"backend":backend.name(), "replication":replication, "sample_p50_seconds":result["sample_p50_seconds"], "wait_p99_seconds":result["wait_p99_seconds"], "turns_per_second":result["turns_per_second"]}))?;
        }
    }
    reporter.emit(&serde_json::json!({"report":path}))
}
