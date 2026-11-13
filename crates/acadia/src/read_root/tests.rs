// Copyright (c) Michael Grier.

use super::*;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "acadia-read-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn root(&self) -> OsReadRoot {
        OsReadRoot::open(&self.0).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn ten_regular_files_and_nested_paths() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("nested")).unwrap();
    let root = fixture.root();
    for index in 0..10 {
        let path = format!("nested/file-{index}");
        let bytes = vec![index as u8; index * 100];
        fs::write(fixture.0.join(&path), &bytes).unwrap();
        let mut result = Vec::new();
        root.open_file(Path::new(&path))
            .unwrap()
            .read_to_end(&mut result)
            .unwrap();
        assert_eq!(result, bytes);
    }
}

#[test]
fn missing_directory_and_escape_fail() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("nested")).unwrap();
    let root = fixture.root();
    for path in [
        "",
        ".",
        "..",
        "../outside",
        "/outside",
        "missing",
        "nested",
        "nested/../outside",
    ] {
        assert!(root.open_file(Path::new(path)).is_err(), "{path}");
    }
    assert!(OsReadRoot::open(Path::new("relative")).is_err());
}

#[test]
fn opened_file_is_not_reopened_after_name_replacement() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("file"), b"original").unwrap();
    let mut file = fixture.root().open_file(Path::new("file")).unwrap();
    fs::rename(fixture.0.join("file"), fixture.0.join("old")).unwrap();
    fs::write(fixture.0.join("file"), b"replacement").unwrap();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"original");
}

#[test]
fn directory_handle_survives_root_name_replacement() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("root")).unwrap();
    fs::write(fixture.0.join("root/file"), b"original").unwrap();
    let root = OsReadRoot::open(&fixture.0.join("root")).unwrap();
    match fs::rename(fixture.0.join("root"), fixture.0.join("old")) {
        Ok(()) => {
            fs::create_dir(fixture.0.join("root")).unwrap();
            fs::write(fixture.0.join("root/file"), b"replacement").unwrap();
        }
        #[cfg(windows)]
        Err(error) => {
            const ERROR_SHARING_VIOLATION: i32 = 32;
            assert_eq!(error.raw_os_error(), Some(ERROR_SHARING_VIOLATION));
        }
        #[cfg(not(windows))]
        Err(error) => panic!("directory rename: {error}"),
    }
    let mut bytes = Vec::new();
    root.open_file(Path::new("file"))
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"original");
}

#[test]
fn hard_links_are_read_as_regular_files() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("file"), b"bytes").unwrap();
    fs::hard_link(fixture.0.join("file"), fixture.0.join("alias")).unwrap();
    let mut bytes = Vec::new();
    fixture
        .root()
        .open_file(Path::new("alias"))
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"bytes");
}

#[cfg(unix)]
#[test]
fn unix_links_in_root_intermediate_and_leaf_are_refused() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("dir")).unwrap();
    fs::write(fixture.0.join("dir/file"), b"bytes").unwrap();
    symlink("dir", fixture.0.join("link")).unwrap();
    symlink("dir/file", fixture.0.join("leaf")).unwrap();
    let root = fixture.root();
    assert!(root.open_file(Path::new("link/file")).is_err());
    assert!(root.open_file(Path::new("leaf")).is_err());
    assert!(OsReadRoot::open(&fixture.0.join("link")).is_err());
}

#[cfg(unix)]
#[test]
fn fifo_is_rejected_without_blocking_on_open() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let fixture = Fixture::new();
    let path = CString::new(fixture.0.join("fifo").as_os_str().as_bytes()).unwrap();
    assert_eq!(
        unsafe { libc::mkfifo(path.as_ptr(), libc::S_IRUSR | libc::S_IWUSR) },
        0
    );
    assert!(fixture.root().open_file(Path::new("fifo")).is_err());
}

#[cfg(windows)]
#[test]
fn junction_root_and_intermediate_are_refused() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("dir")).unwrap();
    fs::write(fixture.0.join("dir/file"), b"outside").unwrap();
    let output = std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(fixture.0.join("junction"))
        .arg(fixture.0.join("dir"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fixture
            .root()
            .open_file(Path::new("junction/file"))
            .is_err()
    );
    assert!(OsReadRoot::open(&fixture.0.join("junction")).is_err());
}
