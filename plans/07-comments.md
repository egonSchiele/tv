# Plan: ADI-223 + ADI-224 comments

Linear: https://linear.app/aditya-bhargava/issue/ADI-201 (parent)
Sub-issues: ADI-223 (`comments.rs`: the event log and folding it into threads, plus three carry-overs: importing a file's comments when it's opened, an AWAITING column in `tv ls`, and polling `comments.jsonl` in the viewer), ADI-224 (comments in the viewer: `c`, markers, `]`/`[`, the thread in the cell popup, `d`).
Depends on: PR #7 merged (`6a1620b`). This is ADI-201's last PR, so merging it closes the parent.

## Goal

- A session's comments live in `comments.jsonl`, an append-only log of `add` and `delete` events. The viewer appends to it now, and the agent CLI will later. `comments.rs` folds the log into threads: a comment on a cell plus its replies, with `awaiting_reply` and `orphaned` worked out from the log and the current table.
- Opening a file that has `comments` (for example one `tv save` wrote) moves them into the new session's log. `tv save` and `:w` write the threads back out, so saving and reopening keeps them. `tv ls` shows how many threads are awaiting a reply.
- The viewer polls `comments.jsonl` on the tick it already has, the same way it polls `table.json`.
- In the viewer:
  - `c` comments on the cursor cell, or replies to the thread already there.
  - A `▸` marks cells with threads: yellow when a thread is awaiting a reply, green when the agent replied last.
  - `]` and `[` jump between those cells.
  - The cell popup (`Enter`) shows the cell's thread, and `d` there deletes it after you press `y`.
  - The status line counts the threads awaiting a reply and the orphaned ones.

## Context for the executing agent

- Read `SPEC.md`:
  - "Sessions / `comments.jsonl`" and "Live updates".
  - "Viewer": Layout (the status line), Rendering (the comment marker), and Keys (`Enter`, `c`, `d`, `]`, `[`).
  - "Agent CLI / `tv agent comments`": the thread shape, which is also what a saved file's `comments` holds.
  - "Table format" (`comments`).
- Build on what's there; don't duplicate it:
  - `table.rs`: `Table.comments: Vec<Value>` with the comment "opaque until ADI-223 defines the type", and the same field on `RawTable`. `Table::save` writes `comments` when there are any. Also `Table::row_id` and `Table::cell`.
  - `session.rs`:
    - `SESSION_FILES` already lists `comments.jsonl`, so `Sessions::remove` already deletes it.
    - `Sessions::create`: read the module doc. A directory counts as a session only once `table.json` exists, and `create` writes `table.json` last.
    - Also there: `Session::{read_view, write_view, table_fingerprint}`, `Fingerprint`, `Summary`, `random_id`.
  - `viewer/sync.rs`:
    - `SessionSync` owns the `Session` and does all the viewer's file IO.
    - `tick` has `changed_table` and the match on its result; `file_error` formats status-line errors.
    - The module doc already reserves `tick` for the `comments.jsonl` poll.
    - `start` sets `seen: None`, so the first tick reads `table.json` again. The comments poll starts the same way.
  - `viewer/app.rs`:
    - `App`: private `table`, `set_table`, `has_cell`, `row_id`, `column_name`, `set_message`.
    - `Mode`, `CommandLine`, and the `Mode::Command` arm of `handle_key`, which holds the line-editing keys.
    - The bindings tables (`GRID_BINDINGS`, `RECORD_BINDINGS`, `lookup`), `open_cell_view`, and `save`.
  - `viewer/render.rs`:
    - `grid_line`, the only code that lays out a grid row, and `cell_style`.
    - `status_line` and `draw_status`.
    - `draw_detail`: its bottom two lines are already set aside for comments ("Comments" and "none yet").
    - `draw_help`.
  - `main.rs`: `open`, `ls`, `save`, `format_table`.
  - `tests/cli.rs`: `tv`, `write_session`, `succeeds`, `write_view`.
- **No new crates.** Everything here is serde, serde_json and std.
- **Two commits, one per sub-issue, each passing `make check`.** `dead_code` fires in a binary crate, so everything a commit adds needs a non-test caller in that commit. The split below keeps that true.
- **Deleting a thread deletes no file.** It appends a `delete` event. There's no new `remove_file` or `remove_dir`, so the deletes `clippy.toml` allows don't change. Tests don't delete files either. To make a session disappear under a viewer, use `sessions.remove(sessions.open(id)?)` on a second handle, as plan 06 did.
- **Pedantic clippy:**
  - The usual traps: `match_same_arms`, and `private_interfaces`. `CommentInput`, `Marker` and `DetailAction` appear in `pub` items, so they're `pub` too.
  - Two lints that came up while compiling this plan's snippets, both in test code:
    - `format_collect`: `.map(|line| format!("{line}\n")).collect::<String>()` is rejected. Write the string with one `format!`, or push in a loop.
    - `assert_is_empty`: `assert!(x.is_empty())` is rejected when `x` is `PartialEq + Debug`, for example a `Vec<Thread>`. Write `assert!(x.is_empty(), "{x:?}")`.

  I compiled the `comments.rs` code below, `Table::row_indices`, and `diff.rs` switched to it, in a scratch copy of `main`:
  - It's clean under `cargo clippy --all-targets -- -D warnings`, and the existing tests still pass.
  - Serde writes the spec's three example log lines back byte for byte.
  - `dead_code` doesn't flag `Author::Agent` or `Event::Delete`, even though in commit 1 only serde creates them.
- Render changes are snapshot tests. `make snapshots` writes the new `.txt` files; read each one before committing it.

## What this plan defers, and to where

| Item | Needs | Lands in |
|---|---|---|
| `tv agent comments`, `reply`, `comment` | the agent CLI | ADI-227. It uses `Session::read_log`, `comments::threads`, `Log::warning` (on stderr, since stdout is JSON), `Session::append_events`, `comments::new_id`, and `Event::comment` and `Event::reply` with `Author::Agent`. |
| `awaiting_reply` in `tv agent sessions` | the agent CLI | ADI-225, from `Summary.awaiting_reply` |
| What `tv agent push` does with `comments` in a pushed table | push | ADI-225. `Session::write_table` leaves them out (Part 1), so they're dropped unless push imports them first. |
| Applying `goto.json` | `tv agent goto` | ADI-228, as one more check in `SessionSync::tick` |

## Part 1: the comment log (ADI-223)

### Layout on disk

```
$TV_HOME/sessions/<id>/
  table.json       # never holds comments
  view.json
  comments.jsonl   # new: appended to by the viewer, and later the agent CLI
```

The events are the spec's, one JSON object per line:

```
{"op":"add","id":"c7h2k1","author":"user","row_id":"412","column":"title","text":"Why is this one still open?","ts":"2026-10-28T19:03:00Z"}
{"op":"add","id":"c9m4p0","author":"agent","reply_to":"c7h2k1","text":"Waiting on review from infra.","ts":"2026-10-28T19:04:12Z"}
{"op":"delete","id":"c7h2k1","ts":"2026-10-28T19:05:00Z"}
```

A saved file's `comments` is a list of threads, in the shape `tv agent comments --all` will print:

```json
"comments": [
  {
    "id": "c7h2k1",
    "row_id": "412",
    "column": "title",
    "value": "Fix login redirect",
    "awaiting_reply": false,
    "orphaned": false,
    "messages": [
      { "id": "c7h2k1", "author": "user", "text": "Why is this one still open?", "ts": "2026-10-28T19:03:00Z" },
      { "id": "c9m4p0", "author": "agent", "text": "Waiting on review from infra.", "ts": "2026-10-28T19:04:12Z" }
    ]
  }
]
```

