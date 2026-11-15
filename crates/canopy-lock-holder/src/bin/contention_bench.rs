// Copyright (c) Michael Grier.

mod contention {
    pub mod campaign;
    pub mod clock;
    pub mod metrics;
    pub mod timed;
    pub mod worker;
}

fn main() {
    let reporter = std::sync::Arc::new(contention::worker::Reporter {
        out: std::sync::Mutex::new(Box::new(std::io::stdout())),
        err: std::sync::Mutex::new(Box::new(std::io::stderr())),
    });
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = if args.first().is_some_and(|arg| arg == "worker") {
        (|| {
            let root = std::path::PathBuf::from(
                args.get(1)
                    .ok_or_else(|| std::io::Error::other("worker root"))?,
            );
            let id = args
                .get(2)
                .ok_or_else(|| std::io::Error::other("worker id"))?
                .parse::<usize>()
                .map_err(std::io::Error::other)?;
            let role = args
                .get(3)
                .ok_or_else(|| std::io::Error::other("worker role"))?;
            let polled = args.get(4).is_some_and(|arg| arg == "polled");
            let callback = args
                .get(5)
                .ok_or_else(|| std::io::Error::other("callback seconds"))?
                .parse::<f64>()
                .map_err(std::io::Error::other)?;
            contention::worker::run(
                root,
                id,
                role,
                polled,
                callback,
                std::sync::Arc::clone(&reporter),
            )
        })()
    } else {
        contention::campaign::run(&args, &reporter)
    };
    if let Err(error) = result {
        reporter.error(&error.to_string());
        std::process::exit(1);
    }
}
