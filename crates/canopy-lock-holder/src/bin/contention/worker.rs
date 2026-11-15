// Copyright (c) Michael Grier.

#![expect(
    clippy::disallowed_methods,
    reason = "benchmark think times and notification watchdogs use real host time"
)]

use super::{clock, metrics::Sample, timed};
use acadia::coord::file::FileFactory;
use acadia::{OsFs, Vfs};
use canopy::{
    Agent, CommitObserver, DocumentBody, DocumentName, NodePath, Nonce, Store, StoreError,
    StoreOptions, Swap, SwapSet,
};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command")]
pub enum Job {
    Write {
        at: u64,
        id: u64,
    },
    Recover {
        at: u64,
        id: u64,
    },
    Burst {
        at: u64,
        id: u64,
        seed: u64,
    },
    Snapshot {
        at: u64,
        id: u64,
    },
    Arm {
        id: u64,
    },
    Attach {
        at: u64,
        id: u64,
        lifetime: f64,
        writable: bool,
    },
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum Event {
    Ready,
    Armed {
        id: u64,
    },
    Notice {
        id: u64,
        elapsed: f64,
        changed: bool,
    },
    Snapshot {
        id: u64,
        elapsed: f64,
        success: bool,
    },
    Attachment {
        id: u64,
        elapsed: f64,
    },
    AdmissionRefused {
        id: u64,
    },
    Done {
        samples: Vec<Sample>,
    },
    Error {
        message: String,
    },
}

pub struct Reporter {
    pub out: Mutex<Box<dyn Write + Send>>,
    pub err: Mutex<Box<dyn Write + Send>>,
}

impl Reporter {
    pub fn emit(&self, value: &impl Serialize) -> io::Result<()> {
        let mut out = self.out.lock().unwrap();
        serde_json::to_writer(&mut *out, value)?;
        writeln!(out)?;
        out.flush()
    }
    pub fn error(&self, message: &str) {
        let _ = writeln!(self.err.lock().unwrap(), "{message}");
    }
}

pub fn options(worker: usize, polled: bool, metrics: Arc<timed::Metrics>) -> StoreOptions {
    let factory = if polled {
        FileFactory::new().polled()
    } else {
        FileFactory::new()
    };
    let mut nonce = 0u64;
    StoreOptions::with_nonce_source(
        Agent::new("contention-bench", "bench", "contention benchmark", 1),
        move || {
            nonce += 1;
            Nonce::new(&format!("worker{worker}-nonce{nonce}")).unwrap()
        },
    )
    .with_coordination(Arc::new(timed::Factory {
        inner: Arc::new(factory),
        metrics,
    }))
}

fn burn(seconds: f64) {
    let until = Instant::now() + Duration::from_secs_f64(seconds);
    while Instant::now() < until {
        std::hint::spin_loop();
    }
}

struct Callback(f64);
impl CommitObserver for Callback {
    fn committed(&self, _node: &NodePath, _document: &DocumentName, _body: &DocumentBody) {
        burn(self.0);
    }
}

fn write(store: &Store, worker: usize, id: u64) -> Result<(), String> {
    let mut attempts = 0;
    let result = loop {
        let swaps = SwapSet::new((0..2).map(|part| {
            Swap::new(
                NodePath::parse("bench").unwrap(),
                DocumentName::new(&format!("worker{worker}-write{id}-part{part}")).unwrap(),
                DocumentBody::new(vec![b'x'; 1024]),
            )
        }))
        .map_err(|error| error.to_string())?;
        match store.apply_transaction(&swaps) {
            Err(StoreError::RecoveryRequired { .. }) if attempts < 100 => {
                store
                    .recover()
                    .and_then(|outcome| outcome.agent_stamp)
                    .map_err(|error| error.to_string())?;
                attempts += 1;
            }
            result => break result.map_err(|error| error.to_string())?,
        }
    };
    result.agent_stamp.map_err(|error| error.to_string())?;
    if !result.value.is_committed() {
        return Err("an unconditional benchmark write was rejected".into());
    }
    Ok(())
}

pub fn run(
    root: PathBuf,
    worker: usize,
    role: &str,
    polled: bool,
    callback: f64,
    reporter: Arc<Reporter>,
) -> io::Result<()> {
    let metrics = Arc::new(timed::Metrics {
        worker,
        ..Default::default()
    });
    let fault = acadia::FaultGate::closed();
    let policy = acadia::FaultPolicy::new().inject_gated(
        acadia::FaultOp::Rename,
        acadia::PathMatcher::has_component("namespace"),
        acadia::Fault::io_error(),
        acadia::When::Always,
        fault.clone(),
    );
    let vfs: Arc<dyn Vfs> = Arc::new(acadia::FaultyFs::new(OsFs::new(), policy));
    let recovery_serial = Arc::new(Mutex::new(()));
    let store = if role == "attacher" {
        None
    } else {
        Some(
            if role == "reader" {
                Store::open_read_only(
                    Arc::clone(&vfs),
                    &root,
                    options(worker, polled, Arc::clone(&metrics)),
                )
            } else {
                Store::open(
                    Arc::clone(&vfs),
                    &root,
                    options(worker, polled, Arc::clone(&metrics)),
                )
                .map(|(store, _)| store)
            }
            .map_err(io::Error::other)?,
        )
    };
    if let Some(store) = &store {
        store
            .observe(Arc::new(Callback(callback)))
            .map_err(io::Error::other)?;
    }
    let stop = Arc::new((Mutex::new(false), Condvar::new()));
    let mut jobs = Vec::new();
    reporter.emit(&Event::Ready)?;
    for line in io::stdin().lock().lines() {
        let job: Job = serde_json::from_str(&line?)?;
        if matches!(job, Job::Stop) {
            break;
        }
        let (store, metrics, reporter, stop, root, vfs) = (
            store.clone(),
            Arc::clone(&metrics),
            Arc::clone(&reporter),
            Arc::clone(&stop),
            root.clone(),
            Arc::clone(&vfs),
        );
        let (fault, recovery_serial) = (fault.clone(), Arc::clone(&recovery_serial));
        jobs.push(thread::spawn(move || {
            let result: Result<(), String> = match job {
                Job::Write { at, id } => {
                    metrics.request(at);
                    let result = write(store.as_ref().unwrap(), worker, id);
                    metrics.clear_request();
                    result
                }
                Job::Recover { at, id } => {
                    let _serial = recovery_serial.lock().unwrap();
                    metrics.request(at);
                    let store = store.as_ref().unwrap();
                    let swaps = SwapSet::new((0..64).map(|unit| {
                        Swap::new(
                            NodePath::parse("bench").unwrap(),
                            DocumentName::new(&format!("worker{worker}-recovery{id}-unit{unit}"))
                                .unwrap(),
                            DocumentBody::new(vec![b'x'; 1024]),
                        )
                    }))
                    .map_err(|error| error.to_string());
                    fault.open();
                    let failed = swaps.and_then(|swaps| {
                        store
                            .apply_transaction(&swaps)
                            .map_err(|error| error.to_string())
                    });
                    fault.close();
                    let result = if failed.is_ok() {
                        Err("recovery injection did not fail".into())
                    } else {
                        store
                            .recover()
                            .and_then(|outcome| outcome.agent_stamp)
                            .map_err(|error| error.to_string())
                    };
                    metrics.clear_request();
                    result
                }
                Job::Burst { at, id, seed } => {
                    let mut rng = contention_sim::rng::SplitMix64::new(seed);
                    let mut result = Ok(());
                    for step in 0..4 {
                        if *stop.0.lock().unwrap() {
                            break;
                        }
                        let requested = if step == 0 {
                            at
                        } else {
                            clock::now().unwrap_or(at)
                        };
                        metrics.request(requested);
                        result = write(store.as_ref().unwrap(), worker, id * 4 + step);
                        metrics.clear_request();
                        if result.is_err() {
                            break;
                        }
                        let think =
                            contention_sim::dist::Dist::Exponential { mean: 0.05 }.sample(&mut rng);
                        thread::sleep(Duration::from_secs_f64(think));
                    }
                    result
                }
                Job::Snapshot { at, id } => {
                    let result = store.as_ref().unwrap().snapshot(|view| {
                        let read = view.read_document(
                            NodePath::parse("bench").unwrap(),
                            &DocumentName::new("seed").unwrap(),
                        );
                        burn(0.001);
                        read
                    });
                    let success = matches!(result, Ok(Ok(_)));
                    let elapsed = clock::now()
                        .map_err(|error| error.to_string())
                        .map(|now| now.saturating_sub(at) as f64 / 1e9);
                    elapsed.and_then(|elapsed| {
                        reporter
                            .emit(&Event::Snapshot {
                                id,
                                elapsed,
                                success,
                            })
                            .map_err(|error| error.to_string())
                    })
                }
                Job::Arm { id } => {
                    use acadia::coord::CoordinationFactory;
                    let factory = if polled {
                        FileFactory::new().polled()
                    } else {
                        FileFactory::new()
                    };
                    let participant = factory.open(vfs, &root).map_err(|error| error.to_string());
                    participant.and_then(|participant| {
                        let sampled = participant.sample().map_err(|error| error.to_string())?;
                        let (ring, notice) = std::sync::mpsc::channel();
                        let begun = Instant::now();
                        let registered = participant
                            .notify(
                                sampled,
                                begun + Duration::from_secs(2),
                                Box::new(move || {
                                    let _ = ring.send(());
                                }),
                            )
                            .map_err(|error| error.to_string())?;
                        reporter
                            .emit(&Event::Armed { id })
                            .map_err(|error| error.to_string())?;
                        notice
                            .recv_timeout(Duration::from_secs(3))
                            .map_err(|error| error.to_string())?;
                        let changed =
                            participant.sample().map_err(|error| error.to_string())? != sampled;
                        drop(registered);
                        reporter
                            .emit(&Event::Notice {
                                id,
                                elapsed: begun.elapsed().as_secs_f64(),
                                changed,
                            })
                            .map_err(|error| error.to_string())
                    })
                }
                Job::Attach {
                    at,
                    id,
                    lifetime,
                    writable,
                } => {
                    let attached = if writable {
                        Store::open(vfs, root, options(worker, polled, metrics))
                            .map(|(store, _)| store)
                    } else {
                        Store::open_read_only(vfs, root, options(worker, polled, metrics))
                    };
                    match attached {
                        Ok(attached) => {
                            let elapsed =
                                clock::now().unwrap_or(at).saturating_sub(at) as f64 / 1e9;
                            let report = reporter
                                .emit(&Event::Attachment { id, elapsed })
                                .map_err(|error| error.to_string());
                            let (stopped, changed) = &*stop;
                            let stopped = stopped.lock().unwrap();
                            if !*stopped {
                                drop(
                                    changed
                                        .wait_timeout(stopped, Duration::from_secs_f64(lifetime))
                                        .unwrap(),
                                );
                            }
                            drop(attached);
                            report
                        }
                        Err(canopy::StoreError::SchemaMismatch { .. }) => reporter
                            .emit(&Event::AdmissionRefused { id })
                            .map_err(|error| error.to_string()),
                        Err(error) => Err(error.to_string()),
                    }
                }
                Job::Stop => Ok(()),
            };
            if let Err(message) = result {
                let _ = reporter.emit(&Event::Error { message });
            }
        }));
    }
    {
        let (stopped, changed) = &*stop;
        *stopped.lock().unwrap() = true;
        changed.notify_all();
    }
    for job in jobs {
        job.join()
            .map_err(|_| io::Error::other("benchmark worker panicked"))?;
    }
    let samples = std::mem::take(&mut *metrics.samples.lock().unwrap());
    reporter.emit(&Event::Done { samples })
}
