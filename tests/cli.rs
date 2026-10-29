//! Tests that run the `tv` binary. Only cases that exit before the viewer
//! starts, since the viewer needs a real terminal.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

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
/// way any other writer would. Unlike `tv agent new`, it picks the ID and can
/// write a `table.json` no command would, such as a broken one. It also pins
/// down the on-disk layout.
fn write_session(home: &Path, id: &str, table: &str) -> PathBuf {
    let dir = home.join("sessions").join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("table.json"), table).unwrap();
    dir
}

/// Fits an `i32`, but no system hands it out (Linux caps pids at 4,194,304
/// and macOS at 99,998), so `kill` says `ESRCH`.
const NO_SUCH_PID: u32 = 2_147_483_647;

/// Write `view.json` into session directory `dir` the way a viewer with
/// process `pid` would, every field included so it parses.
fn write_view(dir: &Path, pid: u32) {
    write_view_at(dir, pid, "412", "title");
}

/// `write_view`, with the cursor on `row_id` and `column`.
fn write_view_at(dir: &Path, pid: u32, row_id: &str, column: &str) {
    let view = format!(
        r#"{{"pid": {pid}, "row_id": "{row_id}", "column": "{column}", "updated_at": "2026-10-28T19:02:11Z"}}"#
    );
    fs::write(dir.join("view.json"), view).unwrap();
}

/// Write `comments.jsonl` into session directory `dir` by hand, one line per
/// entry, the way the viewer and agent append it.
fn write_comments(dir: &Path, lines: &[&str]) {
    fs::write(dir.join("comments.jsonl"), lines.join("\n") + "\n").unwrap();
}

/// A user's comment on PR 404's title in `examples/prs.json`, unanswered.
const USER_COMMENT: &str = r#"{"op":"add","id":"c1","author":"user","row_id":"404","column":"title","text":"why?","ts":"2026-10-28T19:03:00Z"}"#;
/// The agent's reply to it.
const AGENT_REPLY: &str = r#"{"op":"add","id":"c2","author":"agent","reply_to":"c1","text":"waiting","ts":"2026-10-28T19:04:00Z"}"#;

/// This test's process: alive for as long as the test runs, so a `view.json`
/// naming it is an open viewer.
fn open_viewer(dir: &Path) {
    write_view(dir, std::process::id());
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
    assert!(
        saved.get("comments").is_none(),
        "no log, no comments: {saved}"
    );
}

#[test]
fn save_writes_the_threads_with_their_derived_fields() {
    let home = tempfile::tempdir().unwrap();
    let dir = write_session(home.path(), "abc123", &example());
    write_comments(&dir, &[USER_COMMENT, AGENT_REPLY]);
    let out = home.path().join("out.json");
    let printed = succeeds(&tv(
        home.path(),
        &["save", "abc123", out.to_str().unwrap()],
        "",
    ));
    assert_eq!(printed.lines().count(), 1, "no warning: {printed}");
    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&out).unwrap()).unwrap();
    let thread = &saved["comments"][0];
    assert_eq!(thread["id"], "c1");
    assert_eq!(thread["value"], "Bump serde to 1.0.229");
    assert_eq!(thread["awaiting_reply"], false);
    assert_eq!(thread["orphaned"], false);
    assert_eq!(thread["messages"].as_array().unwrap().len(), 2);
}

