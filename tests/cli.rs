//! Tests that run the `tv` binary. Only cases that exit before the viewer
//! starts, since the viewer needs a real terminal.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// Run `tv` with `args` and `TV_HOME` set to `home`, feeding `stdin` to it.
fn tv(home: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tv"))
        .args(args)
        .env("TV_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Assert `tv` failed with exit status 1 and `expected` in its stderr.
fn assert_fails_with(output: &Output, expected: &str) {
    assert_eq!(output.status.code(), Some(1), "{}", stderr(output));
    assert!(stderr(output).contains(expected), "{}", stderr(output));
}

#[test]
fn bad_json_on_stdin_exits_1_and_makes_no_session() {
    let home = tempfile::tempdir().unwrap();
    let output = tv(home.path(), &["-"], "not json");
    assert_fails_with(&output, "parsing stdin");
    assert!(!home.path().join("sessions").exists());
}

#[test]
fn a_missing_file_exits_1_and_makes_no_session() {
    let home = tempfile::tempdir().unwrap();
    let output = tv(home.path(), &["missing.json"], "");
    assert_fails_with(&output, "reading missing.json");
    assert!(!home.path().join("sessions").exists());
}

#[test]
fn bare_tv_with_no_sessions_exits_1() {
    let home = tempfile::tempdir().unwrap();
    assert_fails_with(&tv(home.path(), &[], ""), "no sessions yet");
}

#[test]
fn an_unknown_session_exits_1() {
    let home = tempfile::tempdir().unwrap();
    assert_fails_with(&tv(home.path(), &["zzzzzz"], ""), "no session \"zzzzzz\"");
}