**Where comments live.** In a session, they live only in `comments.jsonl`. `Table.comments` is the file format's field. It's filled in two places: when a file is parsed, so `create` can import the comments, and when saving, from the log. A session's `table.json` never has it.

With only one copy, nobody has to work out which copy is current. It also makes `tv agent get`, which ADI-225 defines as "the table without comments", just `load_table`.

### `src/comments.rs` (new)

This module does no file IO: the event and thread types, the log's text format, folding, and importing. Reading and appending the file is `Session`'s job, the same split as for `view.json`. (It isn't quite pure: in Part 2, `Event::comment` reads the clock and `new_id` draws random IDs.)

```rust
//! Comments: the `comments.jsonl` event log and the threads it folds into.
//! No file IO here: `Session` reads and appends the file.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Author {
    User,
    Agent,
}

/// One line of `comments.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Event {
    Add(Add),
    /// Deletes the comment with this ID. Deleting a top-level comment
    /// deletes its whole thread.
    Delete { id: String, ts: String },
}

/// A comment. A top-level one names its cell (`row_id` and `column`); a
/// reply names its thread (`reply_to`, the top-level comment's ID).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Add {
    pub id: String,
    pub author: Author,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    pub text: String,
    pub ts: String, // RFC 3339, from `time::rfc3339`
}

/// A comment on a cell and its replies, as a saved file's `comments` holds
/// it and `tv agent comments` will print it. `threads` works out `value`,
/// `awaiting_reply`, `orphaned` and `cell` against the current table.
/// `import` ignores them, and a file may leave them out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub id: String, // the top-level comment's ID
    pub row_id: String,
    pub column: String,
    /// The cell's value now: `null` when the thread is orphaned or the row
    /// has no such field. A plain `Value`, not an `Option`: JSON's null is
    /// already a `Value`, and with an `Option` a null cell would be
    /// `Some(Null)` here but `None` once read back from a file.
    #[serde(default)]
    pub value: Value,
    #[serde(default)]
    pub awaiting_reply: bool,
    #[serde(default)]
    pub orphaned: bool,
    pub messages: Vec<Message>,
    /// Where the cell is in the table `threads` folded against, as (row,
    /// column) indices; `None` when orphaned. The viewer draws markers from
    /// it. Not part of the file format. Added in Part 2, together with its
    /// first reader (`App::refold`); in Part 1, `threads` keeps `cell` in a
    /// local.
    #[serde(skip)]
    pub cell: Option<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub author: Author,
    pub text: String,
    pub ts: String,
}

/// The log as read: its events, and the lines that weren't events.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Log {
    pub events: Vec<Event>,
    skipped: Vec<usize>, // 1-based line numbers
}

impl Log {
    /// What to tell the user about skipped lines, if there were any:
    /// `skipped lines that aren't events: 7, 9`. Callers put the file name in
    /// front, the way `file_error` does.
    pub fn warning(&self) -> Option<String> {
        if self.skipped.is_empty() {
            return None;
        }
        let lines: Vec<String> = self.skipped.iter().map(ToString::to_string).collect();
        Some(format!("skipped lines that aren't events: {}", lines.join(", ")))
    }
}

/// The events in `text`, one per line, in order. A line that doesn't parse
/// is skipped and its number recorded. Blank lines are ignored: they hold
/// nothing to lose.
pub fn parse_log(text: &str) -> Log {
    let mut log = Log::default();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(line) {
            Ok(event) => log.events.push(event),
            Err(_) => log.skipped.push(index + 1),
        }
    }
    log
}

/// `events` as log lines, each ending in `\n`.
pub fn log_text(events: &[Event]) -> serde_json::Result<String> {
    events
        .iter()
        .map(|event| Ok(serde_json::to_string(event)? + "\n"))
        .collect()
}
```

`Thread` field order is the spec's, and serde writes fields in declaration order, so keep it. The scratch build printed `{"id":…,"row_id":…,"column":…,"value":…,"awaiting_reply":…,"orphaned":…,"messages":[…]}`, with `cell` left out.

`log_text` collects into a `Result<String, _>`: Rust can collect an iterator of `Result`s into one `Result`, which stops at the first error. That's the declarative form of a loop with `?` in it.

**Why skip bad lines, when a broken `table.json` is an error?** The next push replaces a broken `table.json`. A bad line in an append-only log never goes away, so failing on it would hide every comment for good, in the viewer, `tv ls` and `tv save` alike. Bad lines can only come from a hand edit or a write that was cut short (a full disk). For the same reason, `Session::read_log` reads the file with `String::from_utf8_lossy`, so one bad byte costs one line, not the whole log.

**Skipped isn't silent.** `Log::warning` says which lines were skipped:
- The viewer shows it in the status line each time it reads the log.
- `tv save` prints it after its `wrote …` line.
- `tv ls` shows only a count, so it doesn't repeat the warning on every listing.

That costs no error state: the viewer only reads the log when it changes, so the warning comes up once per change.

**Folding rules.** `threads` follows these, and each one gets a test:

1. Every ID that a `delete` names is deleted, wherever the `delete` is in the log, before or after the `add`. IDs are random and never reused, so the order can't change the answer. A deleted top-level comment takes its thread with it. A deleted reply just disappears from its thread.
2. If an ID appears in two `add`s, the first in the log wins. Only a hand edit can cause this; `new_id` (Part 2) never draws a taken ID.
3. A thread starts from an `add` that has no `reply_to` and has both `row_id` and `column`. Its replies are the `add`s whose `reply_to` is its ID, in log order.
   - Replies are looked up by thread ID, so a reply logged before its comment (two writers racing) still finds it.
   - A reply whose `reply_to` isn't a thread's ID is never looked up, so it's dropped: an unknown ID, a deleted one, or another reply's ID.
   - An `add` with no `reply_to` and an incomplete cell starts nothing.
4. `awaiting_reply`: the last message is the user's.
5. `cell`: the thread's row (`Table::row_indices`) and column, if both exist. `orphaned` is `cell.is_none()`.
6. `value`: that cell's value (`Table::cell`). It's `null` when the thread is orphaned or the row has no such field.

Each `Thread` is built once, with every field worked out from its inputs. Nothing is filled in with a placeholder and fixed up by a later loop.

The map from row ID to index is a new `Table::row_indices`, so the map `diff::changed_cells` builds inline lives in one place. `changed_cells` switches to it in the same commit; it's a one-line change:

```rust
/// Each row's ID (`row_id`) and its index. Row IDs are unique: a key's
/// values are checked (`validate_key`), and `#<index>` can't repeat.
pub fn row_indices(&self) -> HashMap<String, usize> {
    (0..self.rows.len())
        .map(|index| (self.row_id(index), index))
        .collect()
}
```

This is the version that compiled and passed the probe tests:

```rust
/// Fold the log into threads, in the order their top-level comments were
/// logged, with the derived fields worked out against `table`. Deleted IDs
/// are gone wherever the `delete` is; the first `add` of an ID wins; a reply
/// joins the thread its `reply_to` names, wherever it is in the log, and is
/// dropped if that isn't a thread.
pub fn threads(events: &[Event], table: &Table) -> Vec<Thread> {
    let deleted: HashSet<&str> = events
        .iter()
        .filter_map(|event| match event {
            Event::Delete { id, .. } => Some(id.as_str()),
            Event::Add(_) => None,
        })
        .collect();
    // The first `add` of each ID, in log order, unless it was deleted.
    let mut seen = HashSet::new();
    let adds: Vec<&Add> = events
        .iter()
        .filter_map(|event| match event {
            Event::Add(add) => Some(add),
            Event::Delete { .. } => None,
        })
        .filter(|add| !deleted.contains(add.id.as_str()) && seen.insert(add.id.as_str()))
        .collect();
    // Replies by the ID they reply to, in log order.
    let mut replies: HashMap<&str, Vec<&Add>> = HashMap::new();
    for add in &adds {
        if let Some(reply_to) = &add.reply_to {
            replies.entry(reply_to.as_str()).or_default().push(add);
        }
    }
    let rows = table.row_indices();
    adds.iter()
        .filter_map(|top| {
            let (None, Some(row_id), Some(column)) = (&top.reply_to, &top.row_id, &top.column)
            else {
                return None;
            };
            let thread_replies = replies.get(top.id.as_str()).into_iter().flatten();
            let messages: Vec<Message> = iter::once(top)
                .chain(thread_replies)
                .map(|add| message(add))
                .collect();
            let row = rows.get(row_id).copied();
            let col = table.columns.iter().position(|candidate| candidate.name == *column);
            let cell = row.zip(col);
            let value = cell
                .and_then(|(row, _)| table.cell(row, column))
                .cloned()
                .unwrap_or_default();
            let awaiting_reply = messages
                .last()
                .is_some_and(|message| message.author == Author::User);
            Some(Thread {
                id: top.id.clone(),
                row_id: row_id.clone(),
                column: column.clone(),
                value,
                awaiting_reply,
                orphaned: cell.is_none(),
                messages,
                cell, // from Part 2, when the field arrives
            })
        })
        .collect()
}

