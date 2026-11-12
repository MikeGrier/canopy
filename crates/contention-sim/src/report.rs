// Copyright (c) Michael Grier.

//! The one place the simulator writes its report: a sink that lines go to.

use std::io::Write;

pub trait Sink {
    fn line(&mut self, text: &str);
}

/// Every line to standard output and to a file.
pub struct Tee {
    file: std::fs::File,
}

impl Tee {
    pub fn create(path: &std::path::Path) -> std::io::Result<Tee> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(Tee {
            file: std::fs::File::create(path)?,
        })
    }
}

impl Sink for Tee {
    fn line(&mut self, text: &str) {
        println!("{text}");
        let _ = writeln!(self.file, "{text}");
    }
}

/// Reports that the report cannot be written, and ends the process.
pub fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(2);
}

/// Lines kept in memory, for tests.
#[derive(Default)]
pub struct Collect {
    pub lines: Vec<String>,
}

impl Sink for Collect {
    fn line(&mut self, text: &str) {
        self.lines.push(text.to_string());
    }
}