#[test]
fn save_reports_skipped_log_lines() {
    let home = tempfile::tempdir().unwrap();
    let dir = write_session(home.path(), "abc123", &example());
    write_comments(&dir, &[USER_COMMENT, "not json", AGENT_REPLY]);
    let out = home.path().join("out.json");
    let printed = succeeds(&tv(
        home.path(),
        &["save", "abc123", out.to_str().unwrap()],
        "",
    ));
    let lines: Vec<&str> = printed.lines().collect();
    assert!(lines[0].starts_with("wrote 40 rows to "), "{printed}");
    assert_eq!(
        lines[1],
        "comments.jsonl: skipped lines that aren't events: 2"
    );
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
    assert_fails_with(
        &tv(home.path(), &["session", "prune"], ""),
        "give a filter: --older-than <duration> and/or --closed",
    );
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

#[test]
fn ls_shows_which_viewers_are_open() {
    let home = tempfile::tempdir().unwrap();
    open_viewer(&write_session(home.path(), "open00", &example()));
    write_view(
        &write_session(home.path(), "dead00", &example()),
        NO_SUCH_PID,
    );
    write_session(home.path(), "none00", &example());
    let listing = succeeds(&tv(home.path(), &["ls"], ""));
    let viewer = |id: &str| {
        let line = listing.lines().find(|line| line.starts_with(id)).unwrap();
        line.split_whitespace().nth(2).unwrap().to_string()
    };
    assert!(
        listing.starts_with("ID      ROWS×COLS  VIEWER  AWAITING  MODIFIED"),
        "{listing}"
    );
    assert_eq!(viewer("open00"), "open", "{listing}");
    assert_eq!(viewer("dead00"), "-", "{listing}");
    assert_eq!(viewer("none00"), "-", "{listing}");
}

#[test]
fn ls_counts_threads_awaiting_a_reply() {
    let home = tempfile::tempdir().unwrap();
    write_comments(
        &write_session(home.path(), "user00", &example()),
        &[USER_COMMENT],
    );
    write_comments(
        &write_session(home.path(), "agent0", &example()),
        &[USER_COMMENT, AGENT_REPLY],
    );
    write_comments(
        &write_session(home.path(), "broken", "not json"),
        &[USER_COMMENT],
    );
    let listing = succeeds(&tv(home.path(), &["ls"], ""));
    let awaiting = |id: &str| {
        let line = listing.lines().find(|line| line.starts_with(id)).unwrap();
        line.split_whitespace().nth(3).unwrap().to_string()
    };
    assert_eq!(awaiting("user00"), "1", "{listing}");
    assert_eq!(awaiting("agent0"), "-", "{listing}");
    assert_eq!(awaiting("broken"), "?", "{listing}");
}

#[test]
fn rm_refuses_an_open_session_without_force() {
    let home = tempfile::tempdir().unwrap();
    let dir = write_session(home.path(), "abc123", &example());
    open_viewer(&dir);
    assert_fails_with(
        &tv(home.path(), &["rm", "abc123"], ""),
        "session abc123 has a viewer open; close it or use --force",
    );
    assert!(dir.join("table.json").exists());

    let output = succeeds(&tv(home.path(), &["rm", "--force", "abc123"], ""));
    assert_eq!(output, "removed abc123\n");
    assert!(!dir.exists());
}

#[test]
fn prune_never_removes_an_open_session() {
    let home = tempfile::tempdir().unwrap();
    let open = write_session(home.path(), "open00", &example());
    open_viewer(&open);
    let closed = write_session(home.path(), "shut00", &example());
    write_view(&closed, NO_SUCH_PID);
    let prune = |args: &[&str]| tv(home.path(), &[&["session", "prune"], args].concat(), "");

    let output = succeeds(&prune(&["--closed", "--dry-run"]));
    assert_eq!(output, "would remove shut00  Open pull requests\n");

    let output = succeeds(&prune(&["--older-than", "0d"]));
    assert!(output.contains("skipped open00  (viewer open)"), "{output}");
    assert!(
        output.contains("removed shut00  Open pull requests"),
        "{output}"
    );
    assert!(open.join("table.json").exists());
    assert!(!closed.exists());

    // Only the open one is left: skipped, and nothing pruned.
    let output = succeeds(&prune(&["--older-than", "0d"]));
    assert_eq!(output, "skipped open00  (viewer open)\nnothing to prune\n");
    // `--closed` doesn't match it at all, so there's no skipped line.
    assert_eq!(succeeds(&prune(&["--closed"])), "nothing to prune\n");
}

// `tv agent`

/// Run `tv agent <args>`; assert it exited 0 and printed exactly one line,
/// and return that line parsed as JSON. Every successful agent call in these
/// tests goes through here, so every command's output is checked this way.
fn agent(home: &Path, args: &[&str], stdin: &str) -> Value {
    let printed = succeeds(&tv(home, &[&["agent"], args].concat(), stdin));
    assert_eq!(printed.lines().count(), 1, "{printed}");
    assert!(printed.ends_with('\n'), "{printed:?}");
    serde_json::from_str(&printed).unwrap()
}

/// `tv agent <args>`, for a call that should fail.
fn agent_output(home: &Path, args: &[&str], stdin: &str) -> Output {
    tv(home, &[&["agent"], args].concat(), stdin)
}

/// `examples/prs.json`'s rows as a bare array, the way `gh … --json` prints
/// them.
fn example_rows() -> Value {
    let table: Value = serde_json::from_str(&example()).unwrap();
    table["rows"].clone()
}

/// A session made by `tv agent new` from `example_rows()`, keyed on `number`
/// and titled `PRs`. Returns its ID.
fn new_session(home: &Path) -> String {
    let rows = example_rows().to_string();
    let created = agent(home, &["new", "--key", "number", "--title", "PRs"], &rows);
    created["session"].as_str().unwrap().to_string()
}

fn session_dir(home: &Path, id: &str) -> PathBuf {
    home.join("sessions").join(id)
}

#[test]
fn agent_new_reads_stdin_and_prints_how_to_open_it() {
    let home = tempfile::tempdir().unwrap();
    let rows = example_rows().to_string();
    let created = agent(
        home.path(),
        &["new", "--key", "number", "--title", "PRs"],
        &rows,
    );
    let id = created["session"].as_str().unwrap();
    assert_eq!(id.len(), 6, "{created}");
    assert_eq!(
        (created["rows"].as_u64(), created["columns"].as_u64()),
        (Some(40), Some(9))
    );
    // The tests set `TV_HOME`, so the command sets it too.
    let expected = format!("TV_HOME='{}' tv open {id}", home.path().display());
    assert_eq!(created["open_command"], expected.as_str());
    let table = agent(home.path(), &["get", id], "");
    assert_eq!(
        (table["key"].as_str(), table["title"].as_str()),
        (Some("number"), Some("PRs"))
    );
    assert!(succeeds(&tv(home.path(), &["ls"], "")).contains(id));
}

#[test]
fn agent_new_reads_a_file_or_dash() {
    let home = tempfile::tempdir().unwrap();
    let file = home.path().join("rows.json");
    fs::write(&file, example_rows().to_string()).unwrap();
    let from_file = agent(home.path(), &["new", file.to_str().unwrap()], "");
    assert_eq!(from_file["rows"], 40);
    let from_dash = agent(home.path(), &["new", "-"], &example_rows().to_string());
    assert_eq!(from_dash["rows"], 40);
    assert_ne!(from_file["session"], from_dash["session"]);
}

#[test]
fn agent_new_without_a_table_makes_no_session() {
    let home = tempfile::tempdir().unwrap();
    assert_fails_with(&agent_output(home.path(), &["new"], ""), "parsing stdin");
    let missing = agent_output(home.path(), &["new", "no/such.json"], "");
    assert_fails_with(&missing, "reading no/such.json");
    assert!(!home.path().join("sessions").exists());
}

#[test]
fn agent_new_imports_a_saved_files_comments() {
    let home = tempfile::tempdir().unwrap();
    let mut table: Value = serde_json::from_str(&example()).unwrap();
    table["comments"] = serde_json::json!([{
        "id": "c1", "row_id": "404", "column": "title", "messages": [
            {"id": "c1", "author": "user", "text": "why?", "ts": "2026-10-28T19:03:00Z"}
        ]
    }]);
    let created = agent(home.path(), &["new"], &table.to_string());
    let listed = agent(home.path(), &["sessions"], "");
    assert_eq!(listed[0]["session"], created["session"]);
    assert_eq!(listed[0]["awaiting_reply"], 1);
}

#[test]
fn agent_push_keeps_the_key_and_title() {
    let home = tempfile::tempdir().unwrap();
    let id = new_session(home.path());
    let mut rows = example_rows();
    rows[1]["title"] = "Bump serde".into();
    let pushed = agent(home.path(), &["push", &id], &rows.to_string());
    assert_eq!(pushed["session"], id.as_str());
    let table = agent(home.path(), &["get", &id], "");
    assert_eq!(
        (table["key"].as_str(), table["title"].as_str()),
        (Some("number"), Some("PRs"))
    );
    assert_eq!(table["rows"][1]["title"], "Bump serde");

    // So a repeated key is still caught, and the error says how to drop it.
    rows[2]["number"] = rows[1]["number"].clone();
    let output = agent_output(home.path(), &["push", &id], &rows.to_string());
    assert_fails_with(&output, "duplicate key value \"404\"");
    assert!(
        stderr(&output).contains("\"key\": null"),
        "{}",
        stderr(&output)
    );
    assert_eq!(agent(home.path(), &["get", &id], ""), table);
}

#[test]
fn agent_push_with_key_null_drops_the_key() {
    let home = tempfile::tempdir().unwrap();
    let id = new_session(home.path());
    let table = serde_json::json!({"key": null, "rows": [{"n": 1}, {"n": 1}]});
    agent(home.path(), &["push", &id], &table.to_string());
    let table = agent(home.path(), &["get", &id], "");
    assert_eq!(table.get("key"), None, "{table}");
    assert_eq!(table["title"], "PRs");
}

#[test]
fn agent_push_rejects_and_keeps_the_table() {
    let home = tempfile::tempdir().unwrap();
    let id = new_session(home.path());
    let before = agent(home.path(), &["get", &id], "");
    let rows = example_rows().to_string();

    let output = agent_output(home.path(), &["push", &id, "--key", "nope"], &rows);
    assert_fails_with(&output, "key column \"nope\" is not one of the columns");

    let mut with_comments: Value = serde_json::from_str(&example()).unwrap();
    with_comments["comments"] = serde_json::json!([{
        "id": "c1", "row_id": "404", "column": "title", "messages": []
    }]);
    let output = agent_output(home.path(), &["push", &id], &with_comments.to_string());
    assert_fails_with(&output, "push doesn't change comments");
    assert!(
        !session_dir(home.path(), &id)
            .join("comments.jsonl")
            .exists()
    );

    assert_fails_with(
        &agent_output(home.path(), &["push", &id], "not json"),
        "parsing stdin",
    );
    assert_fails_with(
        &agent_output(home.path(), &["push", "nope00"], &rows),
        "no session",
    );
    assert_eq!(agent(home.path(), &["get", &id], ""), before);
}

#[test]
fn agent_push_replaces_a_broken_table() {
    let home = tempfile::tempdir().unwrap();
    write_session(home.path(), "abc123", "not json");
    let rows = r#"[{"id": 1}]"#;
    agent(home.path(), &["push", "abc123", "--key", "id"], rows);
    let table = agent(home.path(), &["get", "abc123"], "");
    assert_eq!(table["rows"], serde_json::json!([{"id": 1}]));
}

#[test]
fn agent_get_prints_input_for_push() {
    let home = tempfile::tempdir().unwrap();
    let first = new_session(home.path());
    let table = agent(home.path(), &["get", &first], "");
    assert_eq!(table.get("comments"), None, "{table}");
    let second = agent(home.path(), &["new"], r#"[{"x": 1}]"#);
    let second = second["session"].as_str().unwrap();
    agent(home.path(), &["push", second], &table.to_string());
    assert_eq!(agent(home.path(), &["get", second], ""), table);
}

#[test]
fn agent_sessions_lists_every_session() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(agent(home.path(), &["sessions"], ""), serde_json::json!([]));

    write_comments(
        &write_session(home.path(), "user00", &example()),
        &[USER_COMMENT],
    );
    open_viewer(&write_session(home.path(), "open00", &example()));
    write_session(home.path(), "broken", "not json");
    let listed = agent(home.path(), &["sessions"], "");
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 3);
    let entry = |id: &str| {
        listed
            .iter()
            .find(|entry| entry["session"] == id)
            .unwrap()
            .clone()
    };
    let user = entry("user00");
    assert_eq!(user["title"], "Open pull requests");
    assert_eq!(
        (user["rows"].as_u64(), user["columns"].as_u64()),
        (Some(40), Some(9))
    );
    assert_eq!(user["awaiting_reply"], 1);
    assert_eq!(user["viewer_open"], false);
    assert!(
        user["modified_at"].as_str().unwrap().ends_with('Z'),
        "{user}"
    );
    // The spec's field order.
    let keys: Vec<&str> = user
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "session",
            "title",
            "rows",
            "columns",
            "viewer_open",
            "awaiting_reply",
            "modified_at"
        ]
    );
    assert_eq!(entry("open00")["viewer_open"], true);
    let broken = entry("broken");
    assert_eq!(
        (broken["rows"].clone(), broken["title"].clone()),
        (Value::Null, Value::Null)
    );
    assert_eq!(broken["awaiting_reply"], Value::Null);
}

