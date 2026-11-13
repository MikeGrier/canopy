// Copyright (c) Michael Grier.

use super::*;

#[test]
fn the_handshake_is_one_line_on_out_and_nothing_on_err() {
    let mut report = Reporter {
        out: Vec::new(),
        err: Vec::new(),
    };
    report.ready().unwrap();
    assert_eq!(report.out, b"locked\n");
    assert!(report.err.is_empty());
}

#[test]
fn a_failure_is_reported_on_err_and_never_looks_like_the_handshake() {
    let mut report = Reporter {
        out: Vec::new(),
        err: Vec::new(),
    };
    let code = report.fail(format_args!("open failed: {}", "no such root"));
    assert_eq!(code, ExitCode::FAILURE);
    assert!(
        report.out.is_empty(),
        "a failure must not be mistaken for acquisition"
    );
    assert_eq!(report.err, b"open failed: no such root\n");
}

#[test]
fn a_handshake_that_cannot_be_delivered_is_an_error() {
    struct Closed;
    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
    }
    let mut report = Reporter {
        out: Closed,
        err: Vec::new(),
    };
    assert!(report.ready().is_err());
}
