# Plan: ADI-220 + ADI-221 `view.json` and live reload

Linear: https://linear.app/aditya-bhargava/issue/ADI-201 (parent)
Sub-issues: ADI-220 (`view.json`: the viewer writes cursor and pid; `ls` viewer-open column, `rm --force`, `prune --closed`), ADI-221 (live reload of `table.json`).
Depends on: ADI-201's first PR merged (#6, `af8ddef`).

## Goal

- The viewer writes `view.json` into its session directory on start, on exit, and when the cursor moves (at most one write per 100ms). Fields: `pid`, `row_id`, `column`, `updated_at`. The exit write leaves out `pid`.
- A session's viewer is "open" if `view.json` names a live pid. `tv ls` shows that in a VIEWER column. `tv rm` refuses an open session unless `--force`. `tv session prune --closed` prunes only closed sessions, and `prune` never removes an open one.
- The viewer notices when `table.json` changes (every 250ms, on the tick it already has), reloads, keeps the cursor on the same row ID and column name, and highlights changed cells and new rows for 5 seconds. A `table.json` that doesn't parse leaves the last good table on screen with the error in the status line until a later reload succeeds.

## Context for the executing agent

- Read `SPEC.md`: "Sessions / `view.json`", "Live updates", "Viewer / Rendering" (changed-cell highlight), and "Changes after review" (`goto.json`, ADI-228, which will ride the same poll later).
- Build on what's there; don't duplicate it:
  - `session.rs`: `Sessions`, `Session { id, dir }`, `Session::load_table`, `write_table`, `modified_at`, `summary`, `SESSION_FILES` (already lists `view.json`), `Summary`.
  - `files::write_atomic` (temp + rename). `Table::save` wraps it for JSON; Part 1 lifts that wrapper into `files::write_json` so `view.json` uses the same one. `time::rfc3339` for `updated_at`.
  - `viewer/mod.rs`: `run(table, session_id)` and `event_loop`, with `TICK = 250ms` and a comment that already reserves the tick for live reload. `App::tick(now)` expires the status message; it takes `now` as an argument so tests don't sleep. Follow that pattern for every new timer.
  - `viewer/app.rs`: `App` keeps `table` and `widths` private, with a comment saying live reload must add a `set_table` that recomputes `widths`. Do exactly that. `Cursor { row, col }` are indices. `App::set_message` is the status-line message (3 seconds). `open_cell_view` refuses Detail and Record modes on a table with no rows or no columns; `draw_detail` relies on that (it indexes `table.columns[cursor.col]`), so a reload must keep it true too.
  - `viewer/render.rs`: `cell_style` is "the one place that decides how a body cell looks", and its comment says change highlights get added there. `status_line` builds the status text. `snapshot::buffer_view` records style runs, so a highlight shows up in a snapshot diff.
  - `viewer/theme.rs`: `Theme { header, cursor_cell, cursor_line, null }` with `color()`, `mono()`, `detect()`.
  - `main.rs`: `ls`, `rm`, `prune`, `PruneFilter { older_than }` with `matches(modified_at, now)` and `is_empty`, `format_table`. Plan 05 left the hooks for `--closed` and the viewer-open column in comments; they're named below.
  - `tests/cli.rs`: `tv(home, args, stdin)`, `write_session(home, id, table)`, `succeeds`, `assert_fails_with`.