#[test]
fn agent_focus_without_a_viewer_is_all_nulls() {
    let home = tempfile::tempdir().unwrap();
    let id = new_session(home.path());
    let printed = succeeds(&agent_output(home.path(), &["focus", &id], ""));
    assert_eq!(
        printed,
        "{\"viewer_open\":false,\"row_id\":null,\"column\":null,\"value\":null,\"row\":null}\n"
    );
}

#[test]
fn agent_focus_reports_the_viewers_cell() {
    let home = tempfile::tempdir().unwrap();
    let dir = write_session(home.path(), "abc123", &example());
    write_view_at(&dir, std::process::id(), "404", "title");
    let focus = agent(home.path(), &["focus", "abc123"], "");
    assert_eq!(focus["viewer_open"], true);
    assert_eq!(
        (focus["row_id"].as_str(), focus["column"].as_str()),
        (Some("404"), Some("title"))
    );
    assert_eq!(focus["value"], "Bump serde to 1.0.229");
    assert_eq!(focus["row"]["number"], 404);

    // `write_view`'s row 412 isn't in `prs.json`: a push removed it.
    open_viewer(&dir);
    let focus = agent(home.path(), &["focus", "abc123"], "");
    assert_eq!(focus["row_id"], "412");
    assert_eq!(
        (focus["value"].clone(), focus["row"].clone()),
        (Value::Null, Value::Null)
    );
}

