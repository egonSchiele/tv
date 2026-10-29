# Plan: ADI-219 + ADI-222 Sessions: directories, IDs, `ls`, `rm`, `save`, `prune`

Linear: https://linear.app/aditya-bhargava/issue/ADI-201 (parent)
Sub-issues: ADI-219 (`session.rs`: dirs, IDs, configurable root), ADI-222 (`tv ls`, `tv rm`, `tv save`, `tv session prune`).
Depends on: ADI-200 merged (#5).

## Goal

- Every open table is a session on disk: `tv file.json` and `cat file.json | tv -` create `$TV_HOME/sessions/<id>/table.json` and open it. The title bar shows the ID.
- `tv` and `tv open` with no argument reopen the most recently modified session. `tv <id>` and `tv open <id>` open a specific one.
- `tv ls` lists sessions. `tv rm <id>…` deletes them. `tv save <id> <file.json>` writes a session's table to a file. `tv session prune --older-than 7d [--dry-run]` deletes old ones.
- `TV_HOME` overrides `~/.tv`, so tests never touch the real home directory.

## Context for the executing agent

- Read `SPEC.md`: "Sessions" (layout and IDs), "Human CLI" (the file-or-session rule), "Implementation / Stack" (random IDs from `std::hash::RandomState`, timestamps as a small RFC 3339 function), and "Changes after review" (session pruning is explicit, never automatic).
- Build on what's there; don't duplicate it:
  - `files::write_atomic`, `files::expand_tilde`, `files::slug`.
  - `Table::from_path`, `Table::parse`, `Table::save` (atomic, adds `writing {path}` context).
  - `main.rs` has `load()` for the `-`/stdin case and the `dup2_stdin` fix. Keep that code; only its caller changes.
  - `App.session_id: Option<String>` already exists and `render.rs` already draws it in the title bar (see the `k3f9x2` render test). The viewer needs no new drawing.
  - `table::single_line` for one-line titles in `ls`.
  - `tests/cli.rs` is the binary-test harness from ADI-200.
- **No new crates.** `std` covers directories, mtimes, random IDs and timestamps.
- **Two commits, one per sub-issue, each passing `make check`.** Everything a commit adds must have a caller in that commit, because `make lint` runs clippy with `-D warnings` and rustc's `dead_code` lint fires in a binary crate. The split below is arranged so that holds.
- **Tests that touch sessions use a `tempfile::TempDir` as the root**, never `~/.tv`. In `tests/cli.rs`, that's the `TV_HOME` environment variable on the child process. Unit tests construct `Sessions::new(dir)` directly and never read the environment, so they can run in parallel.

## What this plan defers, and to where

Three things ADI-222 lists depend on data that doesn't exist yet. Don't add flags or columns that do nothing; add them with the data.

| Item | Needs | Lands in |
|---|---|---|
| `ls` column "awaiting reply", importing a file's `comments` into `comments.jsonl` | the comment log | ADI-223 |
| `ls` column "viewer open", `rm --force` (refuse while a viewer is open), `prune --closed` | `view.json` with the viewer's pid | ADI-220 |
| Live reload of `table.json` | the 250ms poll | ADI-221 |

Until ADI-223, a file's `comments` ride along inside `table.json`: `Table` already holds them as opaque values and `Table::save` writes them. Nothing is lost; ADI-223 moves them into the log when it defines their type.

## Part 1: `session.rs` (ADI-219)

### Layout on disk

```
$TV_HOME/sessions/<id>/table.json     # $TV_HOME defaults to ~/.tv
```

`view.json` and `comments.jsonl` join later. A directory under `sessions/` counts as a session only if it holds `table.json`; anything else is ignored by `list`.

### Types

```rust
//! Sessions: a directory per open table under `$TV_HOME/sessions/`.
//!
//! A directory counts as a session only once it holds `table.json`. That's
//! what makes `create` atomic to readers: `table.json` is written atomically
//! as the last step, so a concurrent `tv ls` sees either no session or a
//! complete one, never a half-made one. Keep that rule when adding other
//! ways to create sessions (`tv agent new`, ADI-225).

/// The sessions root. Holds the path only; every method does its IO on call.
pub struct Sessions {
    root: PathBuf, // …/sessions
}

/// One session directory. Only `Sessions` makes these, so the ID is always
/// well-formed and the directory is always under the root.
pub struct Session {
    id: String,
    dir: PathBuf,
}

impl Sessions {
    /// `$TV_HOME/sessions`, or `~/.tv/sessions`. Takes the variable's value
    /// rather than reading the environment, so tests can pass their own.
    pub fn from_home(tv_home: Option<OsString>) -> anyhow::Result<Sessions>;
    pub fn new(root: PathBuf) -> Sessions;

    /// A new session holding `table`. Creates the root if needed.
    pub fn create(&self, table: &Table) -> anyhow::Result<Session>;
    /// An existing session. Fails with `no session "{id}"` if `id` is
    /// malformed, the directory is missing, or it fails `is_session_dir`
    /// (for example, an empty directory left by a `create` whose
    /// `write_table` failed).
    pub fn open(&self, id: &str) -> anyhow::Result<Session>;
    /// Every session, most recently modified first. A missing root is an
    /// empty list, not an error.
    pub fn list(&self) -> anyhow::Result<Vec<Session>>;
    pub fn most_recent(&self) -> anyhow::Result<Option<Session>>;
}

impl Session {
    pub fn id(&self) -> &str;
    pub fn load_table(&self) -> anyhow::Result<Table>;
    pub fn write_table(&self, table: &Table) -> anyhow::Result<()>;
    /// The newest mtime of any file in the directory. Covers `comments.jsonl`
    /// and `view.json` once they exist, with no change here.
    pub fn modified_at(&self) -> anyhow::Result<SystemTime>;
}
```

- **`from_home`:** `tv_home.filter(|home| !home.is_empty())` first, so an empty `TV_HOME=` counts as unset (the XDG convention; otherwise `PathBuf::from("").join("sessions")` is the relative path `sessions` and sessions land in the current directory). Set → `PathBuf::from(tv_home).join("sessions")`. Unset → `std::env::home_dir()` (an error, `can't find your home directory; set TV_HOME`, if that's `None`) joined with `.tv/sessions`. `from_home` must go through `Sessions::new`; if it builds `Sessions { root }` directly, `new` has only test callers and the normal build fails `dead_code`.
- **IDs:** `const ID_LEN: usize = 6` characters drawn from `const ID_ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz"`. Both `random_id` and `is_session_id` read these two constants, so the format is defined once. `fn random_id() -> String` takes `RandomState::new().build_hasher().finish()` and peels off `ID_LEN` digits in base `ID_ALPHABET.len()`. The keys behind `RandomState` are seeded from the OS once per thread, and each `RandomState::new()` then increments one of them, so every draw hashes differently; that's the property the retry loop depends on, so say it in the code comment. Index the alphabet with `usize::try_from(n % base)`, not `as usize`: pedantic clippy rejects the cast under `-D warnings`. `fn is_session_id(text: &str) -> bool` checks the length and that every byte is in the alphabet. `open` uses it, so `../x` and `/etc` never become a `Session`.
- **`create`:** make the root with `DirBuilder::new().recursive(true)`, and on Unix `.mode(0o700)` (`std::os::unix::fs::DirBuilderExt`), so tables piped in from stdin aren't readable by other users on a machine with 0755 home directories. Then loop: `random_id()`, `fs::create_dir(root.join(&id))`. `AlreadyExists` → try another ID; any other error → return it with the context `creating {dir}`. Making the directory is the collision check, so there's no exists-then-create race. Then `write_table`. If `write_table` fails (disk full), leave the empty directory: `list` and `open` ignore it, and cleaning it up would need a third exempted delete call.
- **`write_table`:** `table.save(&self.dir.join("table.json"))`. That's the atomic write; `Session` adds nothing to it.
- **`list`:** `read_dir(root)`; `NotFound` → `Ok(vec![])`. Keep entries that pass `is_session_dir` (defined under "Deleting safely"; `list`, `open` and `remove` all share it). `modified_at` returns a `Result`, so `sort_by_key` can't use `?`: first collect `Vec<(SystemTime, Session)>` with `?`, then sort by time descending, breaking ties by ID so the order, and `most_recent`, are deterministic when two sessions share an mtime (Linux file clocks are coarse enough for that to happen in tests).
- **`modified_at`:** the max of every file's `metadata()?.modified()?`, folded from `SystemTime::UNIX_EPOCH` so there's no empty case to handle. Don't include the directory's own mtime: it changes whenever an entry is added or renamed, which every write does, so it would mask the file times the `list` order test sets with `set_modified`. A directory with no files never reaches this code, because `list` and `open` require `table.json`.

### Wiring in `main.rs`

Replace `Cli::path()` with a pure function the tests can hit directly:

```rust
/// What the positional argument means. The SPEC's rule: a file if it contains
/// `.` or `/`, `-` for stdin, otherwise a session ID. No argument opens the
/// most recent session.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    MostRecent,
    Stdin,
    File(String),
    Session(String),
}

impl Target {
    fn parse(argument: Option<&str>) -> Target { … }
}
```

`Cli::path()` becomes `fn argument(&self) -> Option<&str>`: the `Open { target }` value, else the positional, else `None`. Both fields are called `target` (rename the positional from `path`, since it may be an ID). That's the only place that knows there are two spellings. `main`, in this commit:

```rust
let cli = Cli::parse();
let sessions = Sessions::from_home(std::env::var_os("TV_HOME"))?;
let session = match Target::parse(cli.argument()) {
    Target::Stdin => sessions.create(&load("-")?)?,
    Target::File(path) => sessions.create(&Table::from_path(&path)?)?,
    Target::Session(id) => sessions.open(&id)?,
    Target::MostRecent => sessions
        .most_recent()?
        .context("no sessions yet; open a file with `tv <file.json>`")?,
};
let table = session.load_table()?;
viewer::run(table, session.id().to_string())
```

- Loading back from `table.json` after `create` is a second parse of the same bytes. It's cheap, and it means all four arms end in the same two lines.
- Put the whole `match` in `fn open(sessions: &Sessions, target: Target) -> anyhow::Result<()>` now, so Part 2 can dispatch to it. The final `main` after Part 2 is:

  ```rust
  let cli = Cli::parse();
  let sessions = Sessions::from_home(std::env::var_os("TV_HOME"))?;
  match &cli.command {
      None | Some(Command::Open { .. }) => open(&sessions, Target::parse(cli.argument())),
      Some(Command::Ls) => ls(&sessions),
      Some(Command::Rm { ids }) => rm(&sessions, ids),
      Some(Command::Save { id, path }) => save(&sessions, id, path),
      Some(Command::Session { command: SessionCommand::Prune { older_than, dry_run } }) => {
          prune(&sessions, &PruneFilter { older_than: *older_than }, *dry_run)
      }
  }
  ```

  Only the `None | Open` arm reaches `Target::parse`. If Part 2 kept Part 1's unconditional `Target::parse(cli.argument())`, `tv ls` would parse as `MostRecent` and open the viewer. Use `match &cli.command`, not `match cli.command`: the second moves the field out of `cli`, and `cli.argument()` then fails to compile with "borrow of partially moved value".
- Drop `arg_required_else_help`: bare `tv` now opens the most recent session, and the "no sessions yet" error covers the empty case before the TUI starts.
- `Command::Open { target: Option<String> }`: the argument becomes optional, because ADI-219 and the SPEC's Human CLI table both say `tv open` with no argument opens the most recent session. It goes through the same `Target::parse`, so `tv open k3f9x2` and `tv open` both work. Update its doc comment (sessions exist now).
- **Existing tests change.** `bare_tv_shows_help` goes away. `open_needs_a_path` flips to "`tv open` parses to `MostRecent`". `a_path_works_with_or_without_open` is rewritten in terms of `Target`.
- `viewer::run(table, session_id: String)` sets `app.session_id = Some(session_id)`. `App::new` is unchanged, so the existing viewer tests still build an `App` without one. Fix the two stale comments while there: `App.session_id`'s "None until sessions exist", and `mod.rs`'s `TICK` comment, which says live reload is ADI-218 (it's ADI-221).
- `load("-")` keeps the `dup2_stdin` call. Nothing about stdin changes.

### Tests

- **`session.rs`**, each with its own `TempDir` as root:
  - `create` makes `root/<id>/table.json`; `id` passes `is_session_id`; `load_table` gives back the same rows.
  - Two `create`s give different IDs.
  - A pre-made directory for an ID makes `create_dir` fail with `AlreadyExists`. Test the private `fn claim_dir(&self, id) -> io::Result<Option<PathBuf>>` (or whatever the loop body is called) returns `None` for a taken ID and `Some` for a free one, so the retry path is covered without depending on randomness.
  - `open` of a missing ID fails and the message contains `no session "zzzzzz"`. `open("../x")` and `open("a/b")` fail the same way and create nothing. `open` of an ID whose directory exists but has no `table.json` fails the same way.
  - `list` on a root that doesn't exist → empty. A stray file and a directory without `table.json` under the root are ignored.
  - `list` order: create three sessions, set their `table.json` mtimes to three different times with `File::set_modified`, and check the order. `most_recent` is the first.
  - `most_recent` on an empty root → `None`.
  - `from_home(Some("/x"))` → `/x/sessions`. `from_home(None)` and `from_home(Some(""))` both end in `.tv/sessions`.
  - `random_id` is 6 chars of `[0-9a-z]`, over a few hundred draws.
- **`main.rs`:**
  - `Target::parse`: `None` → `MostRecent`; `-` → `Stdin`; `x.json`, `./x`, `dir/x`, `~/x` → `File`; `k3f9x2`, `abc` → `Session`.
  - `Cli::try_parse_from`: `tv` and `tv open` both parse to `MostRecent`; `tv k3f9x2` and `tv open k3f9x2` both give `Session("k3f9x2")`.
- **`tests/cli.rs`:** add a `home: &Path` parameter to the `tv()` helper and set `TV_HOME` on the child. Cases that exit before the TUI:
  - Empty home, `tv` → exit 1, stderr contains `no sessions yet`.
  - `tv zzzzzz` → exit 1, stderr contains `no session "zzzzzz"`.
  - The two existing cases (`bad_json_on_stdin_exits_1`, `a_missing_file_exits_1`) also assert that `sessions/` was not created under the home. That pins down "load before create", which nothing else tests.

## Part 2: `ls`, `rm`, `save`, `session prune` (ADI-222)

### CLI

```rust
#[derive(Subcommand)]
enum Command {
    /// Open a file (`.` or `/` in the name), `-` for stdin, or a session ID.
    /// With no argument, the most recent session.
    Open { target: Option<String> },
    /// List sessions, most recent first.
    Ls,
    /// Delete sessions.
    Rm { ids: Vec<String> },  // at least one: #[arg(required = true)]
    /// Write a session's table, comments included, to a JSON file.
    Save { id: String, path: String },
    /// Session maintenance.
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
}

#[derive(Subcommand)]
enum SessionCommand {
    /// Delete sessions that match every given filter. At least one filter is required.
    Prune {
        /// Modified longer ago than this, e.g. `7d` or `12h`.
        #[arg(long, value_parser = parse_duration)]
        older_than: Option<Duration>,
        /// Print what would be deleted without deleting it.
        #[arg(long)]
        dry_run: bool,
    },
}
```

`parse_duration(text) -> Result<Duration, String>`: a number followed by `d` or `h`. Anything else: `expected a duration like 7d or 12h`. Clap shows that message with the flag name. Two traps:

- Split the unit off with `strip_suffix('d')` / `strip_suffix('h')`. `text.split_at(text.len() - 1)` panics when the last character is multibyte (`7é`).
- Multiply with `checked_mul` and return the same error on overflow. `99999999999999999d` parses as a `u64`, and a plain `n * 86_400` panics in debug builds, which is what tests use.

### `session.rs` additions

```rust
impl Sessions {
    /// Delete a session: its known files one by one, then the empty directory.
    /// Never recursive. See "Deleting safely" below for the rules this follows
    /// and the checks it makes first. It lives on `Sessions` because that's
    /// what knows the root; a `Session` shouldn't be handed its own root back.
    pub fn remove(&self, session: Session) -> anyhow::Result<()>;
}

/// Every file a session directory may hold. `remove` deletes exactly these
/// and nothing else; add a name here when a new file joins the layout.
/// `goto.json` is ADI-228's cursor request from the agent; it's listed now so
/// `tv rm` doesn't refuse a session with a pending goto.
const SESSION_FILES: &[&str] = &["table.json", "view.json", "comments.jsonl", "goto.json"];

impl Session {
    /// Title, row and column counts, and mtime for listings.
    pub fn summary(&self) -> anyhow::Result<Summary>;
}

pub struct Summary {
    pub id: String,
    pub title: Option<String>,
    pub rows: usize,
    pub columns: usize,
    pub modified_at: SystemTime,
}
```

`summary` loads the table. For `ls` that's one parse per session, which is fine for a directory a person manages by hand. If it's ever slow, the fix is a sidecar file, not a cache in memory.

### Deleting safely

`rm` and `prune` are the first code in `tv` that deletes directories, so this is where the rules get set. The worry is the usual one: a bug or a bad `TV_HOME` pointing a delete at something that matters. Six layers, strongest first. Each would have to fail for a wrong delete to happen.

1. **No recursive delete anywhere in the crate.** `remove` first reads the directory and checks every entry is either a name in `SESSION_FILES` or a `.*.tmp` left by a crashed `write_atomic` (matched with `file_name` starts-with `.` and ends-with `.tmp`, nothing looser). Anything else, including a subdirectory, makes it return `refusing to delete {dir}: unexpected entry {name}` before a single file is removed, so the session stays intact and openable. Only then does it delete those files one by one, **`table.json` last**, and call the non-recursive `fs::remove_dir`. If something appeared in between (a viewer writing `view.json` while `tv rm` runs; nothing prevents that until ADI-220), `remove_dir` fails with "directory not empty" and that's the backstop. Because `table.json` was the last file deleted, the backstop firing never leaves a directory without it, so the session is still listed and `tv rm` can simply be run again. `std::fs::remove_dir_all` is the only primitive that can take out a home directory, so the binary doesn't contain it.

2. **Clippy enforces that.** Add `clippy.toml` at the repo root:

   ```toml
   # Every delete in the crate is behind an explicit `expect`, so `grep disallowed_methods`
   # lists them all. Recursive delete is banned outright.
   disallowed-methods = [
       { path = "std::fs::remove_dir_all", reason = "never delete recursively; remove known files, then remove_dir (see session.rs)" },
       { path = "std::fs::remove_dir", reason = "only Sessions::remove deletes directories" },
       { path = "std::fs::remove_file", reason = "only Sessions::remove and files::write_atomic delete files" },
   ]
   ```

   Exactly two functions get `#[expect(clippy::disallowed_methods, reason = "…")]`: `files::write_atomic` (its temp-file cleanup, the crate's only existing delete) and `Sessions::remove`. Use `expect`, not `allow`: `expect` warns once the exempted call is gone, so a stale exemption can't linger. `clippy.toml` and both exemptions land in the same commit (ADI-222), or that commit fails `make lint`. `make check` and CI fail if a delete appears anywhere else. ADI-228's viewer-side delete of `goto.json` will be the third exemption; update the `remove_file` reason text then.

3. **Deletes take IDs, never paths.** `tv rm` and `prune` accept session IDs. `Sessions::remove` takes a `Session`, and only `open`, `list` and `create` make those, always as `root.join(id)` after `is_session_id(id)` passes. No string the user typed ever reaches a delete call.

4. **`remove` checks the target looks like a session, right before deleting.** The same predicate `list` and `open` use, as one `fn is_session_dir(&self, dir: &Path) -> bool`: the parent is `self.root`, the file name passes `is_session_id`, `table.json` is present, and `symlink_metadata(dir)` says it's a real directory, not a symlink. If the check fails, `remove` returns `refusing to delete {dir}: not a session directory` and does nothing. It's redundant with layer 3 by design: the check is cheap and the cost of being wrong is high.

5. **The root can't be a home directory.** `from_home` always appends `sessions`, so `TV_HOME=/` gives `/sessions` and `TV_HOME=~` gives `~/sessions`. A delete only ever targets `<something>/sessions/<6 chars>/<known file>`.

6. **Tests can't do damage even when they fail.** Every `remove` test runs in a `TempDir` root with a decoy session alongside and a decoy file in the root, and asserts both survive. The refusal cases test `is_session_dir` on paths inside the temp dir, not a real delete pointed at something real. If a refusal test fails, the worst case is a deleted temp file.

Not doing: a trash directory, or a confirmation prompt on `prune`. The explicit `--older-than` filter and `--dry-run` are enough for a tool that only ever deletes its own few small files, and a prompt gets in the way of scripting.

### `time.rs`

```rust
/// `SystemTime` as UTC RFC 3339 with second precision: `2026-10-28T19:02:11Z`.
pub fn rfc3339(time: SystemTime) -> String;
```

Days-since-epoch to civil date is the standard algorithm (Howard Hinnant's `civil_from_days`, about ten lines); hours, minutes and seconds are divisions. Comments and `view.json` need the same format later, which is why it's a module and not a helper in `main.rs`. Times before 1970 don't occur; `duration_since(UNIX_EPOCH)` falling back to zero is enough. Because of that, every intermediate value is non-negative: write the whole thing in `u64` with no `as` casts, rather than copying the signed version, which pedantic clippy rejects.

### Commands

Each is a `fn` in `main.rs`, called from the `match` on `Command`. They take `&Sessions`, print to stdout, and return `anyhow::Result<()>`. Errors print `Error: …` and exit 1 through `main`, like everything else.

**`ls`:** a padded plain-text table.

```
ID      ROWS×COLS  MODIFIED              TITLE
k3f9x2  40×9       2026-10-28T19:02:11Z  Open pull requests
m8q1zz  3×2        2026-10-27T08:15:40Z  (untitled)
```

TITLE goes last because it's the only free-text column: in the middle, one long title pushes MODIFIED off-screen on every row; at the end it only wraps its own line, and the later "awaiting reply" and "viewer open" columns fit before it (`ls -l` and `docker ps` do the same).

- Build `Vec<Vec<String>>` of cells (header first), compute each column's max width with `layout::display_width`, pad each cell with `layout::fit(cell, width, Align::Left)`, join with two spaces, trim trailing spaces. One `fn format_table(rows: &[Vec<String>]) -> String`, pure, tested on its own. `fit` pads by display width, and because `width` is the column's maximum it never truncates. Don't use `format!("{cell:<width$}")`: that pads by character count, so a row with a CJK title comes out misaligned.
- `layout` is private to the viewer today (`mod layout;` in `viewer/mod.rs`). Make it `pub mod layout;` so `main.rs` can reach it.
- Title through `table::single_line`; `(untitled)` when `None`.
- A session whose `table.json` doesn't parse gets a placeholder row (`?` for ROWS×COLS, `(can't read table.json)` for TITLE) instead of failing the whole listing. Match on `session.summary()` per row. Writes are atomic and validated, so this takes a hand edit or a version mismatch, but when it happens `ls` is how you find the ID to `rm`.
- No sessions: print `no sessions` and exit 0.
- "Awaiting reply" and "viewer open" columns come with ADI-223 and ADI-220, as the table above says.

**`rm <id>…`:** deduplicate the IDs (`tv rm abc123 abc123` should not fail on the second copy), `open` each first so a typo in the second ID fails before the first is deleted, then `remove` each and print `removed {id}`. Refusing while a viewer is open arrives with ADI-220.

**`save <id> <path>`:** `sessions.open(id)?.load_table()?.save(&files::expand_tilde(path))`, then print `wrote {rows} rows to {path}`. Overwrites an existing file: a CLI command with an explicit output path behaves like `cp`. The viewer's `:w` refusal is for interactive typing, where the default path makes clobbering easy; neither applies here.

**`session prune`:** with no filter, fail with `give a filter: --older-than <duration>` rather than deleting everything. The filters are data, and one predicate applies them:

```rust
struct PruneFilter {
    older_than: Option<Duration>, // ADI-220 adds `closed: bool`
}

impl PruneFilter {
    fn is_empty(&self) -> bool;
    /// True when a session last modified at `modified_at` matches every
    /// filter that is set. ADI-220 adds a `viewer_open: bool` parameter.
    fn matches(&self, modified_at: SystemTime, now: SystemTime) -> bool;
}
```

`matches` takes the mtime, not a `Summary`, so `prune` never has to parse a table to decide: it filters on `session.modified_at()` and loads the title only for the printed line, best effort (`load_table().ok()`, falling back to `(can't read table.json)`). That way the cleanup tool can still clean up a session whose `table.json` is broken, which is the one that most needs it. The age test is `now.duration_since(modified_at).unwrap_or_default() >= limit`. Don't write `modified_at < now - limit`: `SystemTime - Duration` panics when `limit` is huge. `unwrap_or_default` treats a future mtime as age zero, and `>=` makes `--older-than 0d` match everything without relying on the clock moving between writing the file and running the command.

The command is then `list()`, `filter(matches)`, and for each match: print `would remove {id}  {title}` under `--dry-run`, else `remove` and print `removed {id}  {title}`. End with `nothing to prune` if nothing matched. Adding `--closed` in ADI-220 is one field in the struct, one clause in `matches`, and one clap flag; the loop doesn't change. `matches` is pure, so it's unit-tested without a filesystem.

### Tests

- **`time.rs`:** `rfc3339(UNIX_EPOCH)` is `1970-01-01T00:00:00Z`. Two more known dates, one on a leap day and one at a year boundary, with the expected strings written into the test. Put both commands that produce them in the comment, since CI runs on both systems: macOS `date -u -r <secs> +%Y-%m-%dT%H:%M:%SZ`, Linux `date -u -d @<secs> +%Y-%m-%dT%H:%M:%SZ`.
- **`main.rs`:**
  - `parse_duration`: `7d` → 7 days, `12h`, `0d`; `7`, `d`, `7w`, `-1d`, `7é`, `99999999999999999d` → `Err`.
  - `PruneFilter::matches`: an mtime 8 days before `now` matches `older_than: 7d`; 6 days before doesn't; an mtime after `now` doesn't; `older_than: 0d` matches an mtime equal to `now`; a huge `older_than` (`Duration::MAX`) returns `false` without panicking; an empty filter reports `is_empty`.
  - `format_table`: widths follow the widest cell, two-space gaps, no trailing spaces, a cell with a CJK character pads by display width.
  - `Cli::try_parse_from`: `tv ls`; `tv rm a b`; `tv rm` → `MissingRequiredArgument`; `tv save k3f9x2 out.json`; `tv session prune --older-than 7d --dry-run`; `tv session prune --older-than soon` → an error naming the flag.
- **`session.rs`:**
  - `remove` deletes the session's directory and leaves a sibling session and a stray file in the root alone.
  - `remove` on a session whose directory also holds a `.table.json.123.tmp` deletes that too and succeeds.
  - `remove` on a session whose directory holds an unknown file (`notes.txt`) or a subdirectory fails with `refusing to delete … unexpected entry`, and both the unknown entry and `table.json` are still there.
  - `is_session_dir` is false for: the root itself, a file in the root, a directory with a non-ID name, an ID directory without `table.json`, and a symlink to a valid session directory. True for a created session.
  - `summary` of a created session has the right title, rows and columns.
- **`tests/cli.rs`**, all against a `TempDir` passed as `TV_HOME`. Sessions are made by writing `sessions/<id>/table.json` directly (a copy of `examples/prs.json`), since no non-TUI command creates one until `tv agent new` (ADI-225). That's also a test of the on-disk contract.
  - `tv ls` on an empty home: exit 0, stdout `no sessions`.
  - After writing `abc123`: `tv ls` stdout contains `abc123`, `Open pull requests` and `40×9`.
  - With a second session `bad000` whose `table.json` is `not json`: `tv ls` exits 0, still lists `abc123`, and shows `bad000` with `(can't read table.json)`. `tv session prune --older-than 0d --dry-run` lists both.
  - `tv save abc123 {dir}/out.json`: exit 0, `out.json` parses as a table with 40 rows.
  - `tv session prune --older-than 7d --dry-run`: prints `nothing to prune`; the directory remains.
  - `tv session prune --older-than 0d --dry-run`: stdout contains `would remove abc123`; the directory remains.
  - `tv session prune --older-than 0d`: stdout contains `removed abc123`; the directory is gone.
  - `tv session prune` with no flags: exit 1, stderr contains `give a filter`.
  - `tv rm abc123` after re-creating it: exit 0; the directory is gone. `tv rm abc123 abc123` removes it once and exits 0. `tv rm zzzzzz`: exit 1, `no session "zzzzzz"`.
  - `tv rm abc123` when its directory also holds `notes.txt`: exit 1, stderr contains `refusing to delete`, and `notes.txt` and `table.json` both remain, so `tv ls` still lists the session.

## Manual check

In a scratch directory, with a throwaway home so the real `~/.tv` stays clean:

```sh
export TV_HOME=$(mktemp -d)
cp ~/tv/examples/prs.json .
```

1. `tv prs.json`. The grid appears with a 6-character ID at the right of the title bar. `q`.
2. `tv ls` shows that one session with `Open pull requests` and `40×9`.
3. `tv` (no arguments) reopens it; the same ID shows. `q`. `tv open` and `tv <that id>` do too.
4. `cat prs.json | tv -` creates a second session. `tv ls` lists two, the new one first.
5. `tv save <id> copy.json`, then `tv copy.json`: same table, a third session.
6. `tv session prune --older-than 0d --dry-run` lists all three and deletes nothing. Without `--dry-run` it deletes them. `tv ls` prints `no sessions`, and `tv` prints the "no sessions yet" error with exit 1.
7. `unset TV_HOME`.

## Acceptance

```sh
make check
TV_HOME=$(mktemp -d) cargo run -q -- ls            # no sessions
TV_HOME=$(mktemp -d) cargo run -q -- ; echo $?      # Error: no sessions yet … ; 1
```

Plus the manual check above.

## Deliverables

- `src/session.rs`: `Sessions`, `Session`, `Summary`, `random_id`, `is_session_id`, `SESSION_FILES`, `is_session_dir`, `remove`
- `clippy.toml`: the `disallowed-methods` list; `#[expect]`s on `write_atomic` and `Sessions::remove` (same commit)
- `src/time.rs`: `rfc3339`
- `src/main.rs`: `Target`, `Cli::argument`, `open`, the new `Command` variants, `SessionCommand`, `PruneFilter`, `parse_duration`, `format_table`, `ls`/`rm`/`save`/`prune`
- `src/viewer/mod.rs`: `run(table, session_id)`, `pub mod layout`; `src/viewer/app.rs`: comment fix
- `Makefile`: `run` sets `TV_HOME=$(CURDIR)/target/tv-home` so `make run` doesn't fill the real `~/.tv` with a session per run (`make clean` then removes them too)
- `tests/cli.rs`: `TV_HOME` plumbing and the cases above
- `SPEC.md`: module layout gains `time.rs`
- Two commits: `sessions: a directory per open table, TV_HOME, open by id or most recent` (ADI-219) and `cli: tv ls, rm, save, session prune` (ADI-222)

## Out of scope

- `view.json`, viewer-open detection, `rm --force`, `prune --closed`: ADI-220.
- `comments.jsonl` and importing a file's comments into it: ADI-223.
- Live reload: ADI-221.
- A `--home` flag (ADI-219 says "consider"). `TV_HOME` does the job for tests and for anyone who wants a different root, and a flag would have to be threaded into every command. Not worth it until someone asks.
- Relative times in `ls` ("3m ago"). RFC 3339 is unambiguous and needs no clock-skew logic; revisit if the column proves hard to read.