fn message(add: &Add) -> Message; // copies id, author, text, ts

/// The events that recreate `threads` in a new log. Each thread's first
/// message becomes a top-level comment with the thread's ID and cell, and
/// the rest become replies to it. IDs and timestamps are kept. A thread with
/// no messages is skipped.
pub fn import(threads: &[Thread]) -> Vec<Event>;
```

`import` gives the top-level comment `thread.id`, not `messages[0].id`. The two are the same in anything `tv` wrote, and a reply's `reply_to` has to match whatever the top-level comment's ID is, so using one of them for both keeps them consistent.

### `session.rs` additions

```rust
/// Public like the other two: the viewer names it in status-line errors.
pub const COMMENTS_FILE: &str = "comments.jsonl";

impl Session {
    /// `comments.jsonl` as read, or an empty `Log` if there's no log yet.
    /// Lines that don't parse are skipped and listed (`comments::parse_log`).
    pub fn read_log(&self) -> anyhow::Result<Log>;

    /// Append `events` to `comments.jsonl` in one write, creating the file
    /// (never the directory), then flush it to disk. No events, no write.
    pub fn append_events(&self, events: &[Event]) -> anyhow::Result<()>;

    /// `comments.jsonl`'s fingerprint, or `None` if there's no log yet.
    pub fn comments_fingerprint(&self) -> anyhow::Result<Option<Fingerprint>>;
}
```

There's no `Session::threads(table)` wrapper. It would have to drop `Log::warning` on the floor, which is the silent skip this design avoids. The two callers, `summary` and `tv save`, call `read_log` and `comments::threads(&log.events, &table)` themselves.

- `SESSION_FILES` uses `COMMENTS_FILE` in place of the `"comments.jsonl"` literal.
- `read_log`:
  - `fs::read` the file. `NotFound` becomes `Ok(Log::default())`; any other error gets a `reading {path}` context.
  - Then `comments::parse_log(&String::from_utf8_lossy(&bytes))`.
- `append_events`:
  - Open with `OpenOptions::new().append(true).create(true)` and make one `write_all` of `comments::log_text(events)?`, then `sync_all`.
  - Errors get a `writing {path}` context.
  - Return early on an empty slice. That isn't a shortcut: without it, an empty append would still create the file and flush it. `create` calls it for every new session, and Part 2's `tick` calls it every 250ms.
  - The doc comment says why it doesn't use `write_atomic`. A temp file and rename would replace the log, and whichever of two writers renamed second would wipe out the other's append. With `O_APPEND`, each write lands at the end, and a single small write keeps a line whole.
  - `create(true)` creates the file but not the directory. After `tv rm --force`, this fails with `NotFound` instead of bringing the session back.
  - The `sync_all` costs about 4ms on macOS, once per comment. A comment is something the user typed, which makes it worth more than `view.json`, which is already flushed the same way.
- **Fingerprints for two files.** Pull the body of `table_fingerprint` into a private `fn fingerprint(path: &Path) -> io::Result<Fingerprint>`, and use it in two places:
  - `table_fingerprint`, which still turns `NotFound` into an error: `table.json` has to exist.
  - `comments_fingerprint`, which turns `NotFound` into `Ok(None)`: no log yet is normal.

  Update `Fingerprint`'s doc comment so it covers both files. The log only ever grows, so its size changes on every append, even when the clock is coarse.
- **`Sessions::create` imports.** Before `session.write_table(table)?`:

  ```rust
  // The comments go first: the directory isn't a session until `table.json`
  // exists, so a reader never sees the table without its comments.
  session.append_events(&comments::import(&table.comments))?;
  ```

  Update the comment about a failed `write_table` leaving the directory behind: it may now hold `comments.jsonl` too, and `list` and `open` still ignore it. Also update the module doc's sentence about `create` writing `table.json` as the last step: that's still true, and the log comes before it.
- **`write_table` leaves comments out:**

  ```rust
  /// Replace the session's table. `Table::save` writes atomically, so a
  /// reader sees the old table or the new one, never a mix. `table.json`
  /// never holds comments (a session keeps them in `comments.jsonl`), so
  /// `table.comments` is left out.
  pub fn write_table(&self, table: &Table) -> anyhow::Result<()> {
      let without_comments = Table { comments: Vec::new(), ..table.clone() };
      without_comments.save(&self.dir.join(TABLE_FILE))
  }
  ```

  The rule then lives where `table.json` is written, so `create` doesn't need to know about it and neither will `push`. `create` keeps taking `&Table`, so none of its 25 test call sites change.

  The copy happens even when there are no comments to leave out. A branch to skip it would be one more case to read, to save a copy on `create` and `push`, both of which also serialize the whole table and flush it to disk.
- **`Summary` gains `awaiting_reply: usize`**: in `summary()`, `comments::threads(&self.read_log()?.events, &table)` filtered on `awaiting_reply` and counted. Add a one-line comment saying `ls` shows counts and leaves skipped-line warnings to the viewer and `tv save`. A log that can't be read makes `summary` fail, and `ls` then shows the placeholder row. That only happens on an IO error.

### `table.rs`

- `Table.comments` and `RawTable.comments` become `Vec<Thread>` (`use crate::comments::Thread`). The doc comment replaces "opaque until ADI-223":

  ```rust
  /// The file format's `comments`: read from a saved file so `create` can
  /// move them into the session's log, and filled from the log when saving.
  /// Empty in a session's own `table.json`.
  ```

- Comments that aren't threads now fail to parse. For example, a thread without `messages` makes `Table::parse` fail with serde's `missing field `messages``. That's validation the opaque field never did. No migration is needed: nothing has ever written comments, because `tv save` only passed through what a file brought with it.
- Update `save_keeps_comments` to use a real thread. Its point still holds: `save` writes a table's comments.

### `tv save`, `:w`, `tv ls`

- **`tv save <id> <path>`** attaches the threads before saving:

  ```rust
  let session = sessions.open(id)?;
  let mut table = session.load_table()?;
  let log = session.read_log()?;
  table.comments = comments::threads(&log.events, &table);
  table.save(&files::expand_tilde(path))?;
  println!("wrote {} rows to {path}", table.rows.len());
  if let Some(warning) = log.warning() {
      println!("{COMMENTS_FILE}: {warning}");
  }
  ```

  The `wrote …` line doesn't change. A skipped line gets a second line on stdout, next to the rest of `tv save`'s output. Nothing in `main.rs` prints to stderr except errors, and this isn't one.
- **`:w` in the viewer** writes the threads the viewer has (below): `Table { comments: self.threads.clone(), ..self.table.clone() }.save(&target)`. That's one copy of the table per save, which a user's keypress can afford.
- **`tv ls`** gets an AWAITING column between VIEWER and MODIFIED. It shows the count, or `-` for 0 (like VIEWER's `-`), so a session that needs you stands out. The placeholder row for a broken `table.json` shows `?`, like its ROWS×COLS. The count includes orphaned threads that are awaiting a reply: they still are.

  ```
  ID      ROWS×COLS  VIEWER  AWAITING  MODIFIED              TITLE
  k3f9x2  40×9       open    2         2026-10-29T10:02:11Z  Open pull requests
  m8q1zz  3×2        -       -         2026-10-27T08:15:40Z  (untitled)
  ```

  This is the "open comment count" from the spec's `tv ls` row, and the "awaiting reply" column the ADI-222 carry-over asked for: a thread is open exactly when it's awaiting a reply.

### The viewer reads the log

The ADI-221 carry-over in ADI-223: poll `comments.jsonl` in `SessionSync::tick`, next to `table.json`. It belongs in Part 1 because `:w` needs the threads.

**`App`** keeps the log and the threads folded from it:

```rust
/// The session's comment log as of the last poll, and the threads folded
/// from it against `table`. `refold` keeps `threads` in step with both.
events: Vec<Event>,
threads: Vec<Thread>,

