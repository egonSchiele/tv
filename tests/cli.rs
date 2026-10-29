//! Tests that run the `tv` binary. Only cases that exit before the viewer
//! starts, since the viewer needs a real terminal.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
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

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Assert `tv` exited 0 and return its stdout.
fn succeeds(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(0), "{}", stderr(output));
    stdout(output)
}

/// Make session `id` under `home` by writing its `table.json` directly, the
/// way any other writer would. No command makes one without the viewer until
/// `tv agent new`, and this also pins down the on-disk layout.
fn write_session(home: &Path, id: &str, table: &str) -> PathBuf {
    let dir = home.join("sessions").join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("table.json"), table).unwrap();
    dir
}

fn example() -> String {
    fs::read_to_string("examples/prs.json").unwrap()
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

#[test]
fn ls_with_no_sessions() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(succeeds(&tv(home.path(), &["ls"], "")), "no sessions\n");
}

#[test]
fn ls_lists_sessions_and_survives_a_broken_one() {
    let home = tempfile::tempdir().unwrap();
    write_session(home.path(), "abc123", &example());
    write_session(home.path(), "bad000", "not json");
    let listing = succeeds(&tv(home.path(), &["ls"], ""));
    let good = listing
        .lines()
        .find(|line| line.starts_with("abc123"))
        .unwrap();
    assert!(good.contains("40×9"), "{listing}");
    assert!(good.contains("Open pull requests"), "{listing}");
    let bad = listing
        .lines()
        .find(|line| line.starts_with("bad000"))
        .unwrap();
    assert!(bad.contains("(can't read table.json)"), "{listing}");

    let pruning = succeeds(&tv(
        home.path(),
        &["session", "prune", "--older-than", "0d", "--dry-run"],
        "",
    ));
    assert!(pruning.contains("would remove abc123"), "{pruning}");
    assert!(pruning.contains("would remove bad000"), "{pruning}");
}

#[test]
fn save_writes_the_table_to_a_file() {
    let home = tempfile::tempdir().unwrap();
    write_session(home.path(), "abc123", &example());
    let out = home.path().join("out.json");
    let printed = succeeds(&tv(
        home.path(),
        &["save", "abc123", out.to_str().unwrap()],
        "",
    ));
    assert!(printed.starts_with("wrote 40 rows to "), "{printed}");
    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(saved["rows"].as_array().unwrap().len(), 40);
}

#[test]
fn prune_by_age() {
    let home = tempfile::tempdir().unwrap();
    let dir = write_session(home.path(), "abc123", &example());
    let prune = |args: &[&str]| tv(home.path(), &[&["session", "prune"], args].concat(), "");

    let output = succeeds(&prune(&["--older-than", "7d", "--dry-run"]));
    assert_eq!(output, "nothing to prune\n");
    assert!(dir.exists());

    let output = succeeds(&prune(&["--older-than", "0d", "--dry-run"]));
    assert!(
        output.contains("would remove abc123  Open pull requests"),
        "{output}"
    );
    assert!(dir.exists());

    let output = succeeds(&prune(&["--older-than", "0d"]));
    assert!(
        output.contains("removed abc123  Open pull requests"),
        "{output}"
    );
    assert!(!dir.exists());
}

#[test]
fn prune_needs_a_filter() {
    let home = tempfile::tempdir().unwrap();
    assert_fails_with(&tv(home.path(), &["session", "prune"], ""), "give a filter");
}

#[test]
fn rm_removes_sessions() {
    let home = tempfile::tempdir().unwrap();
    let dir = write_session(home.path(), "abc123", &example());
    assert_eq!(
        succeeds(&tv(home.path(), &["rm", "abc123"], "")),
        "removed abc123\n"
    );
    assert!(!dir.exists());

    // Naming it twice removes it once.
    let dir = write_session(home.path(), "abc123", &example());
    let output = succeeds(&tv(home.path(), &["rm", "abc123", "abc123"], ""));
    assert_eq!(output, "removed abc123\n");
    assert!(!dir.exists());

    assert_fails_with(
        &tv(home.path(), &["rm", "zzzzzz"], ""),
        "no session \"zzzzzz\"",
    );
}

#[test]
fn rm_checks_every_id_before_deleting_any() {
    let home = tempfile::tempdir().unwrap();
    let dir = write_session(home.path(), "abc123", &example());
    assert_fails_with(
        &tv(home.path(), &["rm", "abc123", "zzzzzz"], ""),
        "no session \"zzzzzz\"",
    );
    assert!(dir.join("table.json").exists());
}

#[test]
fn rm_refuses_a_directory_with_a_file_it_doesnt_know() {
    let home = tempfile::tempdir().unwrap();
    let dir = write_session(home.path(), "abc123", &example());
    fs::write(dir.join("notes.txt"), "mine").unwrap();
    assert_fails_with(
        &tv(home.path(), &["rm", "abc123"], ""),
        "refusing to delete",
    );
    assert!(dir.join("notes.txt").exists());
    assert!(dir.join("table.json").exists());
    assert!(succeeds(&tv(home.path(), &["ls"], "")).contains("abc123"));
}
