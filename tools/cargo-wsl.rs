// Copyright (c) Michael Grier.

use std::io;
use std::path::{Component, Path, PathBuf, Prefix};
use std::process::{Command, ExitCode};

#[cfg(test)]
#[path = "cargo-wsl/tests.rs"]
mod tests;

fn linux_path(path: &Path) -> io::Result<String> {
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Err(io::Error::other(
            "the cargo bridge requires an absolute drive path",
        ));
    };
    let drive = match prefix.kind() {
        Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => char::from(drive).to_ascii_lowercase(),
        _ => return Err(io::Error::other("UNC paths are not mounted drive paths")),
    };
    if components.next() != Some(Component::RootDir) {
        return Err(io::Error::other(
            "the cargo bridge requires an absolute drive path",
        ));
    }
    let rest = components
        .as_path()
        .to_str()
        .ok_or_else(|| io::Error::other("WSL paths must be Unicode"))?
        .replace('\\', "/");
    Ok(format!("/mnt/{drive}/{rest}"))
}

fn quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn execution_root(host: &Path, native: Option<&str>) -> io::Result<String> {
    match native {
        Some(path) if path.starts_with('/') && !path.contains('\0') => Ok(path.to_owned()),
        Some(_) => Err(io::Error::other(
            "CANOPY_WSL_ROOT must be an absolute Linux path",
        )),
        None => linux_path(host),
    }
}

fn forward() -> io::Result<ExitCode> {
    let native_root = std::env::var("CANOPY_WSL_ROOT").ok();
    let root = execution_root(&std::env::current_dir()?, native_root.as_deref())?;
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".cargo")))
        .ok_or_else(|| io::Error::other("CARGO_HOME or USERPROFILE is required"))?;
    let home = linux_path(&home)?;
    let distribution = std::env::var("CANOPY_WSL_DISTRO").unwrap_or_else(|_| "Ubuntu".into());
    let target =
        std::env::var("CANOPY_WSL_TARGET_DIR").unwrap_or_else(|_| "/tmp/canopy-linux".into());
    let executable = std::env::current_exe()?;
    let program = if executable.file_stem().is_some_and(|name| name == "rustc") {
        "rustc"
    } else {
        "cargo"
    };
    let arguments: Vec<_> = std::env::args()
        .skip(1)
        .map(|argument| quoted(&argument))
        .collect();
    let script = format!(
        "cd {} && exec env -u RUSTC -u RUSTUP_TOOLCHAIN CARGO_HOME={} CARGO_TARGET_DIR={} {program} {}",
        quoted(&root),
        quoted(&home),
        quoted(&target),
        arguments.join(" ")
    );
    let status = Command::new("wsl.exe")
        .args(["-d", &distribution, "--", "bash", "-lc", &script])
        .status()?;
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .map_or(ExitCode::FAILURE, ExitCode::from))
}

fn main() -> ExitCode {
    match forward() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("cargo-wsl: {error}");
            ExitCode::FAILURE
        }
    }
}