/// Replace the comment log after a poll, and refold.
pub fn set_events(&mut self, events: Vec<Event>);

/// `threads = comments::threads(&events, &table)`. `set_events` and
/// `set_table` both call it: a reload can orphan a thread or change its
/// `value`.
fn refold(&mut self);
```

`set_table` calls `refold()` after it replaces the table. Folding is pure, so `App` stays all state and no IO, and it can't hold threads that are stale for its table.

**`SessionSync`** gets `comments_seen: Option<Fingerprint>`, which `start` sets to `None`, and a twin of `changed_table`:

```rust
/// The log if `comments.jsonl` changed since the last look, `None` if not.
/// No log is an empty one. A failed read isn't recorded as seen, so the
/// next tick tries again: nothing in the log can fail to parse (bad lines
/// are skipped), so an error here is IO, and it may clear up.
fn changed_comments(&mut self) -> anyhow::Result<Option<Log>> {
    let fingerprint = self.session.comments_fingerprint()?;
    if fingerprint == self.comments_seen {
        return Ok(None);
    }
    let log = self.session.read_log()?;
    self.comments_seen = fingerprint;
    Ok(Some(log))
}
```

- The fingerprint is still read before the file. An append that lands between the two is in the events but not in `comments_seen`, so the next tick reads the log again. That's harmless, and nothing gets missed.
- `None` means "no log". At start there may be none, and then `None == None` reads nothing, which is right because `App` starts with no events. If a log exists, `Some(_) != None` reads it on the first tick.
- If the log disappears (`tv rm --force`), the fingerprint goes back to `None`, `read_log` returns an empty log, and the markers go away. The table stays, with `reload failed` in the status line, as plan 06 set up.
- **`tick` becomes a list of steps.** Today it returns early when the cursor hasn't moved (`if position == self.written { …; return; }`). A poll added at its end would then be skipped on every tick where the cursor stays put. So each step gets its own method, and `tick` just calls them in order:

  ```rust
  /// Called once per event-loop iteration, before drawing. A reload can
  /// move the cursor (its row was deleted), so the polls come before the
  /// `view.json` write. ADI-228's `goto.json` check joins this list.
  pub fn tick(&mut self, app: &mut App, now: Instant) {
      self.poll_table(app, now);
      self.poll_comments(app);
      self.write_view_if_moved(app, now);
  }
  ```

  - `poll_table` is today's `match self.changed_table() { … }`, moved.
  - `write_view_if_moved` is today's `view.json` half, from `let position = Position::of(app);` to the end, moved. Its early return now only ends that step.
  - `poll_comments` is new:

  ```rust
  /// Load `comments.jsonl` into `app` if it changed, saying so if lines were
  /// skipped or it couldn't be read.
  fn poll_comments(&mut self, app: &mut App) {
      match self.changed_comments() {
          Ok(None) => {}
          Ok(Some(log)) => {
              if let Some(warning) = log.warning() {
                  app.set_message(format!("{COMMENTS_FILE}: {warning}"));
              }
              app.set_events(log.events);
          }
          Err(error) => app.set_message(file_error(COMMENTS_FILE, &error)),
      }
  }
  ```

  A persistent error sets the message again on every tick, so it stays up as long as the problem lasts and fades 3 seconds after. Errors don't go in `reload_error`: that one belongs to `table.json`, and `set_table` clears it.
- Update the module doc (`comments.jsonl` is in now; `goto.json` is still to come) and the `SESSION_FILES`-adjacent comments that say "later".

### Tests

**Compare threads with what's in a file as JSON:** `serde_json::to_value(&threads)` on both sides. A file holds JSON, so that's the comparison that matters. It also leaves `cell` out the same way the file does. A direct `assert_eq!` between a parsed thread and a folded one would pass in commit 1 and then fail in commit 2, when `cell` arrives.

- **`comments.rs`:**
  - `parse_log(...).events` then `log_text` gives the spec's three lines back exactly; compare the strings. This pins the field order and the omitted `None` fields. `warning()` is `None`.
  - `parse_log` skips `not json`, an unknown `op`, and a cut-off last line. It keeps the rest in order and records those lines' 1-based numbers; a blank line isn't recorded. `warning()` reads `skipped lines that aren't events: 2, 6`.
  - Folding:
    - A comment and its reply make one thread with two messages. A reply logged before its comment still attaches.
    - A `delete` of the top-level comment removes the thread and its replies. A `delete` of a reply removes only that message. A `delete` logged before its `add` still deletes it.
    - A reply to an unknown ID, a deleted thread, or another reply is dropped.
    - An `add` with no `reply_to` and only `row_id` starts no thread.
    - A duplicate ID keeps the first `add` in the log, even when the second is a reply with the same ID.
  - `awaiting_reply`: true when the user wrote last, false when the agent did.
  - `orphaned`: true when the row is gone, and when the column is gone.
  - `value`:
    - It's the cell's current value, a number staying a number.
    - It's `Value::Null` when orphaned, and when the row exists but has no such field.
    - A thread folded on a null cell, written as JSON and read back, is equal to itself, `value` included.
  - Unkeyed tables: anchors are `#2` and follow position.
  - `import` then `threads` gives back the same threads (the probe test). `import` gives the top-level comment `thread.id` and the replies `reply_to: thread.id`, skips a thread with no messages, and ignores the derived fields.
  - A `Thread` without `value`, `awaiting_reply` or `orphaned` deserializes, with defaults.
- **`table.rs`:**
  - A file with a thread in `comments` parses into `Table.comments`.
  - A thread missing `messages` fails, with `messages` in the error.
  - `save_keeps_comments` uses a real thread.
  - `row_indices` maps a keyed table's key values, and an unkeyed table's `#0`, `#1`, to their indices. The `diff.rs` tests keep covering `changed_cells` on top of it.
