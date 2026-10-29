//! Tests that run the `tv` binary. Only cases that exit before the viewer
//! starts, since the viewer needs a real terminal.

use std::io::Write;
use std::process::{Command, Output, Stdio};

/// Run `tv` with `args`, feeding `stdin` to it.
fn tv(args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tv"))
        .args(args)
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

#[test]
fn bad_json_on_stdin_exits_1() {
    let output = tv(&["-"], "not json");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("parsing stdin"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_missing_file_exits_1() {
    let output = tv(&["missing.json"], "");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("reading missing.json"),
        "{}",
        stderr(&output)
    );
}