/// The last line of session directory `dir`'s comment log, parsed.
fn last_log_line(dir: &Path) -> Value {
    let log = fs::read_to_string(dir.join("comments.jsonl")).unwrap();
    serde_json::from_str(log.lines().last().unwrap()).unwrap()
}

#[test]
fn agent_comments_lists_what_awaits_a_reply() {
    let home = tempfile::tempdir().unwrap();
    let dir = write_session(home.path(), "abc123", &example());
    write_comments(&dir, &[USER_COMMENT]);
    let threads = agent(home.path(), &["comments", "abc123"], "");
    assert_eq!(threads.as_array().unwrap().len(), 1, "{threads}");
    assert_eq!(threads[0]["id"], "c1");
    assert_eq!(threads[0]["awaiting_reply"], true);

    write_comments(&dir, &[USER_COMMENT, AGENT_REPLY]);
    let threads = agent(home.path(), &["comments", "abc123"], "");
    assert_eq!(threads, serde_json::json!([]));
    let all = agent(home.path(), &["comments", "abc123", "--all"], "");
    assert_eq!(all[0]["value"], "Bump serde to 1.0.229");
    assert_eq!(all[0]["messages"].as_array().unwrap().len(), 2);
}

#[test]
fn agent_reply_answers_a_thread() {
    let home = tempfile::tempdir().unwrap();
    let dir = write_session(home.path(), "abc123", &example());
    write_comments(&dir, &[USER_COMMENT]);
    let message = agent(home.path(), &["reply", "abc123", "c1", "on it"], "");
    assert_eq!(message["author"], "agent");
    assert_eq!(message["text"], "on it");
    let id = message["id"].as_str().unwrap();
    assert!(id.starts_with('c') && id.len() == 6, "{message}");
    let logged = last_log_line(&dir);
    assert_eq!(
        (logged["id"].as_str(), logged["reply_to"].as_str()),
        (Some(id), Some("c1"))
    );
    assert_eq!(
        agent(home.path(), &["comments", "abc123"], ""),
        serde_json::json!([])
    );

    // Text that starts with `-` is still text.
    let message = agent(home.path(), &["reply", "abc123", "c1", "-1 is wrong"], "");
    assert_eq!(message["text"], "-1 is wrong");
}

