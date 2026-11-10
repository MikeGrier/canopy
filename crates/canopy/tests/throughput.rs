// Copyright (c) Michael Grier.

//! Durable-transaction throughput measurement (D84 / OQ12 sizing).
//!
//! Not a pass/fail test -- a measurement. It drives the real D53 two-phase commit
//! path (`Store::update_document` over the real `OsFs` backend, so every commit
//! pays its true fsyncs) and reports transactions-per-second and per-transaction
//! latency percentiles across payload sizes and partition counts N.
//!
//! It is `#[ignore]`d so the normal suite stays fast and disk-free; run it
//! explicitly:
//!
//! ```text
//! cargo test -p canopy --release --test throughput -- --ignored --nocapture
//! ```
//!
//! libtest captures a passing test's stdout, so the harness also writes a report
//! to `<repo>/.scratch/canopy-throughput.txt` (through the same `OsFs` seam), which
//! is the reliable channel when run under a capturing harness.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use acadia::{OsFs, Vfs};
use canopy::{DocumentBody, DocumentName, GenesisConfig, NodePath, Nonce, ShireId, Store, Swap};

/// Payload sizes (bytes) swept per partition count.
const PAYLOADS: &[usize] = &[256, 1024, 4096, 65_536, 1_048_576];

/// Partition counts swept (D84: N=1 in-process baseline vs N=3 quorum).
const PARTITIONS: &[usize] = &[1, 3];

/// Transactions timed per configuration, chosen per payload so the largest
/// payloads do not write tens of gigabytes: smaller for bigger bodies.
fn transactions_for(payload: usize) -> usize {
    match payload {
        0..=4_096 => 300,
        4_097..=65_536 => 150,
        _ => 50,
    }
}

/// Untimed warm-up transactions before each measured run.
const WARMUP: usize = 10;

/// A deterministic 1:1 mix (SplitMix64 finalizer) so distinct counters yield
/// distinct, uniformly-distributed node names that spread across shards.
fn mix(i: u64) -> u64 {
    let mut z = i.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn percentile(sorted_micros: &[u128], p: f64) -> u128 {
    if sorted_micros.is_empty() {
        return 0;
    }
    let rank = (p / 100.0 * (sorted_micros.len() as f64 - 1.0)).round() as usize;
    sorted_micros[rank.min(sorted_micros.len() - 1)]
}

#[test]
#[ignore = "measurement, not a pass/fail test; run with --release --ignored --nocapture"]
fn measure_transaction_throughput() {
    let vfs: Arc<dyn Vfs> = Arc::new(OsFs::new());

    // <repo>/.scratch/canopy-throughput/ -- CARGO_MANIFEST_DIR is crates/canopy.
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root above crates/canopy")
        .to_path_buf();
    let bench_root = repo_root.join(".scratch").join("canopy-throughput");
    vfs.create_dir_all(&bench_root).expect("create bench root");

    let mut report = String::new();
    report
        .push_str("canopy durable-transaction throughput (single-writer path, real OsFs fsyncs)\n");
    report.push_str(&format!("store volume: {}\n", bench_root.display()));
    report.push_str(&format!(
        "build: {}\n\n",
        if cfg!(debug_assertions) {
            "debug (numbers are pessimistic; prefer --release)"
        } else {
            "release"
        }
    ));
    report.push_str(&format!(
        "{:>2}  {:>10}  {:>4}  {:>9}  {:>9}  {:>9}  {:>9}  {:>9}\n",
        "N", "payload", "txns", "TPS", "p50 ms", "p90 ms", "p99 ms", "max ms"
    ));
    report.push_str(&format!("{}\n", "-".repeat(74)));

    let mut counter: u64 = 0;

    for &n in PARTITIONS {
        for &payload in PAYLOADS {
            let txns = transactions_for(payload);
            let store_dir = bench_root.join(format!("n{n}-p{payload}"));
            // A fresh empty root each run (init refuses a non-empty/initialized dir).
            let _ = vfs.remove_dir_all(&store_dir);
            vfs.create_dir_all(&store_dir).expect("create store dir");

            let config = GenesisConfig::new(ShireId::new("bench").expect("shire id"), 0)
                .with_pointer_replica_count(n)
                .expect("odd pointer replica count");
            let store = Store::init(vfs.clone(), &store_dir, config).expect("init store");

            let body = DocumentBody::new(vec![0xABu8; payload]);
            let document = DocumentName::new("record").expect("doc name");

            let write_one = |store: &Store, c: &mut u64| {
                let node =
                    NodePath::parse(&format!("{:016X}", mix(*c))).expect("valid hex node path");
                let nonce = Nonce::new(&format!("T{c}")).expect("valid nonce");
                *c += 1;
                let outcome = store
                    .update_document(Swap::new(node, document.clone(), body.clone()), &nonce)
                    .expect("write ok");
                assert!(outcome.is_committed(), "unconditional write must commit");
            };

            for _ in 0..WARMUP {
                write_one(&store, &mut counter);
            }

            let mut latencies: Vec<u128> = Vec::with_capacity(txns);
            let start = Instant::now();
            for _ in 0..txns {
                let t0 = Instant::now();
                write_one(&store, &mut counter);
                latencies.push(t0.elapsed().as_micros());
            }
            let elapsed = start.elapsed();

            latencies.sort_unstable();
            let tps = txns as f64 / elapsed.as_secs_f64();
            let ms = |u: u128| u as f64 / 1000.0;
            report.push_str(&format!(
                "{:>2}  {:>10}  {:>4}  {:>9.1}  {:>9.2}  {:>9.2}  {:>9.2}  {:>9.2}\n",
                n,
                fmt_bytes(payload),
                txns,
                tps,
                ms(percentile(&latencies, 50.0)),
                ms(percentile(&latencies, 90.0)),
                ms(percentile(&latencies, 99.0)),
                ms(*latencies.last().unwrap()),
            ));

            let _ = vfs.remove_dir_all(&store_dir);
        }
    }

    print!("\n{report}");

    let report_path = repo_root.join(".scratch").join("canopy-throughput.txt");
    vfs.write(&report_path, report.as_bytes())
        .expect("write report");
    vfs.fsync(&report_path).expect("fsync report");
}

fn fmt_bytes(n: usize) -> String {
    if n >= 1024 * 1024 {
        format!("{} MiB", n / (1024 * 1024))
    } else if n >= 1024 {
        format!("{} KiB", n / 1024)
    } else {
        format!("{n} B")
    }
}