- **No new crates.** The pid check needs `kill(pid, 0)`, which std doesn't expose. `rustix` is already a dependency (for `dup2_stdin`); add its `process` feature. I checked with this lockfile: `cargo tree --locked` still passes and the crate count is unchanged (89 on the host, 103 with `--target all`), because the feature only turns on a feature of `linux-raw-sys`, which is already in the tree. `Cargo.lock` doesn't change. crossterm doesn't turn `process` on (it uses `std`, `stdio`, `termios`, `fs`, `alloc`), so the `Cargo.toml` comment needs to say why this one adds no crates.
- **Two commits, one per sub-issue, each passing `make check`.** `make lint` runs clippy with `-D warnings` and `dead_code` fires in a binary crate, so everything a commit adds needs a caller in that commit. The split below holds that.
- **Nothing here needs a terminal to test.** The viewer's file IO is in one module (`viewer/sync.rs`) whose `SessionSync` owns the `Session` and takes `&mut App`, so unit tests drive it with a `TempDir` session and an `Instant` they control. Render changes are snapshot tests with `TestBackend`, as before.
- **Subtracting `Duration`s panics if the result would be negative** (`Instant - Instant` saturates at zero, `Duration - Duration` doesn't). Use `saturating_sub` for every "time left" calculation, like plan 05 did for `SystemTime`.

## What this plan defers, and to where

| Item | Needs | Lands in |
|---|---|---|
| Polling `comments.jsonl` (ADI-221 names it) | the comment log | ADI-223 |
| Applying `goto.json` on the poll | `tv agent goto` | ADI-228 |
| `tv agent sessions`, `tv agent focus` reading `view.json` | the agent CLI | ADI-202 (ADI-225..) |

The poll loop in `viewer/sync.rs` is where those plug in: one more check per tick, next to the `table.json` one.

## Part 1: `view.json` (ADI-220)

### Layout on disk

```
$TV_HOME/sessions/<id>/
  table.json
  view.json      # new: written by the viewer, atomically
```

```json
{
  "pid": 81234,
  "row_id": "412",
  "column": "title",
  "updated_at": "2026-10-28T19:02:11Z"
}
```

`row_id` and `column` are omitted when the table has no rows or no columns; there is no cell to point at. `row_id` is `Table::row_id(cursor.row)`, so an unkeyed table writes `#3`. `pid` is `std::process::id()` while the viewer runs. The exit write leaves `pid` out and keeps the final position, so `tv agent focus` can still read where the cursor was:

```json
{
  "row_id": "412",
  "column": "title",
  "updated_at": "2026-10-28T19:40:02Z"
}
```

Why drop it: macOS reuses pids once it reaches 99,998, and a typical machine has about a thousand live processes, so the pid of a viewer that quit belongs to some live process about 1% of the time. If the pid stayed, `ls` would say `open`, `rm` would refuse, and `prune` would skip a session nobody is looking at. With the exit write leaving `pid` out, only a crashed viewer leaves a pid behind.

### `session.rs` additions

```rust
/// What the viewer tells the agent: where its cursor is and which process it
/// is. Written by the viewer, read by `tv ls` and the agent CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    /// The viewer's process while it runs; `None` after it quits normally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    pub updated_at: String, // RFC 3339, from `time::rfc3339`
}

/// `pub`, like `TABLE_FILE` becomes: the viewer names these files in its
/// status-line errors.
pub const VIEW_FILE: &str = "view.json";

impl Session {
    /// Write `view.json` atomically (temp file + rename, like `table.json`).
    pub fn write_view(&self, view: &View) -> anyhow::Result<()>;

    /// `view.json`, or `None` if the viewer hasn't written one. A file that
    /// doesn't parse is an error, like a broken `table.json`.
    pub fn read_view(&self) -> anyhow::Result<Option<View>>;

    /// Whether a viewer has this session open: `view.json` names a pid that is
    /// alive. Missing or unreadable `view.json`, or one without a pid, counts
    /// as closed; `ls` and `prune` must not fail because of it.
    pub fn viewer_open(&self) -> bool;
}

/// `kill(pid, 0)`: `Ok` means alive, `EPERM` means alive but someone else's,
/// `ESRCH` means gone. A pid that doesn't fit an `i32` is nobody.
fn pid_is_alive(pid: u32) -> bool;
```

- `write_view` and `Table::save` do the same thing: pretty-print JSON, add a trailing newline, `write_atomic`, add a `writing {path}` context. Don't write it twice. Add to `files.rs`:

  ```rust
  /// Write `value` as pretty JSON with a trailing newline, atomically.
  pub fn write_json(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
      let mut json = serde_json::to_string_pretty(value)?;
      json.push('\n');
      write_atomic(path, json.as_bytes()).with_context(|| format!("writing {}", path.display()))
  }
  ```

  `Table::save` becomes `files::write_json(path, self)` and `write_view` is `files::write_json(&self.dir.join(VIEW_FILE), view)`. The existing save tests cover the move.
- `read_view`: `fs::read_to_string` with `NotFound` mapped to `Ok(None)` and other errors given `reading {path}` context; parse errors get `parsing {path}`.
- `viewer_open`, in two steps so each says one thing:

  ```rust
  let pid = self.read_view().ok().flatten().and_then(|view| view.pid);
  pid.is_some_and(pid_is_alive)
  ```

  The `.ok()` is deliberate and the doc comment says why: only the viewer writes `view.json`, atomically, so an unreadable one is a hand edit, and `ls` and `prune` must keep working through it.
- `SESSION_FILES` uses `VIEW_FILE` in place of the `"view.json"` literal, and `TABLE_FILE` becomes `pub` too.
- `pid_is_alive`, Unix. The two `true` cases share one arm: pedantic clippy rejects separate arms with the same body (`match_same_arms`, checked).

  ```rust
  #[cfg(unix)]
  fn pid_is_alive(pid: u32) -> bool {
      use rustix::io::Errno;
      use rustix::process::{Pid, test_kill_process};
      let Some(pid) = i32::try_from(pid).ok().and_then(Pid::from_raw) else {
          return false;
      };
      match test_kill_process(pid) {
          Ok(()) | Err(Errno::PERM) => true, // EPERM: alive, owned by another user
          Err(_) => false,                   // ESRCH: no such process
      }
  }

  #[cfg(not(unix))]
  fn pid_is_alive(_pid: u32) -> bool {
      false // no viewer-open detection off Unix; everything counts as closed
  }
  ```

  `Pid::from_raw` returns `None` for 0, so pid 0 (which `kill` would treat as "my process group") can never be asked about. Don't use `as i32`: pedantic clippy rejects the cast and `try_from` is the honest version anyway.

- `Summary` doesn't change. `ls` calls `session.viewer_open()` for every row, because the placeholder row for a broken `table.json` has no `Summary` to read it from, and one way of asking is simpler than two. (ADI-220's description says `Summary` gains a field; this is the simpler version of the same thing.)
- `Cargo.toml`: `rustix = { …, features = ["std", "stdio", "process"] }`. Rewrite the comment: `stdio` is for `dup2_stdin` in `main.rs` and `process` for the pid check in `session.rs`; crossterm already turns on `std` and `stdio`, and `process` only turns on a feature of `linux-raw-sys`, which is already in the tree, so neither adds a crate.