#[test]
fn agent_reply_refuses_what_isnt_a_thread() {
    let home = tempfile::tempdir().unwrap();
    let dir = write_session(home.path(), "abc123", &example());
    write_comments(&dir, &[USER_COMMENT, AGENT_REPLY]);
    let log = fs::read_to_string(dir.join("comments.jsonl")).unwrap();
    let reply = |thread: &str, text: &str| {
        agent_output(home.path(), &["reply", "abc123", thread, text], "")
    };
    assert_fails_with(&reply("c999", "hi"), "no thread \"c999\"");
    // `c2` is a reply, not a thread.
    assert_fails_with(&reply("c2", "hi"), "no thread \"c2\"");
    assert_fails_with(&reply("c1", "  "), "the comment is empty");
    assert_eq!(fs::read_to_string(dir.join("comments.jsonl")).unwrap(), log);
}

#[test]
fn agent_comment_starts_a_thread() {
    let home = tempfile::tempdir().unwrap();
    write_session(home.path(), "abc123", &example());
    let message = agent(
        home.path(),
        &[
            "comment", "abc123", "--row", "403", "--col", "author", "look",
        ],
        "",
    );
    assert_eq!(
        (message["author"].as_str(), message["text"].as_str()),
        (Some("agent"), Some("look"))
    );
    let all = agent(home.path(), &["comments", "abc123", "--all"], "");
    assert_eq!(all[0]["id"], message["id"]);
    assert_eq!(
        (all[0]["row_id"].as_str(), all[0]["column"].as_str()),
        (Some("403"), Some("author"))
    );
    assert_eq!(all[0]["value"], "fatima");
    assert_eq!(all[0]["awaiting_reply"], false);
    // The agent wrote last, so nothing awaits a reply.
    assert_eq!(
        agent(home.path(), &["comments", "abc123"], ""),
        serde_json::json!([])
    );
}

