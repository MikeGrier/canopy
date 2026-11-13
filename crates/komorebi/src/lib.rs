// Copyright (c) Michael Grier.

//! `komorebi` -- the observability substrate for Canopy.
//!
//! # Filtering
//!
//! Filtering uses the `tracing_subscriber` [`EnvFilter`] directive syntax
//! (`level`, `target=level`, comma-separated). [`LogConfig`] carries a default
//! directive string plus the name of an environment variable that, when set and
//! non-empty, overrides it -- so an operator can raise or lower verbosity without
//! a rebuild while the daemon keeps a sensible built-in default. An unparseable
//! directive is dropped rather than failing startup: a mistyped filter must not
//! take the daemon down (the D56 never-fault posture).

pub mod capture;
pub mod conventions;

use std::fmt;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::writer::BoxMakeWriter;

/// Where formatted log lines are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogTarget {
    /// Standard output.
    Stdout,
    /// Standard error -- the default, so logs stay off the daemon's stdout data
    /// path.
    Stderr,
}

/// How each event is formatted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// The default single-line-per-event human-readable format.
    Full,
    /// A terser single-line format.
    Compact,
    /// A multi-line, indented format for local debugging.
    Pretty,
}

/// How the Canopy `tracing` subscriber is built and filtered.
///
/// Construct with [`LogConfig::default`], adjust with the `with_*` builders, then
/// [`install`](LogConfig::install) it once at process start. The defaults are:
/// level `info`, overridable via `RUST_LOG`, written to stderr in the full
/// format with ANSI colors.
#[derive(Debug, Clone)]
pub struct LogConfig {
    default_directives: String,
    env_var: String,
    target: LogTarget,
    format: LogFormat,
    ansi: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        LogConfig {
            default_directives: "info".to_string(),
            env_var: "RUST_LOG".to_string(),
            target: LogTarget::Stderr,
            format: LogFormat::Full,
            ansi: true,
        }
    }
}

impl LogConfig {
    /// Sets the default filter directives used when the environment variable is
    /// unset or empty (e.g. `"info"`, `"info,canopy=debug"`).
    #[must_use]
    pub fn with_default_directives(mut self, directives: impl Into<String>) -> Self {
        self.default_directives = directives.into();
        self
    }

    /// Sets the environment variable consulted to override the default
    /// directives. An empty name disables the environment override entirely.
    #[must_use]
    pub fn with_env_var(mut self, env_var: impl Into<String>) -> Self {
        self.env_var = env_var.into();
        self
    }

    /// Sets where formatted lines are written.
    #[must_use]
    pub fn with_target(mut self, target: LogTarget) -> Self {
        self.target = target;
        self
    }

    /// Sets the event format.
    #[must_use]
    pub fn with_format(mut self, format: LogFormat) -> Self {
        self.format = format;
        self
    }

    /// Sets whether ANSI color codes are emitted (disable for files or pipes).
    #[must_use]
    pub fn with_ansi(mut self, ansi: bool) -> Self {
        self.ansi = ansi;
        self
    }

    /// Builds the [`EnvFilter`] for this config: the environment variable's value
    /// when set and non-empty, otherwise the default directives, with
    /// unparseable directives dropped rather than propagated.
    fn env_filter(&self) -> EnvFilter {
        let from_env = if self.env_var.is_empty() {
            None
        } else {
            std::env::var(&self.env_var)
                .ok()
                .filter(|v| !v.trim().is_empty())
        };
        let directives = from_env.unwrap_or_else(|| self.default_directives.clone());
        EnvFilter::builder().parse_lossy(directives)
    }

    /// Builds the subscriber and installs it as the process-wide `tracing`
    /// default.
    ///
    /// Safe to attempt more than once: only the first install in a process
    /// succeeds; a later attempt returns [`InitError`] rather than panicking or
    /// replacing the live subscriber.
    pub fn install(self) -> Result<(), InitError> {
        let make_writer = match self.target {
            LogTarget::Stdout => BoxMakeWriter::new(std::io::stdout),
            LogTarget::Stderr => BoxMakeWriter::new(std::io::stderr),
        };
        let builder = tracing_subscriber::fmt()
            .with_env_filter(self.env_filter())
            .with_ansi(self.ansi)
            .with_writer(make_writer);
        let result = match self.format {
            LogFormat::Full => builder.try_init(),
            LogFormat::Compact => builder.compact().try_init(),
            LogFormat::Pretty => builder.pretty().try_init(),
        };
        result.map_err(|e| InitError {
            message: e.to_string(),
        })
    }
}

/// Installs the default Canopy subscriber (see [`LogConfig::default`]).
///
/// A convenience for the common case; use [`LogConfig`] to customize.
pub fn init() -> Result<(), InitError> {
    LogConfig::default().install()
}

/// The error returned when a `tracing` subscriber could not be installed --
/// almost always because one is already installed in this process.
#[derive(Debug, Clone)]
pub struct InitError {
    message: String,
}

impl InitError {
    /// The underlying reason, as reported by the subscriber machinery.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "could not install tracing subscriber: {}", self.message)
    }
}

impl std::error::Error for InitError {}

#[cfg(test)]
mod tests;