- **`session.rs`:**
  - `create` from a table with two threads:
    - `read_log()?.events` equals `comments::import` of them.
    - The `table.json` text has no `"comments"`.
    - `comments::threads` over that log and the loaded table gives the threads back, with their derived fields worked out. Compare as JSON.
  - `create` from a table without comments leaves no `comments.jsonl`: the path doesn't exist.
  - `write_table` of a table with comments leaves them out of the file.
  - `read_log`:
    - It's empty with no log.
    - Two `append_events` calls come back as one list, in order.
    - `append_events(&[])` creates no file.
    - A log with an invalid UTF-8 byte inside one line still gives the other lines.
  - `append_events` on a session removed through a second handle fails with `NotFound`, and the directory doesn't come back.
  - `comments_fingerprint` is `None` with no log, `Some` after an append, and different after a second append.
  - `summary().awaiting_reply` counts a user-last thread and not an agent-last one.
  - `remove` deletes a session that has a `comments.jsonl`, as the `view.json` test does.
- **`viewer/app.rs`:**
  - `set_events` folds.
  - `set_table` to a table without the commented row refolds, and the thread is orphaned.
  - `:w` writes the threads with their derived fields. The saved file's `comments` equals `serde_json::to_value(app.threads)`.
- **`viewer/sync.rs`**, using the fixture:
  - Events appended through a second handle (the agent's side) are in `app` after the next `tick`.
  - With no log, `tick` reads nothing and sets no message.
  - An append from the agent's side shows up even on a tick where the cursor hasn't moved since the last `view.json` write. That's the case `write_view_if_moved`'s early return would otherwise skip.
  - A log with a `not json` line between two good ones: `tick` loads both good events and sets the message `comments.jsonl: skipped lines that aren't events: 2`.
  - Unix only: a `comments.jsonl` that's a directory.
    - The fingerprint works, and `fs::read` fails with `Is a directory (os error 21)` (checked on macOS).
    - So `tick` sets the message `comments.jsonl: Is a directory (os error 21)`, and a second `tick` sets it again (retried).
    - Then `fs::rename` the directory out of the session (into the `TempDir`) and append a real event. The next `tick` loads it.
- **`main.rs` / `tests/cli.rs`:**
  - Add a `write_comments(dir, lines)` helper that writes `comments.jsonl` by hand, like `write_view`.
  - `tv ls`:
    - The header now starts `ID      ROWS×COLS  VIEWER  AWAITING  MODIFIED`; update `ls_shows_which_viewers_are_open`.
    - A session with a user-last thread shows `1`, one with an agent-last thread shows `-`, and a broken `table.json` with a log shows `?`.
  - `tv save` of a session with a log writes `comments` with `value`, `awaiting_reply` and `orphaned` filled in. `tv save` of a session without one writes no `comments` key.
  - `tv save` of a session whose log has a bad line prints `wrote … rows to …` and then `comments.jsonl: skipped lines that aren't events: 2`.

Opening a file can't be tested from `tests/cli.rs`, because it starts the viewer, which needs a terminal. The `session.rs` tests above cover the import, and the manual check covers the whole path.

## Part 2: comments in the viewer (ADI-224)

### Keys

| Where | Key | Does |
|---|---|---|
| Grid | `c` | Comment on the cursor cell: the status line becomes `comment: ` with the cursor after it. `Enter` sends, `Esc` cancels. If the cell has a thread, it's a reply to the newest one. |
| Grid | `]` / `[` | Next / previous cell with a thread, in reading order: by row, then left to right on screen (the pinned key column first). Wraps around. With none: `no comments`. |
| Cell popup | `c` | Close the popup and comment, as `c` in the grid does. After reading a thread, answering it is the obvious next step. |
| Cell popup | `d` | Asks `delete this cell's comments? y/n` in the status line. `y` deletes every thread on the cell; any other key goes back to the popup. With none: `no comments on this cell`. |
| Cell popup | `Esc` `Enter` `q` | Close, as now. |

- `c` on a table with no rows or no columns shows the same `no rows` / `no columns` message as `Enter` and `r`. Pull that choice out of `open_cell_view` into `fn no_cell_message(&self) -> &'static str` so the two can't drift.
- The comment is for the cell the cursor was on when you pressed `c`. `CommentInput` records that cell by row ID and column name, so a reload while you type can't move the comment. If that row is gone by the time you press `Enter`, the comment is saved anyway and shows up as orphaned. That's better than throwing away what you typed.
- Whether it's a reply is decided on `Enter`, not on `c`. If the agent starts a thread on the cell while you're typing, yours joins it.
- **Several threads on one cell.** The viewer always replies to an existing thread, but the agent's `tv agent comment` (ADI-227) or an imported file can start a second one. The spec talks about "the cell's thread" as if there's always one. Here, every thread on the cell shows in the popup, `c` replies to the newest, and `d` deletes all of them after one `y`. That hides nothing.
- `d` records the threads' IDs when you press it. `y` deletes exactly those, even if a reload landed in between.

### `App`

```rust
pub enum Mode {
    Grid,
    Help,
    Command(CommandLine),
    Detail,
    Record,
    /// Typing a comment.
    Comment(CommentInput),
    /// The cell popup, asking whether to delete these threads.
    ConfirmDelete(Vec<String>),
}

/// A comment being typed, and the cell it's for: named by row ID and column,
/// so a reload while typing can't move it to another cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentInput {
    pub line: CommandLine,
    row_id: String,
    column: String,
}

/// How a cell's comment marker is colored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    Awaiting, // some thread on the cell is awaiting a reply
    Replied,  // the agent replied last on every one
}
```

`Thread` gains `cell` (shown in Part 1's struct), and `threads` stores the `cell` it already works out.

New fields:
- `thread_cells: HashMap<Cursor, Vec<usize>>`: the threads on each cell, as indices into `threads`. `refold` builds it from each thread's `cell`, which `comments::threads` already worked out. Orphaned threads have none, so they're left out without a check of their own.
- `pending: Vec<Event>`: events for `SessionSync` to write.

New methods:
- `pub fn threads(&self) -> &[Thread]`, for the status-line counts.
- `pub fn marker(&self, row, col) -> Option<Marker>`. It's `Awaiting` if any thread on the cell is awaiting a reply.
- `pub fn cell_threads(&self) -> Vec<&Thread>`: the cursor cell's threads, in log order, for the popup.
- `pub(super) fn take_pending(&mut self) -> Vec<Event>`: `std::mem::take`.

There's one rule for where a thread's cell is, in `comments::threads`. The marker drawn in the grid and the `orphaned` flag the agent reads can't disagree, because both come from `Thread.cell`.

- `Action::{Comment, NextComment, PreviousComment}` and three `GRID_BINDINGS` rows after `r`: `c` "comment on this cell", `]` "next comment", `[` "previous comment".
- `DetailAction::{Close, Comment, Delete}` and a `DETAIL_BINDINGS` table, replacing the `matches!(key.code, …)` in the `Mode::Detail` arm:
  - Close (`Esc`, `Enter`, `q`): "close".
  - Comment (`c`): "comment on this cell". It calls the same `open_comment` as the grid's `Action::Comment`, and `Comment` mode returns to the grid as it does from there.
  - Delete (`d`): "delete this cell's comments".

  The help overlay lists the table, like the others.
- **The line editing moves into `CommandLine::edit(&mut self, key: &KeyEvent)`.** That's Backspace, Left, Right, Home, End, Ctrl-u and characters, the arms now inside `Mode::Command`. Both `Command` and `Comment` call it for any key other than `Enter` and `Esc`, so the two input lines can't start behaving differently.
- **Sending:** trim the text, and do nothing if it's empty. Otherwise draw `id = comments::new_id(self.events.iter().chain(&self.pending))` and look for the newest thread on the cell: `self.threads.iter().rev().find(|thread| thread.row_id == input.row_id && thread.column == input.column)`.
  - If there is one, push `Event::reply(id, Author::User, thread.id, text)`.
  - If not, push `Event::comment(id, Author::User, row_id, column, text)`.
  - Either way, `mode = Grid`.
- **`y` in ConfirmDelete** pushes one `Event::delete(id)` per ID and goes back to `Detail`. `SessionSync` writes them all in one append. Any other key goes back to `Detail`. (Ctrl-c still quits from everywhere.)
- **`set_table`'s check** that closes the cell popup when there's no cell becomes `matches!(self.mode, Mode::Detail | Mode::Record | Mode::ConfirmDelete(_))`. `Comment` mode doesn't need a cell: it knows its own.
- **`]` / `[`:** collect `(row, screen position, col)` for each key of `thread_cells` and sort it. Screen position comes from `column_order`, so the pinned key column comes first, as `cursor_position` does. Find the first entry after `(cursor.row, cursor_position())`, or the last one before it for `[`, and wrap to the other end if there's none. Then `move_to`.

`comments.rs` gains constructors (they set `ts` to now with `time::rfc3339`) and the ID draw:

```rust
/// Characters after the `c` in a comment ID: `c7h2k1`.
const COMMENT_ID_LEN: usize = 5;

impl Event {
    pub fn comment(id: String, author: Author, row_id: String, column: String, text: String) -> Event;
    pub fn reply(id: String, author: Author, thread: String, text: String) -> Event;
    pub fn delete(id: String) -> Event;

    /// The ID it adds or deletes.
    pub fn id(&self) -> &str;
}

/// A new comment ID: `c` and `COMMENT_ID_LEN` characters from the
/// session-ID alphabet, drawn again if one of `taken` has it. 36^5 is about
/// 60 million, so a redraw is rare, but a repeated ID would silently drop
/// the later comment (rule 2), so it's checked.
pub fn new_id<'a>(taken: impl IntoIterator<Item = &'a Event>) -> String {
    let taken: HashSet<&str> = taken.into_iter().map(Event::id).collect();
    loop {
        let id = format!("c{}", random_chars(COMMENT_ID_LEN));
        if !taken.contains(id.as_str()) {
            return id;
        }
    }
}
```

`session.rs`'s `random_id` becomes `pub fn random_chars(len: usize) -> String`, and `random_id()` calls `random_chars(ID_LEN)`. The doc comment says what a `u64` can hold: 12 characters from a 36-letter alphabet.

`new_id` takes the events to avoid as an iterator, so `App` passes both the log and what's still queued. That way it doesn't rely on the event loop writing `pending` before the next key; the loop does that today, and a future change to read several keys per frame would break that silently.

`time.rs`'s module doc now says `ls`, `view.json` and comments use it.

### `SessionSync` writes

A new first step in `tick`'s list, before the polls, so the new comment shows up on the same frame:

```rust
pub fn tick(&mut self, app: &mut App, now: Instant) {
    self.write_pending(app);
    self.poll_table(app, now);
    self.poll_comments(app);
    self.write_view_if_moved(app, now);
}

/// Write what the user did since the last tick. `poll_comments` then sees
/// the log grow and reads it back.
fn write_pending(&mut self, app: &mut App) {
    if let Err(error) = self.session.append_events(&app.take_pending()) {
        app.set_message(format!("not saved: {}", file_error(COMMENTS_FILE, &error)));
    }
}
```

- A failed append loses what was typed, and says so: `not saved: comments.jsonl: No such file or directory (os error 2)` after `tv rm --force`. Retyping is the fix. Keeping a failed comment around to retry would mean queueing it, and there's nowhere to keep it once the session is gone.
- Nothing is lost on `q`. The loop runs `tick` before every key it reads, so events from one key are written before the next key, `q` included, can quit.

### Rendering

- **`Theme`** gains `awaiting` and `replied`:
  ```rust
  // Theme::color(): the spec's colors. Named colors follow the user's
  // palette. Full strength on a dimmed null cell too.
  awaiting: Style::new().fg(Color::Yellow).remove_modifier(Modifier::DIM),
  replied: Style::new().fg(Color::Green).remove_modifier(Modifier::DIM),
  // Theme::mono(): heavy versus faint, each clearing the other.
  awaiting: Style::new().add_modifier(Modifier::BOLD).remove_modifier(Modifier::DIM),
  replied: Style::new().add_modifier(Modifier::DIM).remove_modifier(Modifier::BOLD),
  ```

  The marker is patched over the cell's style, and `Style::patch` applies `remove_modifier` too (`ratatui-core` 0.1.2, `style.rs`). The `remove_modifier`s are there for two cells where the marker would otherwise look wrong:
  - In mono, the cursor's row and column are bold (`cursor_line`). There a replied marker would come out bold and dim, which looks like neither. That's exactly where the user is looking.
  - A null cell is dim. There an awaiting marker would come out dim too, and a comment on an empty cell ("why is this blank?") is a likely case.

  Say both reasons in a comment.
- **The marker takes the cell's right padding.** The spec says "`▸` in the cell's last character", and the last character of a cell's span is its right padding. A marker there never hides text and doesn't change a column's width. (Its layout drawing shows `│    340▸│`.)
  - `grid_line`'s closure returns a struct instead of a tuple: `struct GridCell { text: String, align: Align, style: Style, marker: Option<Style> }`.
  - Each column becomes `[left padding + fitted text, right part, separator]`. The right part is `Span::styled(MARKER, style.patch(marker))` when there's a marker, and the padding in `style` otherwise. `const MARKER: &str = "▸";` sits next to `STATUS_HINTS` in `render.rs`. This assumes `CELL_PADDING` is 1; say so in a comment.
  - The header passes `marker: None`.
  - The buffer comes out the same for cells without a marker, so the grid snapshots don't change.
  - One existing snapshot does change: `help.txt`, below. Expect that diff, and read it.
- **Update `cell_style`'s doc comment.** It says "Comment markers get added here", but they're drawn in `grid_line`.
  - `body_lines` maps `app.marker(row, col)` to `theme.awaiting` or `theme.replied`. On the cursor cell the marker is patched over the reversed style, so it stays visible. Under a change highlight it's patched over the highlight.
- **The status line** puts the counts after the position, leaving out a zero, and the hints or the message after them:

  ```
   row 2/40  col 2/9  ·  1 awaiting reply  ·  1 orphaned  ·  ? help  : command  q quit
  ```

  A `fn comment_counts(app) -> Vec<String>` builds the middle parts, and the line is the parts joined with `  ·  `. This applies in the grid and the record view.
- **`draw_status`** is the one place that decides what the status row shows in each mode:
  - `Command` and `Comment`: an input line. Both are drawn the same way: the prefix (`:` or `comment: `), the text, and the terminal cursor after the text before the cursor. Generalize the existing `Mode::Command` code with a `(prefix, &CommandLine)` pair; don't copy it.
  - **The input line scrolls.** Today the line is drawn from its first character and the cursor stops at the right edge. That's fine for `:w out.json`, but comments are sentences: on 80 columns there's room for about 68 characters after `comment: `, and after that you'd be typing blind.
    - If the prefix, the text before the cursor, and one cell for the cursor don't fit, drop characters from the front of the text until they do.
    - Count in display width (`layout::display_width`), not characters.
    - Draw the text from there, and put the cursor after the part before it.
    - `:` gets the same scrolling, since it's the same code.
    - Go no further: no selection, no multi-line input.
  - `ConfirmDelete`: ` delete this cell's comments? y/n`.
  - Everything else: `status_line`, which stays the position, counts and hints, and knows nothing about prompts.
- **`draw_detail`** (also for `ConfirmDelete`, which `draw`'s match gains):
  - With no threads, the comments area is the two lines it is now.
  - With threads, the value and the comments split the popup: `Constraint::Fill(1)` each.
  - The heading is `Comments  ·  d delete`.
  - Each message is `author` in bold, two spaces, then the text. A multi-line message puts its first line after the author and the rest underneath. Split on `\n` yourself: ratatui drops control characters inside a `Line`, including `\n`, so the words would run together. Tabs become four spaces, as in the value.
  - A blank line goes between threads.
  - The paragraph wraps. A thread longer than its half of the popup is cut off; scrolling is out of scope.
  - Control characters other than `\n` and tab need no handling: ratatui drops them (`ratatui-core` `buffer.rs` and `span.rs` filter `char::is_control`), so comment text can't send escape sequences to the terminal.
- **`draw_help`** gets a "Cell detail" section, `binding_rows(DETAIL_BINDINGS)`, between Grid and Record view.
  - That makes the overlay 42 rows: 19 grid keys, 3 detail keys, 5 record keys, 4 commands, 4 headings, 3 blank lines between sections, the footer and its blank line, and the border.
  - `help_overlay` renders at 60×36 today, and `popup_area` shrinks the box to fit, so the end of the list would be cut off without any test failing. Render it at 60×44.
  - On a real 80×24 terminal the overlay was already cut off before this plan. Making it scroll is a separate change.

### Tests

- **`comments.rs`:**
  - `new_id(&[])` is `c` plus `COMMENT_ID_LEN` alphabet characters.
  - `new_id(log.iter().chain(&pending))`, with known IDs in both, returns none of them. (A wrong retry condition would loop forever on an empty log, so the first test catches that too.)
  - `Event::id` is the added or deleted ID.
  - `cell` is the (row, column) indices of the thread's cell. It's `None`, matching `orphaned`, when the row is gone and when the column is gone. A serialized `Thread` has no `cell`.
  - The constructors set the fields they're given, and `ts` parses as `rfc3339` output (20 characters, ending in `Z`).
- **`session.rs`:** `random_chars(5)` has 5 alphabet characters. The existing ID tests cover `random_id`.
- **`viewer/app.rs`:**
  - `c` on a cell without a thread, typing `hi`, then `Enter` leaves one pending event: an `Add` from the user, with that cell's `row_id` and column and no `reply_to`. The mode is `Grid`.
    - On a cell with two threads, the event replies to the newer one.
    - `Esc`, or `Enter` with only spaces typed, leaves nothing pending.
    - On an empty table, `c` shows `no rows`.
  - `set_table` while typing, with the row moved to a new index, doesn't change the anchor: the event still names the original row ID.
  - Editing works the same in both input lines: one test drives `CommandLine::edit` through Backspace, Left, Home, End and Ctrl-u. The existing `:` tests keep covering the `Command` side.
  - In Detail, `c` goes to `Comment` mode for the cursor cell, as `c` in the grid does.
  - In Detail, `d` on a cell with threads goes to `ConfirmDelete` with their IDs. `y` then leaves one `Delete` per ID pending and goes back to `Detail`; `n` leaves nothing and goes back to `Detail`.
    - `d` on a cell without threads says `no comments on this cell` and stays in `Detail`.
  - `set_table` to an empty table in `ConfirmDelete` goes to `Grid`.
  - `]` and `[` go in reading order:
    - Use a keyed table with threads at (row 0, a column right of the key), (row 0, the key column) and (row 3, …). The key column comes first on its row.
    - Both directions wrap.
    - An orphaned thread is never visited.
    - With no threads: `no comments`.
  - `marker`:
    - `None` without a thread.
    - `Replied` when the agent replied last.
    - `Awaiting` when the user wrote last.
    - `Awaiting` when one of two threads on the cell is awaiting.
  - `take_pending` empties the queue.
  - Every `DETAIL_BINDINGS` entry has help text, as the record test checks.
- **`viewer/sync.rs`:**
  - After `c`, `hi` and `Enter` on the fixture's app, one `tick` writes the event to `comments.jsonl` and puts the thread in `app.threads()`, with its marker on the cell.
  - After the session is removed through a second handle, the same steps set a message starting with `not saved: comments.jsonl: No such file`, and the directory doesn't come back.
- **`viewer/render.rs`** snapshots (`buffer_view` records styles, so these pin the colors):
  - `comment_markers`: `prs()` at 100×8 with three threads:
    - an awaiting one on 404's title
    - a replied one on 409's author
    - an orphaned one whose last message is the agent's, so only one thread is awaiting

    Check that:
    - The markers are in the right padding, yellow and green.
    - The status line shows `1 awaiting reply  ·  1 orphaned`.
  - `comment_markers_mono`: the same with `Theme::mono()`. The cursor starts on row 0, which is 409's row, so the replied marker there sits on the bold cursor line. Check that it isn't bold: assert on the buffer cell's modifiers, not only the snapshot.
  - Use a small inline table with an awaiting thread on a null cell. Check that the marker isn't dim. (`prs.json`'s first null is in row 7, which isn't on screen at 100×8.)
  - `comment_detail`: the popup on 404's title, with a two-message thread and the `d delete` heading.
  - `comment_input`: press `c`, `h`, `i` (the way `command_line_mid_edit` types its command); the status line reads `comment: hi`. `CommentInput`'s fields are private to `app.rs`, so the test can't build the mode directly.
  - The input line scrolls: type 100 characters into the comment line at width 50. The last characters typed are on screen, and the cursor (`assert_cursor_position`) is inside the status area.
  - `comment_delete_prompt`: `ConfirmDelete`; the status line asks.
  - `help`: re-recorded at 60×44 (above). The diff adds the three grid rows and the Cell detail section, and the end of the Commands section and `Press any key to close` are still there.
  - A test that a `ConfirmDelete` app reloaded with an empty table renders without panicking.
  - `tiny_terminal_does_not_panic` also renders `Comment` mode (press `c`) and `ConfirmDelete` (built directly: its IDs are public) at each size, so the new prompt and popup code runs at 0×0 and 1×1.

## Manual check

In a scratch directory, with a throwaway home. The file gets three threads: one awaiting a reply on PR 404's title, one the agent answered on 409's author, and one on a row that doesn't exist.

```sh
export TV_HOME=$(mktemp -d)
cp ~/tv/examples/prs.json .
python3 - <<'EOF'
import json
t = json.load(open("prs.json"))
def msg(id, author, text): return {"id": id, "author": author, "text": text, "ts": "2026-10-29T10:00:00Z"}
t["comments"] = [
    {"id": "cuser1", "row_id": "404", "column": "title", "messages": [msg("cuser1", "user", "Why is this still open?")]},
    {"id": "cuser2", "row_id": "409", "column": "author", "messages": [
        msg("cuser2", "user", "Who reviews this?"), msg("cagnt1", "agent", "Sam, tomorrow.")]},
    {"id": "cgone1", "row_id": "1", "column": "title", "messages": [msg("cgone1", "user", "Row 1 isn't a PR")]},
]
json.dump(t, open("prs-comments.json", "w"), indent=2)
EOF
tv prs-comments.json   # note the ID; leave it open
```

In a second terminal, in the same directory, with the same `TV_HOME` (step 8 compares files in it):

1. The viewer shows a yellow `▸` at the right edge of 404's title and a green one on 409's author. The status line says `2 awaiting reply  ·  1 orphaned`: the orphaned thread's last message is the user's, so it's awaiting too. `tv ls` shows `2` under AWAITING. `cat $TV_HOME/sessions/<id>/comments.jsonl` shows four `add` lines, and `table.json` has no `comments`.
2. From the top-left, `]` goes to 409's author, `]` again to 404's title, and `]` again wraps to 409's author. `[` goes back.
3. On 404's title, `Enter` shows the title, then `Comments  ·  d delete` and `user  Why is this still open?`. `Esc` closes it.
4. `c`, type `ping`, `Enter`. The marker stays yellow (you wrote last), and the log's last line is a reply with `"reply_to":"cuser1"`.
5. Act as the agent: `printf '%s\n' '{"op":"add","id":"cagnt2","author":"agent","reply_to":"cuser1","text":"Looking now.","ts":"2026-10-29T10:10:00Z"}' >> $TV_HOME/sessions/<id>/comments.jsonl`. Within a quarter second the marker turns green and the status line drops to `1 awaiting reply  ·  1 orphaned`. `tv ls` shows `1`.
   - Then `echo 'not json' >> …/comments.jsonl`. The status line shows `comments.jsonl: skipped lines that aren't events: 7` for 3 seconds, and the markers stay.
   - Remove that line by hand (an editor, or `sed -i '' '$d' …/comments.jsonl` on macOS) before going on.
6. `c` on 404's author, type `new thread`, `Enter`: a yellow marker appears. `Enter`, `d`: the status line asks `delete this cell's comments? y/n`. Press `n`; the thread is still in the popup. Press `d`, then `y`; the comments area says `none yet`. `Esc`: the marker is gone, and the log ends with a `delete` line.
7. Push a table without PR 404. Rewrite `table.json` atomically with `python3`, the way plan 06's check did (read, drop the row, write a temp file, `os.replace`). The 404 thread's marker goes, and the status line counts `2 orphaned`. Copy `prs.json`'s rows back the same way, and the marker returns.
8. `:w out.json`. In the other terminal, `tv save <id> out2.json`, then `cmp out.json out2.json`: the files are identical. `python3 -c 'import json; print(json.load(open("out.json"))["comments"][0])'` shows `value`, `awaiting_reply` and `orphaned` filled in.
9. `q`, then `tv out.json`. The new session shows the same markers and counts. `q`.
10. `NO_COLOR=1 tv <id>`: the markers are bold (awaiting) and dim (replied). `q`.
11. `tv rm <id>` for both sessions. They go, `comments.jsonl` included. `unset TV_HOME`.

## Acceptance

```sh
make check
cargo tree --locked -e normal --prefix none | sort -u | wc -l   # 89, unchanged
```

Plus the manual check above.

## Deliverables

- `src/comments.rs` (new):
  - Types: `Author`, `Event`, `Add`, `Thread`, `Message`, `Log` (with `warning`).
  - Functions: `parse_log`, `log_text`, `threads`, `import`.
  - Part 2: `Event::{comment, reply, delete, id}`, `new_id`, `COMMENT_ID_LEN`, `Thread.cell`.
- `src/table.rs`: `comments: Vec<Thread>` on `Table` and `RawTable`, with the new doc comment; `save_keeps_comments` uses a thread; `row_indices`.
- `src/viewer/diff.rs`: `changed_cells` uses `Table::row_indices`.
- `src/session.rs`:
  - `pub COMMENTS_FILE`, used in `SESSION_FILES`.
  - `Session::{read_log, append_events, comments_fingerprint}`, and the shared private `fingerprint(path)`.
  - `write_table` leaves comments out, and `create` imports first. The comments about `create` are updated.
  - `Summary.awaiting_reply`.
  - `random_chars` (Part 2).
- `src/main.rs`: `mod comments`; the `ls` AWAITING column, with `?` on the placeholder row; `save` attaches threads and prints the log's warning.
- `src/viewer/sync.rs`:
  - `tick` as a list of steps: `poll_table` and `write_view_if_moved` (both moved from `tick`), and `poll_comments` (new), with `comments_seen` and `changed_comments`.
  - `write_pending` as the first step (Part 2).
  - Module doc updated.
- `src/viewer/app.rs`:
  - Part 1: `events`, `threads`, `set_events`, `refold` (also called from `set_table`), and `:w` writing threads.
  - Part 2: `Mode::{Comment, ConfirmDelete}`, `CommentInput`, `Marker`, `thread_cells`, `pending`, `threads()`, `marker`, `cell_threads`, `take_pending`.
  - Part 2: `Action::{Comment, NextComment, PreviousComment}` with three grid bindings, and `DetailAction::{Close, Comment, Delete}` with `DETAIL_BINDINGS`.
  - Part 2: `CommandLine::edit`, `no_cell_message`, and the `ConfirmDelete` arm in `set_table`'s check.
- `src/viewer/render.rs`:
  - `GridCell`, `MARKER`, and the marker in `grid_line`.
  - The status-line counts. `draw_status` maps each mode to its status row: the two input lines (shared code), the delete prompt, or `status_line`.
  - `draw_detail` threads, and the "Cell detail" help section.
  - The input line scrolls.
  - `cell_style`'s doc comment.
  - Five new snapshots, and `help.txt` re-recorded at 60×44.
- `src/viewer/theme.rs`: `awaiting`, `replied`.
- `src/time.rs`: module doc.
- `tests/cli.rs`: `write_comments`, the AWAITING cases, the `save` cases, and the updated `ls` header.
- `docs/schema.md`: the `comments` row says what a thread is (the shape above), that it's imported when the file is opened, and that `value`, `awaiting_reply` and `orphaned` are worked out on save and ignored on import. Add a short "Comments" section with the example thread.
- `SPEC.md`:
  - "`comments.jsonl`":
    - Lines that don't parse are skipped, and the viewer and `tv save` say which.
    - A `delete` of a reply removes just that reply.
    - Replies attach whatever their place in the log.
    - A reply to anything but a thread is dropped.
    - The first of two equal IDs wins.
  - "Sessions": a session's `table.json` never holds comments.
  - "Human CLI": `tv ls` lists the number of threads awaiting a reply.
  - "Rendering": the marker is drawn in the cell's right padding. Without color, awaiting is bold and replied is dim.
  - "Keys":
    - `]`/`[` wrap around.
    - `d` asks for `y`.
    - With several threads on a cell, `c` replies to the newest and `d` deletes them all.
  - Module layout:
    - `sync.rs` also polls `comments.jsonl` and writes comments.
    - `time.rs` is used by comments now.
    - `comments.rs` is the event types, the log format, and folding. It no longer says "event log append/read": `session.rs` does that.
    - `session.rs`'s line mentions `comments.jsonl`.
    - `app.rs`'s list of modes gains Comment and ConfirmDelete.
- Two commits:
  1. `comments.jsonl: the event log and its threads; import on open, save, ls AWAITING, viewer poll (ADI-223)`. This is everything in Part 1. Every item has a non-test caller:
     - `import`, `append_events`: `create`.
     - `threads`: `summary`, `save` and `App::refold`.
     - `read_log`: `summary`, `save` and `SessionSync`.
     - `Log::warning`: `save` and `SessionSync`.
     - `comments_fingerprint`: `SessionSync`.
     - `row_indices`: `threads` and `diff::changed_cells`.
     - `App::threads`: `:w`.
     - `Author::Agent` and `Event::Delete`: serde.
  2. `comments in the viewer: c, markers, ]/[, the thread in the cell popup, d (ADI-224)`. This is everything in Part 2.

## Out of scope

- The agent's side, `tv agent comments`, `reply` and `comment`: ADI-227. Part 1's `Session` methods are what it calls.
- Editing a comment, or marking a thread resolved. The spec has no resolved state on purpose: a thread stops awaiting once the agent replies.
- Scrolling a long thread in the cell popup, and showing timestamps there. Add them if threads get long. The agent sees timestamps in `tv agent comments`.
- Compacting the log, which only ever grows. A busy session has a few hundred lines.
- Comments in the record view, highlighting comments that arrive, and multi-line comment input.
- Showing orphaned threads in the viewer beyond the count. They have no cell, and the agent gets them with `orphaned: true`.