#[test]
fn agent_comment_on_a_missing_cell_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let dir = write_session(home.path(), "abc123", &example());
    let comment = |row: &str, col: &str| {
        agent_output(
            home.path(),
            &["comment", "abc123", "--row", row, "--col", col, "x"],
            "",
        )
    };
    assert_fails_with(&comment("412", "title"), "no row \"412\"");
    assert_fails_with(&comment("404", "nope"), "no column \"nope\"");
    assert!(!dir.join("comments.jsonl").exists());
}

#[test]
fn agent_comment_on_an_unkeyed_table() {
    let home = tempfile::tempdir().unwrap();
    write_session(home.path(), "abc123", r#"[{"a": 1}, {"a": 2}]"#);
    agent(
        home.path(),
        &["comment", "abc123", "--row", "#1", "--col", "a", "two?"],
        "",
    );
    let all = agent(home.path(), &["comments", "abc123", "--all"], "");
    assert_eq!(
        (all[0]["row_id"].as_str(), all[0]["value"].as_u64()),
        (Some("#1"), Some(2))
    );
}

#[test]
fn agent_comments_warns_about_skipped_lines_on_stderr() {
    let home = tempfile::tempdir().unwrap();
    let dir = write_session(home.path(), "abc123", &example());
    write_comments(&dir, &[USER_COMMENT, "not json"]);
    let output = agent_output(home.path(), &["comments", "abc123"], "");
    let threads: Value = serde_json::from_str(&succeeds(&output)).unwrap();
    assert_eq!(threads[0]["id"], "c1");
    assert_eq!(
        stderr(&output),
        "comments.jsonl: skipped lines that aren't events: 2\n"
    );
}

/// The `goto.json` waiting in session directory `dir`, if there is one.
fn pending_goto(dir: &Path) -> Option<Value> {
    let text = fs::read_to_string(dir.join("goto.json")).ok()?;
    Some(serde_json::from_str(&text).unwrap())
}

#[test]
fn agent_goto_asks_the_viewer_and_prints_the_focus() {
    let home = tempfile::tempdir().unwrap();
    let dir = write_session(home.path(), "abc123", &example());
    let focus = agent(
        home.path(),
        &["goto", "abc123", "--row", "404", "--col", "title"],
        "",
    );
    assert_eq!(focus["viewer_open"], false);
    assert_eq!(
        (focus["row_id"].as_str(), focus["column"].as_str()),
        (Some("404"), Some("title"))
    );
    assert_eq!(focus["value"], "Bump serde to 1.0.229");
    assert_eq!(focus["row"]["number"], 404);
    assert_eq!(
        pending_goto(&dir),
        Some(serde_json::json!({"row_id": "404", "column": "title"}))
    );
}

#[test]
fn agent_goto_without_col_keeps_the_viewers_column() {
    let home = tempfile::tempdir().unwrap();
    let dir = write_session(home.path(), "abc123", &example());
    // An open viewer on `author` keeps it.
    write_view_at(&dir, std::process::id(), "409", "author");
    let focus = agent(home.path(), &["goto", "abc123", "--row", "404"], "");
    assert_eq!(
        (focus["column"].as_str(), focus["value"].as_str()),
        (Some("author"), Some("dependabot"))
    );
    assert_eq!(
        pending_goto(&dir),
        Some(serde_json::json!({"row_id": "404"}))
    );

    // A closed one: the next viewer starts on the key column, whatever
    // `view.json` last said.
    write_view_at(&dir, NO_SUCH_PID, "409", "author");
    let focus = agent(home.path(), &["goto", "abc123", "--row", "404"], "");
    assert_eq!(focus["viewer_open"], false);
    assert_eq!(
        (focus["column"].as_str(), focus["value"].as_u64()),
        (Some("number"), Some(404))
    );
}

#[test]
fn agent_goto_to_a_missing_cell_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let dir = write_session(home.path(), "abc123", &example());
    let output = agent_output(home.path(), &["goto", "abc123", "--row", "412"], "");
    assert_fails_with(&output, "no row \"412\"");
    let output = agent_output(
        home.path(),
        &["goto", "abc123", "--row", "404", "--col", "nope"],
        "",
    );
    assert_fails_with(&output, "no column \"nope\"");
    assert_eq!(pending_goto(&dir), None);
}

#[test]
fn rm_removes_a_session_with_a_pending_goto() {
    let home = tempfile::tempdir().unwrap();
    let dir = write_session(home.path(), "abc123", &example());
    agent(home.path(), &["goto", "abc123", "--row", "404"], "");
    assert!(pending_goto(&dir).is_some());
    succeeds(&tv(home.path(), &["rm", "abc123"], ""));
    assert!(!dir.exists());
}
