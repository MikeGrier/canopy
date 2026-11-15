// Copyright (c) Michael Grier.

use super::*;

#[test]
fn native_execution_roots_are_explicit_absolute_and_do_not_change_the_default() {
    let host = Path::new(r"Q:\github\canopy");
    assert_eq!(execution_root(host, None).unwrap(), "/mnt/q/github/canopy");
    for path in [
        "/home/user/github/canopy",
        "/tmp/with spaces",
        "/tmp/with'quotes",
        "/tmp/$literal",
        "/tmp/a;b",
        "/tmp/a&b",
        "/tmp/[name]",
        "/tmp/a|b",
        "/tmp/one-two",
        "/",
        "/tmp/with\\backslash",
        "/tmp/two  spaces",
    ] {
        assert_eq!(execution_root(host, Some(path)).unwrap(), path);
    }
    for path in ["", "relative", "C:/windows", "~/repo", "/tmp/bad\0path"] {
        assert!(execution_root(host, Some(path)).is_err());
    }
}

#[test]
fn absolute_drive_paths_preserve_components_and_normalize_the_mount_drive() {
    for (input, expected) in [
        (r"C:\one", "/mnt/c/one"),
        (r"q:\github\canopy", "/mnt/q/github/canopy"),
        (r"Q:\github\canopy", "/mnt/q/github/canopy"),
        (r"D:\with spaces\repo", "/mnt/d/with spaces/repo"),
        (r"E:\with'quotes", "/mnt/e/with'quotes"),
        (r"F:\$literal", "/mnt/f/$literal"),
        (r"G:\semi;colon", "/mnt/g/semi;colon"),
        (r"H:\brackets[1]", "/mnt/h/brackets[1]"),
        (r"I:\dash-name", "/mnt/i/dash-name"),
        (r"J:\", "/mnt/j/"),
        ("K:/forward/slashes", "/mnt/k/forward/slashes"),
        (r"\\?\L:\verbatim", "/mnt/l/verbatim"),
    ] {
        assert_eq!(linux_path(Path::new(input)).unwrap(), expected);
    }
}

#[test]
fn relative_root_relative_and_network_paths_are_refused() {
    for input in [
        "",
        ".",
        "relative",
        r"C:relative",
        r"\root-relative",
        r"\\server\share\root",
        r"\\?\UNC\server\share\root",
    ] {
        assert!(linux_path(Path::new(input)).is_err(), "{input}");
    }
}

#[test]
fn bash_arguments_keep_metacharacters_literal() {
    for (input, expected) in [
        ("", "''"),
        ("test", "'test'"),
        ("--offline", "'--offline'"),
        ("with spaces", "'with spaces'"),
        ("two  spaces", "'two  spaces'"),
        ("$HOME", "'$HOME'"),
        ("$@", "'$@'"),
        ("a;b", "'a;b'"),
        ("a&b", "'a&b'"),
        ("a|b", "'a|b'"),
        ("[glob]*", "'[glob]*'"),
        ("a'b", "'a'\\''b'"),
        ("\"quoted\"", "'\"quoted\"'"),
    ] {
        assert_eq!(quoted(input), expected);
    }
}
