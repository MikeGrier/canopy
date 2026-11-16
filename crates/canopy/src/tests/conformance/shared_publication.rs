// Copyright (c) Michael Grier.

mod action;
mod process;
mod projection;
mod replay;
mod tests;

use super::corpus::Trace;
use process::Implementation;

fn judge(trace: &Trace, implementation: Implementation) -> Result<(), String> {
    match (replay::replay(trace, implementation), &trace.departs) {
        (Ok(()), None) => Ok(()),
        (Ok(()), Some(expected)) => Err(format!(
            "the Store followed the trace; expected a departure at {expected}"
        )),
        (Err(departure), Some(expected)) if trace.steps[departure.step].id == *expected => Ok(()),
        (Err(departure), _) => Err(format!(
            "{}: {}",
            trace.at(departure.step),
            departure.detail
        )),
    }
}