Stale pids are not handled beyond dropping `pid` on a normal exit. A crashed viewer's pid can still be reused, and if two viewers have the same session open, the last write wins: the first to quit makes the session look closed until the other moves its cursor. The spec declines both.

**Using the viewer now counts as activity.** `modified_at` is the newest mtime of any file in the session directory (plan 05 meant it to cover `view.json`), and the viewer writes `view.json` when the cursor moves. So MODIFIED in `tv ls` changes as you move around, a session you keep looking at isn't caught by `prune --older-than`, and bare `tv` reopens the session you looked at last. That's intended; nothing in `session.rs` changes for it.

### The viewer writes it

A new module `src/viewer/sync.rs`: the viewer's side of the session directory. It owns the `Session` and all file IO the event loop does, so `mod.rs` stays an event loop and `app.rs` stays pure state. In Part 1 it writes; in Part 2 it also reads.

```rust
//! The viewer's half of the session directory: `view.json` out (where the
//! cursor is), and in Part 2 `table.json` in (live reload). Everything that
//! touches the filesystem from the event loop goes through here, with the
//! time passed in so tests control it.

/// At most one `view.json` write per this much cursor movement.
const VIEW_DEBOUNCE: Duration = Duration::from_millis(100);

/// Not `Sync`: that's the standard library's thread-safety trait, in scope
/// everywhere, and a struct with its name would hide it.
pub struct SessionSync {
    session: Session,
    /// The position last written (or last tried), so a tick with no movement
    /// writes nothing. `start` writes, so there always is one.
    written: Position,
    /// When the cursor first moved away from `written`, if it has.
    moved_at: Option<Instant>,
}

/// What `view.json` records about the cursor: the row's ID and the column's
/// name, which survive a reload where indices don't.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Position {
    row_id: Option<String>,
    column: Option<String>,
}

impl SessionSync {
    /// Write `view.json` once, so the agent can see the viewer the moment it
    /// opens.
    pub fn start(session: Session, app: &mut App) -> SessionSync;

    /// Called once per event-loop iteration, before drawing. Writes
    /// `view.json` if the cursor moved and the debounce has passed.
    pub fn tick(&mut self, app: &mut App, now: Instant);

    /// How long the event loop may wait for a key before `tick` has something
    /// to do: `None` means "no deadline, use the normal tick".
    pub fn next_due(&self, now: Instant) -> Option<Duration>;

    /// The final write on exit, without `pid`. A directory that's gone
    /// (`tv rm --force` under the viewer) is fine: there's nothing left to
    /// record. Any other error is returned.
    pub fn finish(&self, app: &App) -> anyhow::Result<()>;
}

impl Position {
    fn of(app: &App) -> Position;

    /// `view.json`'s contents for this position, stamped now. `start`,
    /// `tick` and `finish` all build their `View` here.
    fn view(&self, pid: Option<u32>) -> View;
}

/// A status-line error about one of the session's files: `table.json: {the
/// underlying error}`, without the context chain (see "Rendering" in Part 2).
fn file_error(file: &str, error: &anyhow::Error) -> String {
    format!("{file}: {}", error.root_cause())
}
```

`start` and `tick` share one private `write(&mut self, app: &mut App, position: Position)`: build the `View` with `Some(std::process::id())`, write it, put an error in the status line, set `written`. `finish` builds `position.view(None)` and writes it.

`finish` decides "directory gone" from the error itself: `error.downcast_ref::<io::Error>()` reaches through anyhow's context to the `io::Error`, and a `NotFound` there means `write_atomic` couldn't create its temp file because the directory is missing. It doesn't swallow other errors: a viewer that quit but couldn't say so has left a live-looking pid behind, which is worth an `Error:` line.

- `App` gets `row_id()` and `column_name()`, both `Option<String>`: `Table::row_id(cursor.row)` and `columns[cursor.col].name`, or `None` when the table has no rows or no columns. Add `fn has_cell(&self) -> bool` (at least one row and one column) for that check, and make `open_cell_view` use it too, so the check that allows Detail and Record modes is written once. `sync.rs` builds a `Position` from `row_id()` and `column_name()`.
- `tick` logic, in words: compute the current position. If it equals `written`, clear `moved_at` and return. Otherwise set `moved_at` to `now` if unset. If `now.duration_since(moved_at) >= VIEW_DEBOUNCE`, write, set `written`, clear `moved_at`. So a held-down `j` writes about ten times a second, and a single tap writes 100ms later. Each write fsyncs (`write_atomic` calls `sync_all`, which is `F_FULLFSYNC` on macOS); I timed write + sync + rename at 4.1ms median, so ten a second costs about 40ms of each second. Don't add a no-fsync variant.
- `next_due` returns `VIEW_DEBOUNCE.saturating_sub(now.duration_since(moved_at))` when a write is pending, else `None`. Not `VIEW_DEBOUNCE - (now - moved_at)`: the event loop always ticks first with the same `now`, but tests call `next_due` at times they choose, and a `Duration` subtraction that goes negative panics. The event loop polls for `min(TICK, due)`, so a single tap lands in `view.json` about 100ms after the key, not up to 250ms later.
- Write errors: `app.set_message(file_error(VIEW_FILE, &error))`, which reads `view.json: No such file or directory (os error 2)`. The full context chain would start with the session's path and push the reason off a narrow status line (see "Rendering" in Part 2). Make `set_message` `pub(super)`. The viewer keeps running; the cursor position is advisory. Don't retry on the next tick unless the cursor moves again (`written` is still updated), or a full disk produces a message every 100ms.
- `Position::view` builds `View { pid, row_id, column, updated_at: time::rfc3339(SystemTime::now()) }`.

`viewer/mod.rs` changes:

```rust
pub fn run(table: Table, session: Session) -> anyhow::Result<()> {
    let mut app = App::new(table);
    app.session_id = Some(session.id().to_string());
    app.theme = Theme::detect();
    let mut sync = SessionSync::start(session, &mut app);
    let result = ratatui::run(|terminal| event_loop(terminal, &mut app, &mut sync));
    // Also after an error, so view.json says the viewer is gone. The loop's
    // error matters more, so it's the one returned if both fail.
    let finished = sync.finish(&app);
    result.and(finished)
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    sync: &mut SessionSync,
) -> anyhow::Result<()> {
    while !app.should_quit {
        let now = Instant::now();
        let size = terminal.size()?;
        fit_viewport(app, Rect::new(0, 0, size.width, size.height));
        app.tick(now);
        sync.tick(app, now);
        terminal.draw(|frame| render::draw(frame, app))?;
        let timeout = sync.next_due(now).map_or(TICK, |due| due.min(TICK));
        if event::poll(timeout)?
            && let Event::Key(key) = event::read()?
        {
            app.handle_key(key);
        }
    }
    Ok(())
}
```

`finish` runs in `run`, after `ratatui::run` returns, so an `event_loop` error still gets the exit write, and an error from `finish` prints after the terminal is restored. A panic doesn't get the exit write; that's the crashed-viewer case above.

`main.rs`'s `open` passes the `Session` instead of its ID: `viewer::run(table, session)`. `Session` isn't `Clone` and doesn't need to be; `run` takes it by value.

Also rename the existing `sync_viewport` in `mod.rs` to `fit_viewport` so it doesn't read like part of `SessionSync`.

### `ls`, `rm --force`, `prune --closed`

**`ls`:** a VIEWER column between ROWS×COLS and MODIFIED, `open` when `session.viewer_open()`, `-` otherwise. Call it once per row, before matching on `summary()`, so the placeholder row for a broken `table.json` fills it the same way; `viewer_open` doesn't need the table.

```
ID      ROWS×COLS  VIEWER  MODIFIED              TITLE
k3f9x2  40×9       open    2026-10-28T19:02:11Z  Open pull requests
m8q1zz  3×2        -       2026-10-27T08:15:40Z  (untitled)
```

**`rm`:** gains `#[arg(long)] force: bool`. After opening every ID and before deleting any, check `session.viewer_open()` for each; the first open one fails the command with `session {id} has a viewer open; close it or use --force`. With `--force`, delete anyway. The viewer then finds its `table.json` gone on the next poll (Part 2 shows the error in the status line and keeps running), and its exit write finds the directory gone, which `finish` treats as nothing left to record. `Sessions::remove` doesn't change; the rule lives in the command, like the open-all-first rule does.

`rm --force` can refuse once while the viewer is in use: if the viewer rewrites `view.json` in the middle of the delete, `remove`'s second look at the directory finds the new file and refuses with `it changed while being deleted; try again`. That's the safeguard working, and running `tv rm --force` again succeeds. Don't change `remove` to avoid it.

**`prune`:**

```rust
struct PruneFilter {
    older_than: Option<Duration>,
    closed: bool,
}

impl PruneFilter {
    fn is_empty(&self) -> bool; // no --older-than and no --closed

    /// True when a session last modified at `modified_at`, whose viewer is
    /// open or not, matches every filter that is set.
    fn matches(&self, modified_at: SystemTime, viewer_open: bool, now: SystemTime) -> bool {
        let age = now.duration_since(modified_at).unwrap_or_default();
        let old_enough = self.older_than.is_none_or(|limit| age >= limit);
        let closed_enough = !self.closed || !viewer_open;
        old_enough && closed_enough
    }
}
```

- `SessionCommand::Prune` gains `/// Only sessions whose viewer is closed.` `#[arg(long)] closed: bool`. The no-filter error becomes `give a filter: --older-than <duration> and/or --closed`.
- `prune` never removes a session with an open viewer, whatever the filters. If one matches (`--older-than 30d` on a session someone has open), print `skipped {id}  (viewer open)` and move on. This is the same guard as `rm` without `--force`; `prune` has no `--force` because a bulk delete shouldn't have one. Both `rm` and `prune` call the same `session.viewer_open()`; the difference is error versus skip, and that's a one-line decision in each command, not a shared helper.
- The loop calls `session.viewer_open()` once per session and passes the result to both `matches` and the skip check.
- The counter (rename `matched` to `pruned`) counts sessions removed, or that would be removed with `--dry-run`. Skipped sessions don't count, so `nothing to prune` prints whenever it's 0, also after `skipped …` lines:

  ```
  $ tv session prune --older-than 0d --dry-run
  skipped k3f9x2  (viewer open)
  nothing to prune
  ```

### Tests

- **`session.rs`:**
  - `write_view` then `read_view` round-trips, including `row_id: None` (the field is absent from the file: check the JSON text doesn't contain `"row_id"`).
  - `read_view` on a fresh session is `Ok(None)`; on `view.json` containing `not json` it's `Err`, and `viewer_open()` is then `false`.
  - `viewer_open` is `true` with `pid: Some(std::process::id())` (the test process) and `false` with `pid: None`, with `pid: Some(u32::MAX)` (doesn't fit an `i32`) and with `NO_SUCH_PID`, a test constant `2_147_483_647` with the comment: fits an `i32`, but no system hands it out (Linux caps pids at 4,194,304 and macOS at 99,998), so `kill` says `ESRCH`. `tests/cli.rs` gets the same constant. These are `#[cfg(unix)]`, like the existing permission test.
  - `pid_is_alive(0)` is `false` (from `Pid::from_raw`), without calling `kill`.
  - `remove` still deletes a session that has a `view.json` (it's in `SESSION_FILES`; one assertion in the existing test).
- **`viewer/sync.rs`**, each with a `Sessions::new(tempdir)` session and a hand-made `App`:
  - `start` writes `view.json` with the test's pid and the cursor's row ID and column name.
  - Moving the cursor and ticking at `now` writes nothing; ticking at `now + 100ms` writes the new position; ticking again with no movement writes nothing.
  - "Writes nothing" is checked by mtime, not by the file's bytes: `updated_at` has whole-second precision, so rewriting the same position within one second gives identical bytes and a bytes check could never fail. A helper sets `view.json`'s mtime into the past (`File::options().write(true).open(&path)?.set_modified(past)`) and another checks it's unchanged. A real write goes through `write_atomic`, which renames a new file into place, so it gets a fresh mtime.
  - Two moves 30ms apart, then a tick at 100ms after the first: `view.json` has the second position.
  - `next_due` is `None` at rest, `Some(100ms)` after a move and a tick at the same `now`, `Some(40ms)` 60ms later, `Some(0)` (not a panic) 150ms later without a tick, and `None` again once written.
  - An empty table writes `view.json` with no `row_id` or `column`.
  - `finish` writes the final position with no `pid`, and `viewer_open()` is then `false`.
  - `finish` on a session whose directory is gone returns `Ok`. To make it gone, use the one delete path the crate has: `sessions.remove(sessions.open(id)?)` on a second `Session` handle for the same directory, which is exactly what `tv rm --force` does under an open viewer. No test-only constructor and no new delete call.
- **`viewer/app.rs`:** `row_id()` and `column_name()` are `None` on an empty table and `Some` otherwise; `open_cell_view` still refuses with `no rows` and `no columns` (the existing tests cover it).
- **`main.rs`:** `PruneFilter::matches` with `closed: true` rejects an open session and accepts a closed one; `closed: false` ignores the flag; `is_empty` is `false` with only `closed`. `tv rm --force a` and `tv session prune --closed` parse.
- **`tests/cli.rs`:**
  - `write_view(dir, pid)` helper writing `view.json` by hand into the directory `write_session` returns, with every field so it parses.
  - `tv ls` shows `open` for a session whose `view.json` has the test's own pid, and `-` for one with pid `2147483647` and for one with no `view.json`.
  - `tv rm <open>` exits 1 with `has a viewer open`, and the directory remains. `tv rm --force <open>` removes it.
  - `tv session prune --closed --dry-run` lists the closed session and not the open one. `tv session prune --older-than 0d` removes the closed session and prints `skipped <open>  (viewer open)`, and the open one remains.
  - `tv session prune --older-than 0d` with only an open session prints the `skipped` line and `nothing to prune`.
  - `tv session prune --closed` with no closed sessions prints `nothing to prune`.

## Part 2: live reload (ADI-221)

### What changes and when

Every event-loop iteration (so at least every 250ms) `SessionSync::tick` stats `table.json`. If its fingerprint differs from the one recorded at the last load, it reloads.

```rust
/// Enough to notice a rewrite: `write_atomic` renames a new file into place,
/// so the mtime changes, and the size catches two writes within one clock
/// tick on a filesystem with coarse timestamps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint {
    modified: SystemTime,
    len: u64,
}

impl Session {
    /// One `metadata` call, with a `reading {path}` context like `load_table`.
    pub fn table_fingerprint(&self) -> anyhow::Result<Fingerprint>;
}
```

`Fingerprint` is `pub` (with private fields) because `sync.rs` stores one, and a `pub fn` returning a private type trips the `private_interfaces` warning, which `make lint` turns into an error.

`SessionSync` gains `seen: Option<Fingerprint>`, set in `start` (the table was loaded moments before, by `main`), and one method that answers "is there a new table?":

```rust
impl SessionSync {
    /// The table if `table.json` changed since the last look, `None` if not.
    /// A file that fails to parse is still recorded as seen, so it's parsed
    /// once, not every 250ms.
    fn changed_table(&mut self) -> anyhow::Result<Option<Table>> {
        // Fingerprint first, then read. A write landing between the two
        // costs one extra reload next tick; the other order could record the
        // new fingerprint with the old contents and miss the change.
        let fingerprint = self.session.table_fingerprint()?;
        if Some(fingerprint) == self.seen {
            return Ok(None);
        }
        self.seen = Some(fingerprint);
        self.session.load_table().map(Some)
    }
}
```

In `tick`, before the `view.json` logic:

```rust
match self.changed_table() {
    Ok(None) => {}
    Ok(Some(table)) => app.set_table(table, now),
    Err(error) => app.reload_error = Some(file_error(TABLE_FILE, &error)),
}
```

That is the only line that sets `reload_error`. Whatever failed, stat, read or parse, the message comes from `file_error`, the same helper the `view.json` errors use: `table.json: ` and the underlying error without its context chain: `table.json: No such file or directory (os error 2)`, `table.json: expected value at line 1 column 1`. The chain would start with the session's full path (about 50 characters, after the 37 of ` row 2/38  col 4/9  ·  reload failed: `), which on an 80- or 100-column terminal pushes the line number, the one part you need, off the screen. The viewer only reloads one file, so the path says nothing new. `tv ls` and `tv save` still print the full chain.

- The error stays on screen (below) until a reload succeeds; `set_table` clears it.
- `table.json` missing (`tv rm --force` while open) is a stat error like any other: the last table stays, the viewer keeps running. Nothing in the poll exits the viewer. Readers never see `table.json` missing during a normal write, because `rename` replaces it atomically.

### `App::set_table`

```rust
impl App {
    /// Replace the table after a reload. Recomputes `widths` (the reason
    /// `table` is private), keeps the cursor on the same row ID and column
    /// name, clamps if either is gone, and highlights what changed. The
    /// highlight replaces the previous reload's: its indices were into the
    /// old table and may point at different cells now.
    pub fn set_table(&mut self, table: Table, now: Instant) {
        let cursor = diff::same_cell(&self.table, self.cursor, &table);
        let changed = diff::changed_cells(&self.table, &table);
        self.widths = layout::column_widths(&table);
        self.table = table;
        self.highlight = Some(Highlight { cells: changed, until: now + HIGHLIGHT_DURATION });
        self.reload_error = None;
        self.move_to(cursor.row, cursor.col); // clamps and scrolls into view
        // Detail and Record need a cell under the cursor (`draw_detail`
        // indexes the cursor's column), the same check `open_cell_view` makes.
        if matches!(self.mode, Mode::Detail | Mode::Record) && !self.has_cell() {
            self.mode = Mode::Grid;
        }
    }
}
```

New fields on `App`: `pub reload_error: Option<String>` and `highlight: Option<Highlight>` with `pub fn is_highlighted(&self, row, col) -> bool` for render. `const HIGHLIGHT_DURATION: Duration = Duration::from_secs(5);`. `tick(now)` clears `highlight` once `now >= until`, next to the message expiry it already does.

`move_to` clamps the cursor, and `scroll_into_view` (which `move_to` calls) clamps `first_row` for a table that got shorter. It doesn't clamp `first_col` when the cursor is on the pinned key column: it returns early (src/viewer/app.rs:671–673). Today that can't matter, because the column count never changes. After a reload it does:

1. Scroll right, so `first_col` is 6.
2. Go back to the key column with `0`.
3. The agent pushes a table with 3 columns.

`visible_columns` keeps only columns with `index >= first_col`, so the grid would show the key column and nothing else until the user pressed `l`. Fix it where the scroll state lives, not in `set_table`: in `scroll_into_view`, before the pinned-column early return, clamp `first_col`:

```rust
// A reload can leave fewer columns than `first_col`.
self.first_col = self.first_col.min(self.table.columns.len().saturating_sub(1));
```

Then `set_table` doesn't need to know about it, and any later change to the table gets it too.

Detail and Record modes need a cell under the cursor. A reload to a table with no columns (`{"rows": []}` or `[{}]`) while the cell popup is open would otherwise panic in `draw_detail`, and one with no rows would leave the record view showing a row that doesn't exist. Hence the mode check, which goes back to the grid. Command and Help modes need nothing: they read the cursor and table through the same fields on the next draw.

### `viewer/diff.rs`

Pure functions over two tables, tested without a terminal. This is the unit test ADI-221 asks for.

```rust
/// Where `cursor` in `old` is in `new`: the row with the same row ID and the
/// column with the same name. A row or column that's gone keeps its index,
/// which the caller clamps. Unkeyed tables have `#index` row IDs, so their
/// cursor stays by position. A cursor that isn't on a cell of `old` (an
/// empty table's `(0, 0)`) is returned as it is.
pub fn same_cell(old: &Table, cursor: Cursor, new: &Table) -> Cursor;

/// The cells to highlight after a reload: every cell of a row whose ID isn't
/// in `old` (a new row), and every cell whose value differs from the cell with
/// the same row ID and column name in `old`. A column `old` doesn't have
/// counts as changed in every row. Indices are into `new`. Without a key, rows
/// match by index, so a row inserted at the top marks every row below it as
/// changed; that's expected, not a bug.
pub fn changed_cells(old: &Table, new: &Table) -> HashSet<Cursor>;
```

Implementation notes:

- `same_cell` must check that the cursor is on a cell of `old` before reading its ID or column name: `cursor.row < old.rows.len()` and `old.columns.get(cursor.col)` is `Some`, otherwise return `cursor`. A session's first reload is often from an empty table (an agent creates the session, then pushes rows), whose cursor is `(0, 0)` with no row 0 and no column 0. `old.columns[cursor.col]` would panic there, and on a keyed table `old.row_id(0)` returns `""` (`cell_text(None)`), which could match a new row whose key is the empty string.

- Build `HashMap<String, usize>` from `old`'s row IDs once (`(0..old.rows.len()).map(|index| (old.row_id(index), index))`), not a linear search per row. Row IDs are unique when a key exists (`validate_key`) and `#<index>` otherwise.
- Compare `old.cell(old_row, name)` with `new.cell(new_row, name)` as `Option<&Value>`. Missing and `null` compare unequal here, which is fine: a cell going from absent to `null` is a change nobody will notice either way.
- `Cursor` needs `Hash` for the set: add it to its derive list.
- Deleted rows and deleted columns highlight nothing; there's no cell left to color.

### Rendering

- `Theme` gains `changed: Style`. Color: `Style::new().bg(Color::Indexed(58))` (a dark olive that reads as "recently changed" without fighting the cursor row's gray) `.fg(Color::Indexed(230))` (a pale yellow). Say what the two palette numbers are in a comment, like `cursor_line`'s. Mono: `Modifier::UNDERLINED`.
- `cell_style`: name the cursor-cell test once (`let on_cursor = (row, col) == (cursor.row, cursor.col);`) and use it in both places. After the cursor tiers: `if app.is_highlighted(row, col) && !on_cursor { style = style.patch(theme.changed) }`. The cursor cell stays reversed: it's already the brightest thing on screen, and reversing a background color would make it look like a third kind of highlight. The cursor row's cells do take the highlight, patched over `cursor_line`.
- `status_line`: when `app.reload_error` is `Some` and no transient message is showing, `reload failed: {error}` replaces the hints: ` row 2/38  col 4/9  ·  reload failed: table.json: expected value at line 3 column 1`. A transient message (a save result, say) still wins while it's up; the reload error comes back when the message expires. Nothing else changes in the line.

### Tests

- **`viewer/diff.rs`:**
  - `same_cell`: keyed table with rows reordered, cursor follows the ID; cursor row deleted, index unchanged (caller clamps); columns reordered, cursor follows the name; column deleted, index unchanged; unkeyed table, cursor stays by index; from an empty table, the cursor comes back unchanged (no panic).
  - `changed_cells`: one cell changed → that one cell; a new row → every cell of it; a deleted row → nothing; a renamed column → every row in that column; identical tables → empty; from an empty table → every cell; a value changing from a number to a string of the same digits (`1` vs `"1"`) → changed (it's a different `Value`).
- **`viewer/app.rs`:**
  - `set_table` recomputes widths: a reload that makes a cell much wider changes `visible_columns()` (or compare `widths` directly through a `#[cfg(test)]` accessor).
  - `set_table` with the cursor on the last row and a shorter table clamps the cursor and `first_row` (no panic, cursor on the new last row).
  - `set_table` from an empty table to `grid_app`'s table: no panic, every cell highlighted.
  - `set_table` with the cursor on the pinned key column and `first_col` scrolled past the new table's last column: `first_col` is clamped (by `scroll_into_view`) and `visible_columns().len() > 1`.
  - `set_table` in Detail mode, and in Record mode, with an empty table: the mode is `Grid` afterwards. In Detail mode with a table that still has cells: the mode stays `Detail`.
  - `set_table` sets `highlight`; `tick(now + 5s)` clears it; `tick(now + 4s)` doesn't.
  - `set_table` clears `reload_error`.
- **`viewer/render.rs`** snapshots (buffer_view records styles, so these pin the highlight):
  - `reload_highlights`: `prs()` reloaded with one cell changed and one row added, cursor elsewhere; the changed cell and the new row carry the `changed` style and the cursor cell doesn't.
  - `reload_error_status`: `reload_error` set, the status line shows it in place of the hints.
  - A test that a Detail-mode app reloaded with an empty table renders without panicking.
- **`viewer/sync.rs`**, with a real `Session` in a `TempDir`:
  - `tick` right after `start` reloads nothing (same fingerprint).
  - After `session.write_table(&changed)`, `tick` replaces `app.table()` and sets a highlight.
  - After writing `not json` to `table.json` by hand, `tick` leaves `app.table()` unchanged and sets `reload_error` to `table.json: expected ident at line 1 column 2` (starts with `table.json: ` and has no path); after that, `seen` equals the file's current fingerprint, which is what stops a re-parse every tick. After `write_table` with a good table, `tick` clears `reload_error`.
  - After the session is removed through `Sessions::remove` (the second-handle trick above), `tick` sets `reload_error` containing `No such file` and the table stays. Tests never call `fs::remove_file` themselves: the clippy ban covers test code too, and the two exempted sites stay the only ones.
  - Fingerprint: two writes of different lengths within the same second are both noticed (write, tick, write, tick: two reloads). Don't test same-length writes within one second; that depends on the filesystem's clock.

## Manual check

In a scratch directory, with a throwaway home:

```sh
export TV_HOME=$(mktemp -d)
cp ~/tv/examples/prs.json .
tv prs.json          # note the ID in the title bar; leave it open
```

In a second terminal, same `TV_HOME`:

1. `tv ls` shows the session with `open` in VIEWER. `cat $TV_HOME/sessions/<id>/view.json` shows your pid and `row_id`/`column` for the top-left cell. Move the cursor in the viewer; `cat` again within a second shows the new position.
2. `tv rm <id>` refuses with `has a viewer open`.
3. Edit `table.json`: change one `title`, append a row, and save it atomically (`python3 - <<EOF` that reads, edits, writes to a temp file and `os.replace`s it; a plain editor save is fine too since the viewer only needs the mtime to change). Within a quarter second the viewer shows the new values, the changed cell and the new row are highlighted, and the highlight fades after 5 seconds. The cursor is still on the same row.
4. Write `not json` into `table.json`. The viewer keeps the table and the status line shows `reload failed: table.json: expected ident at line 1 column 2`, with the line number visible on an 80-column terminal. `tv ls` shows `(can't read table.json)` for it. Restore the file (`tv save <id>` can't, the table is broken; `cp prs.json $TV_HOME/sessions/<id>/table.json`). The error clears and the table reloads.
5. `tv session prune --older-than 0d --dry-run` prints `skipped <id>  (viewer open)` and then `nothing to prune`. `tv session prune --closed --dry-run` prints just `nothing to prune`.
6. `q` in the viewer. `cat view.json` shows the final position and no `pid`. `tv ls` shows `-`. `tv session prune --closed` removes it.
7. Reopen with `tv prs.json`, then in the other terminal `tv rm --force <id>` (run it again if it says `it changed while being deleted`). The viewer shows `reload failed: table.json: No such file or directory (os error 2)` in the status line and keeps working; `q` exits cleanly.
8. `unset TV_HOME`.

## Acceptance

```sh
make check
cargo tree --locked -e normal --prefix none | sort -u | wc -l   # 89, unchanged
```

Plus the manual check above.

## Deliverables

- `Cargo.toml`: `rustix` gains the `process` feature (comment rewritten). `Cargo.lock` unchanged.
- `src/files.rs`: `write_json`; `src/table.rs`: `Table::save` calls it.
- `src/session.rs`: `View` (with `pid: Option<u32>`), `pub VIEW_FILE` and `pub TABLE_FILE`, `Session::{write_view, read_view, viewer_open, table_fingerprint}`, `pub Fingerprint`, `pid_is_alive`. `Summary` unchanged.
- `src/viewer/sync.rs` (new): `SessionSync::{start, tick, next_due, finish, write, changed_table}`, `Position::{of, view}`, `file_error`, `VIEW_DEBOUNCE`.
- `src/viewer/diff.rs` (new): `same_cell`, `changed_cells`.
- `src/viewer/app.rs`: `set_table` (with the mode check), the `first_col` clamp in `scroll_into_view`, `has_cell` (also used by `open_cell_view`), `row_id`/`column_name`, `reload_error`, `Highlight`, `is_highlighted`, `HIGHLIGHT_DURATION`, `tick` clears highlights, `set_message` is `pub(super)`, `Cursor: Hash`.
- `src/viewer/render.rs`: `cell_style` highlight, `status_line` reload error, two snapshots.
- `src/viewer/theme.rs`: `changed`.
- `src/viewer/mod.rs`: `run(table, session)` with `finish` after `ratatui::run`, `event_loop` with `SessionSync` and the poll timeout, `fit_viewport` rename, `mod sync; mod diff;`, `TICK` comment updated (it now does what it promised).
- `src/main.rs`: `viewer::run(table, session)`, `ls` VIEWER column, `Rm { force }`, `Prune { closed }`, `PruneFilter.closed`, `matches(modified_at, viewer_open, now)`, the skip line in `prune`, `pruned` counter.
- `tests/cli.rs`: `write_view` helper and the cases above.
- `SPEC.md`:
  - module layout: `viewer/sync.rs` and `viewer/diff.rs`; the `session.rs` line mentions `view.json`; the `time.rs` line says `ls` and `view.json` use it now.
  - "`view.json`": one sentence that the exit write leaves out `pid`, so only a crashed viewer leaves one behind.
  - "Human CLI": `tv rm <id>… [--force]` (refuses a session whose viewer is open without it), and `tv ls` lists whether a viewer is open.
- Two commits:
  1. `view.json: the viewer writes its cursor and pid; ls shows open viewers, rm --force, prune --closed` (ADI-220). Everything in Part 1: `View`, `SessionSync` with `start`/`tick`/`next_due`/`finish` (no reload yet), `has_cell`/`row_id`/`column_name`, `ls`/`rm`/`prune` changes. Each added item has a caller here.
  2. `live reload: poll table.json, keep the cursor, highlight changes` (ADI-221). `Fingerprint`, `table_fingerprint`, `changed_table` and its match in `SessionSync::tick`, `diff.rs`, `set_table`, highlights, `reload_error`, theme and render changes.

## Out of scope

- `comments.jsonl` polling, comment markers, orphan counts: ADI-223 and ADI-224.
- `goto.json`: ADI-228. It plugs into `SessionSync::tick` as one more check.
- Handling two viewers on one session beyond last-write-wins: the spec declines it.
- A `--force` for `prune`, or pruning open sessions: a bulk delete shouldn't have a way to pull a table out from under someone. `rm --force` exists for the single-session case.
- Reload on `SIGHUP` or inotify/kqueue watching: the 250ms stat is one syscall and already there. Revisit only if someone runs `tv` over a network filesystem where stat is slow.
- Highlighting removed rows or columns: there's nowhere to draw it. A status-line note like `3 rows removed` could come later if it's missed.
