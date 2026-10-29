# Plan: ADI-202 the agent CLI

Linear: https://linear.app/aditya-bhargava/issue/ADI-202 (parent)
Sub-issues:
- ADI-225: `tv agent new`, `push`, `get`, `sessions`.
- ADI-226: `tv agent focus`.
- ADI-227: `tv agent comments`, `reply`, `comment`.
- ADI-228: `tv agent goto`, which writes `goto.json`, and the viewer applying it.
- ADI-229: CLI integration tests, including the new → push → comment → reply → save → reopen round trip.

Depends on: PR #8 merged (`8b84995`). This plan covers all of ADI-202, so merging it closes the parent. `tv agent help` and its guide are ADI-203.

## Goal

An agent can do everything the spec's "Agent CLI" section lists, with no viewer and no terminal:

- **Tables:**
  - `tv agent new` makes a session from a file or stdin and prints the command the user runs to open it.
  - `push` replaces a session's table, and an open viewer reloads it within a quarter second (plan 06).
  - `get` prints the table.
  - `sessions` lists sessions.
- **Focus:** `focus` says which cell the user's cursor is on, with its value and row. `goto` moves the cursor.
- **Comments:** `comments` lists the threads awaiting a reply, `reply` answers one, and `comment` starts a thread on a cell.

Every `tv agent` command prints one line of JSON on stdout and exits 0. An error is plain text on stderr with exit status 1.

## Context for the executing agent

- Read `SPEC.md`:
  - "Agent CLI": every subsection except the agent guide, which is ADI-203.
  - "Table format", "Sessions", "Live updates".
  - The `goto` bullet under "Changes after review".
- Build on what's there; don't duplicate it:
  - `session.rs`:
    - `Sessions::{create, open, list}`.
    - `Session::{load_table, write_table, read_view, viewer_open, read_log, append_events, summary, modified_at}`.
    - `SESSION_FILES`, which already lists `"goto.json"` so that `tv rm` accepts it.
    - `write_table` leaves `comments` out, and `create` imports a file's comments into the log first.
  - `comments.rs`: `threads`, `new_id`, `Event`, `Add`, `Message`, `Thread`, `Author`, and `Log::warning`.
  - `table.rs`:
    - `Table::parse` (the bare-array shorthand), `from_path`, `row_id` and `cell`.
    - `TryFrom<RawTable>`, which validates the key: missing or duplicate values are an error naming the rows.
  - `files::write_json`: atomic, pretty-printed JSON.
  - `viewer/sync.rs`: `tick` is a list of steps, and `goto.json` is one more (the module doc says so).
  - `viewer/app.rs`: `move_to`, `set_message`, `mode`, and `Mode`, which derives `PartialEq`.
  - `tests/cli.rs`: `tv`, `succeeds`, `write_session`, `write_view`, `open_viewer`, `write_comments`, `USER_COMMENT` and `assert_fails_with`. Every test there runs with `TV_HOME` set to a temp dir.
- **No new crates.** `std::io::IsTerminal` and `std::path::absolute` are std. `cargo tree` stays at 89.
- **Five code commits, one per sub-issue, each passing `make check`.**
  - `dead_code` fires in a binary crate, so everything a commit adds needs a non-test caller in that commit.
  - The split below keeps that true. Each part adds its own arms to `agent::run`'s match.
- **One new delete.** The viewer deletes `goto.json` after reading it (Part 4). It's the third `#[expect(clippy::disallowed_methods)]` in the crate, after `files::write_atomic` and `Sessions::remove`:
  - It deletes one file, by a name the code builds (`dir.join(GOTO_FILE)`), and nothing recursive. `dir` is always under the sessions root, because only `Sessions` makes a `Session`.
  - `clippy.toml`'s reason for `remove_file` names it.
  - Tests don't delete anything else.
- **Scratch check.** I compiled every snippet below in a scratch copy of `main`: all of `agent.rs`, `Table::parse_with`, the `Goto` IO, `App::go_to`, `poll_goto`, and the `Add` constructors. After the anti-pattern check, I compiled its changes again in a fresh copy: `Sessions::tv_home`, `Table::{row_index, column_index, from_path_with}`, the reworked `go_to`, and `ready_for_goto`.
  - It's clean under `cargo clippy --all-targets -- -D warnings` (pedantic), and the existing tests pass.
  - I ran each command by hand against a temp `TV_HOME`, and ran a `goto` test through `SessionSync::tick`.
  - Running `push` by hand is how decision 2 below was found.
  - One trap the compiler caught: `threads` maps `&&Add` items, so `.map(Message::from)` doesn't compile there. Write `.map(|&add| Message::from(add))`.

## Decisions

These go beyond what the spec and the Linear issues say. Each one is cheap, and SPEC.md is updated to match (Deliverables).

1. **Output is one line of compact JSON** (`serde_json::to_string`, then a newline), for every command.
   - Agents read stdout as tokens. For `examples/prs.json`, compact JSON is 30% smaller than pretty-printed JSON, and `jq` reads either.
   - The spec's examples are pretty-printed for reading, not as a format.
2. **`push` keeps the session's title and key when the input names neither.** `--title` and `--key` still override, and a full table with `"key": null` drops the key on purpose.
   - Without this, `gh pr list --json … | tv agent push <id>` sends a bare array, and the session silently loses its key. Duplicate keys are then accepted, and every comment's row ID stops matching: they all go orphaned.
   - I hit this by hand in the scratch copy: a push of `[{"id":1},{"id":1}]` succeeded.
3. **`push` refuses a table that has `comments`.**
   - `write_table` would drop them silently.
   - Importing them into a live log would mix two histories. `first add wins` would mostly hide the duplicates, but not reliably.
   - The error says to remove `comments`, or to start a new session from the file with `tv agent new`, which imports them the way `tv open file.json` does.
4. **`open_command` carries `TV_HOME` when it's set:** `TV_HOME='/abs/path' tv open k3f9x2`.
   - The agent gives this command to the user to run in their own shell, which may not have `TV_HOME` set. Without it, `tv open` looks in `~/.tv` and says `no session`.
   - The path is made absolute and single-quoted. Unset, the command is the spec's `tv open k3f9x2`.
   - `Sessions::from_home` already reads `TV_HOME`, so it keeps the absolute path too, and `open_command` asks `sessions.tv_home()`. `main` doesn't change.
5. **`sessions` lists a session whose `table.json` can't be read**, with `null` for `title`, `rows`, `columns` and `awaiting_reply`. That's where `tv ls` shows `?`, and like `ls`, one broken session doesn't hide the rest.
6. **`focus` and `goto` print `row: null` when the row isn't in the table.**
   - `view.json` may name a row that a push removed after the viewer closed.
   - `row_id` and `column` are still printed as recorded. `row` is an object whenever the row exists, so `null` there can't be confused with an empty cell.
7. **`reply` and `comment` print the new message:** `{id, author, text, ts}`, the shape of a message inside a thread.
   - The spec says this for `reply` and nothing for `comment`. For a new thread, the message's `id` is the thread's ID, which is what a later `reply` needs.
   - `reply` refuses an ID that isn't a thread, because `comments::threads` would silently drop the reply.
   - `comment` and `goto` refuse a row or column the table doesn't have, before writing anything.
8. **The viewer applies a `goto` only in the grid.**
   - In the cell popup, the record view, an input line or the help overlay, moving the cursor would change what the user is reading or typing about.
   - So `goto.json` waits, and applies on the first tick after they're back in the grid.
9. **A `goto` says so in the status line** (`the agent moved the cursor here`, for the usual 3 seconds), instead of flashing the cell as ADI-228 says.
   - The cursor cell is drawn reversed, the brightest thing on screen, and `cell_style` keeps highlights off it on purpose. A flash there wouldn't show.
   - The message also explains why the cursor jumped.
10. **`goto` works with no viewer open.** The file waits, and the next viewer applies it on its first tick: the agent can point at a cell before the user opens the table. The printed `viewer_open: false` tells the agent.
11. **Comment text is one argument.** Shell quoting is the agent's job; ADI-203's guide shows it.
    - `allow_hyphen_values` lets text like `-1 is wrong` through.
    - Text is trimmed, and empty text is refused, as in the viewer.
12. **`AgentCommand` lives in `agent.rs`, not `main.rs`.** Nine commands' clap definitions next to their code keep `main.rs` to one dispatch line. SPEC's module layout gets updated to match.

## What this plan defers, and to where

| Item | Lands in |
|---|---|
| `tv agent help` and the guide text | ADI-203. clap generates a `help` subcommand for `tv agent` today, which prints the command list. ADI-203's `Help` variant needs `#[command(disable_help_subcommand = true)]` on the `Agent` group. |
| What the guide must tell agents (from this plan's decisions) | ADI-203: <ul><li>Quote row IDs that start with `#`: `--row '#3'`, because an unquoted `#` starts a shell comment.</li><li>Comment text is one quoted argument.</li><li>`push` keeps the key, and `"key": null` drops it.</li><li>Output is compact JSON.</li><li>`goto` waits until the user is back in the grid.</li></ul> |
| `tv skills` | ADI-203 |
| Publishing | ADI-204 |

## Part 1: `new`, `push`, `get`, `sessions` (ADI-225)

### The command group

`main.rs` gains one subcommand, and the agent's commands live in `src/agent.rs` (new):

```rust
// main.rs, in `enum Command`
/// Commands for agents. Each prints JSON.
Agent {
    #[command(subcommand)]
    command: agent::AgentCommand,
},
```

```rust
// main.rs, `main`'s match
Some(Command::Agent { command }) => agent::run(&sessions, command),
```

`Sessions` keeps `TV_HOME` for `open_command` (decision 4). `from_home` is the one place that reads it, so that's where it's made absolute:

```rust
pub struct Sessions {
    root: PathBuf, // …/sessions
    /// `TV_HOME`, made absolute, if it was set. `tv agent`'s `open_command`
    /// repeats it, since the user's shell may not have it.
    tv_home: Option<PathBuf>,
}

impl Sessions {
    pub fn from_home(tv_home: Option<OsString>) -> anyhow::Result<Sessions> {
        // (the existing comment about an empty `TV_HOME=`)
        let tv_home = tv_home
            .filter(|home| !home.is_empty())
            .map(std::path::absolute)
            .transpose()
            .context("resolving TV_HOME")?;
        let home = match &tv_home {
            Some(home) => home.clone(),
            None => std::env::home_dir()
                .context("can't find your home directory; set TV_HOME")?
                .join(".tv"),
        };
        Ok(Sessions {
            tv_home,
            ..Sessions::new(home.join("sessions"))
        })
    }

    pub fn new(root: PathBuf) -> Sessions {
        Sessions {
            root,
            tv_home: None,
        }
    }

    /// `TV_HOME`, absolute, or `None` for the default `~/.tv`.
    pub fn tv_home(&self) -> Option<&Path> {
        self.tv_home.as_deref()
    }
}
```

- `from_home` builds through `Sessions::new`, which would otherwise have only test callers and fail `dead_code`. The scratch compile caught that.
- A relative `TV_HOME` now gives an absolute root. That's the same directory, since `tv` never changes its working directory.

`agent.rs` starts like this. The variants listed are Part 1's; each later part adds its own.

```rust
//! `tv agent`: the commands an agent runs. Each one prints one line of JSON
//! on stdout. Errors go to stderr through `main`, with exit status 1.

#[derive(Subcommand)]
pub enum AgentCommand {
    /// Create a session from FILE, or from JSON on stdin.
    New(TableInput),
    /// Replace a session's table. Same input as `new`.
    Push {
        id: String,
        #[command(flatten)]
        input: TableInput,
    },
    /// Print a session's table, without comments.
    Get { id: String },
    /// List sessions, most recently modified first.
    Sessions,
}

/// The table `new` and `push` read, and what to set on it.
#[derive(Args)]
pub struct TableInput {
    /// A JSON file: the table format, or a bare array of rows. Stdin without
    /// one, or with `-`.
    file: Option<String>,
    /// Set the title.
    #[arg(long)]
    title: Option<String>,
    /// Set the key column. Rows need unique, non-null values in it.
    #[arg(long)]
    key: Option<String>,
}

/// Run `command` and print its result.
pub fn run(sessions: &Sessions, command: &AgentCommand) -> anyhow::Result<()> {
    let json = match command {
        AgentCommand::New(input) => serde_json::to_string(&new(sessions, input)?)?,
        AgentCommand::Push { id, input } => serde_json::to_string(&push(sessions, id, input)?)?,
        AgentCommand::Get { id } => serde_json::to_string(&sessions.open(id)?.load_table()?)?,
        AgentCommand::Sessions => serde_json::to_string(&list_sessions(sessions)?)?,
    };
    println!("{json}");
    Ok(())
}
```

Each arm serializes its own result because `Serialize` can't be a trait object, so the arms can't return one shared type. Each arm is still one line saying which function to call, and printing happens once, below the match.

- **`get`** is one arm. A session's `table.json` never holds comments, so the loaded table serializes without them, with inferred columns and types written out. That's the full format, so the output is valid input for `push`.
- **Errors** come out the way the human commands' do: `main` returns the `anyhow` error, Rust prints `Error: …` and the `Caused by:` chain on stderr, and exits 1. A clap usage error exits 2, which is still non-zero, as the spec asks.

### Reading the input

```rust
impl TableInput {
    /// The table from the file or stdin, with `--title` and `--key` applied
    /// before it's validated, and `fill`'s title and key where the input
    /// names neither.
    fn read(&self, fill: Meta) -> anyhow::Result<Table> {
        let set = Meta {
            title: self.title.as_deref(),
            key: self.key.as_deref(),
        };
        match self.file.as_deref() {
            None | Some("-") => {
                Table::parse_with(&read_stdin()?, set, fill).context("parsing stdin")
            }
            Some(path) => Table::from_path_with(path, set, fill),
        }
    }
}

/// Stdin, unless it's a terminal: an agent that forgot the pipe would
/// otherwise wait forever for input nobody types.
fn read_stdin() -> anyhow::Result<String> {
    let stdin = io::stdin();
    if stdin.is_terminal() {
        bail!("no table: give a FILE, or pipe JSON to stdin");
    }
    io::read_to_string(stdin).context("reading stdin")
}
```

- No `~` expansion: the shell does that for an argument, as with `tv open file.json`. Both go through `from_path`'s code, so their `reading …` and `parsing …` errors read the same.
- `--key` has to be in place **before** validation, or a duplicate key would get through. So it goes into the JSON before serde runs `TryFrom<RawTable>`. In `table.rs`, `parse` and `from_path` become `parse_with` and `from_path_with` with nothing to set:

```rust
/// A table's `title` and `key`, as `tv agent new` and `push` set them.
#[derive(Debug, Default, Clone, Copy)]
pub struct Meta<'a> {
    pub title: Option<&'a str>,
    pub key: Option<&'a str>,
}

impl Table {
    /// Parse a table from JSON text, accepting either the full format or a bare
    /// array of row objects.
    pub fn parse(text: &str) -> anyhow::Result<Table> {
        Table::parse_with(text, Meta::default(), Meta::default())
    }

    /// `parse`, with `set` replacing the text's `title` and `key`, and `fill`
    /// supplying them where the text has neither a value nor `null`. Both
    /// apply before validation, so a key from either is checked like one in
    /// the file.
    pub fn parse_with(text: &str, set: Meta, fill: Meta) -> anyhow::Result<Table> {
        let mut value: Value = serde_json::from_str(text)?;
        if value.is_array() {
            let mut wrapper = Map::new();
            wrapper.insert("rows".to_string(), value);
            value = Value::Object(wrapper);
        }
        // `get` is `None` for a field the text doesn't have, and for a
        // scalar, which serde then rejects below.
        let title = set.title.or(fill.title.filter(|_| value.get("title").is_none()));
        let kept_key = fill.key.filter(|_| set.key.is_none() && value.get("key").is_none());
        let key = set.key.or(kept_key);
        if let Value::Object(fields) = &mut value {
            for (name, chosen) in [("title", title), ("key", key)] {
                if let Some(chosen) = chosen {
                    fields.insert(name.to_string(), chosen.into());
                }
            }
        }
        let table = serde_json::from_value(value);
        // A key the caller didn't ask for is puzzling in an error, so say
        // where it came from and how to drop it.
        match kept_key {
            Some(key) => table.with_context(|| {
                format!(
                    "kept the session's key \"{key}\", since the table names none; \
                     pass --key, or push the full format with \"key\": null to drop it"
                )
            }),
            None => Ok(table?),
        }
    }

    pub fn from_path(path: impl AsRef<Path>) -> anyhow::Result<Table> {
        Table::from_path_with(path, Meta::default(), Meta::default())
    }

    /// `from_path`, with `set` and `fill` applied as in `parse_with`.
    pub fn from_path_with(path: impl AsRef<Path>, set: Meta, fill: Meta) -> anyhow::Result<Table> {
        let path = path.as_ref();
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Table::parse_with(&text, set, fill).with_context(|| format!("parsing {}", path.display()))
    }
}
```

`table.rs` also gets two lookups by name. `Focus`, `check_cell` and `App::go_to` each find one row or one column, and without these each would write its own search:

```rust
    /// The index of the row whose `row_id` is `row_id`.
    pub fn row_index(&self, row_id: &str) -> Option<usize> {
        (0..self.rows.len()).find(|&index| self.row_id(index) == row_id)
    }

    /// The index of the column named `name`.
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|column| column.name == name)
    }
```

`row_index` goes through `row_id`, the one rule for row IDs, as `row_indices` does. `row_indices` builds a map, which is the right tool when you look up many rows, but not for one.

These arrive in Part 2 (`row_index`) and Part 3 (`column_index`) with their first callers.

- The check is "the text has the field", not "the field is non-null", so `"key": null` in the input wins over `fill`. That's how an agent drops a key on purpose (decision 2).
- When the key came from `fill`, a validation error says so. Otherwise a push of rows without a `number` column fails with `key column "number" is not one of the columns`, about a key the agent never mentioned. With the context, stderr reads:

  ```
  Error: parsing stdin

  Caused by:
      0: kept the session's key "number", since the table names none; pass --key, or push the full format with "key": null to drop it
      1: key column "number" is not one of the columns
  ```
- The message names the session, which is agent-specific for `table.rs`. That's fine: `Meta` exists for `tv agent new` and `push`, and its doc says so.

### `new` and `push`

```rust
/// What `new` and `push` print.
#[derive(Debug, Serialize)]
pub struct Created {
    session: String,
    open_command: String,
    rows: usize,
    columns: usize,
}

impl Created {
    fn of(sessions: &Sessions, session: &Session, table: &Table) -> Created {
        Created {
            session: session.id().to_string(),
            open_command: open_command(session.id(), sessions.tv_home()),
            rows: table.rows.len(),
            columns: table.columns.len(),
        }
    }
}

/// The command that opens session `id`, for the agent to give the user.
/// With `TV_HOME` set, the command sets it too: the user's shell may not
/// have it, and `tv open` would then look in `~/.tv`.
fn open_command(id: &str, tv_home: Option<&Path>) -> String {
    match tv_home {
        None => format!("tv open {id}"),
        Some(home) => format!(
            "TV_HOME={} tv open {id}",
            shell_quote(&home.to_string_lossy())
        ),
    }
}

/// `text` in single quotes, safe to paste into a POSIX shell. A `'` inside
/// ends the quote, adds an escaped `'`, and starts a new quote.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn new(sessions: &Sessions, input: &TableInput) -> anyhow::Result<Created> {
    let table = input.read(Meta::default())?;
    let session = sessions.create(&table)?;
    Ok(Created::of(sessions, &session, &table))
}

fn push(sessions: &Sessions, id: &str, input: &TableInput) -> anyhow::Result<Created> {
    let session = sessions.open(id)?;
    // Input that names no title or key keeps the session's: dropping the key
    // would orphan every comment. A `table.json` that can't be read has
    // nothing to keep, and replacing it is one reason to push.
    let current = session.load_table().ok();
    let keep = current.as_ref().map_or_else(Meta::default, |table| Meta {
        title: table.title.as_deref(),
        key: table.key.as_deref(),
    });
    let table = input.read(keep)?;
    if !table.comments.is_empty() {
        bail!(
            "push doesn't change comments: take `comments` out of the table, \
             or start a new session from it with `tv agent new`"
        );
    }
    session.write_table(&table)?;
    Ok(Created::of(sessions, &session, &table))
}
```

- **`new`** goes through `Sessions::create`, so a saved file's comments are imported, the same as with `tv open file.json`. A bad table fails before any directory is made.
- **`push`** opens the session before reading stdin, so an unknown ID fails without consuming the input. `write_table` is atomic, so an open viewer sees the old table or the new one, and plan 06's reload keeps its cursor on the same row ID.
- **A rejected `push`** writes nothing, so the table stays as it was. That covers missing or duplicate key values, a key that isn't a column, comments, and JSON that doesn't parse.

### `sessions`

```rust
/// One session in `tv agent sessions`. What comes from `table.json` is `null`
/// when it can't be read, where `tv ls` shows `?`.
#[derive(Debug, Serialize)]
pub struct Listed {
    session: String,
    title: Option<String>,
    rows: Option<usize>,
    columns: Option<usize>,
    viewer_open: bool,
    awaiting_reply: Option<usize>,
    modified_at: Option<String>,
}

fn list_sessions(sessions: &Sessions) -> anyhow::Result<Vec<Listed>> {
    let listed = sessions
        .list()?
        .iter()
        .map(|session| {
            let summary = session.summary().ok();
            Listed {
                session: session.id().to_string(),
                title: summary.as_ref().and_then(|summary| summary.title.clone()),
                rows: summary.as_ref().map(|summary| summary.rows),
                columns: summary.as_ref().map(|summary| summary.columns),
                viewer_open: session.viewer_open(),
                awaiting_reply: summary.as_ref().map(|summary| summary.awaiting_reply),
                modified_at: session.modified_at().ok().map(time::rfc3339),
            }
        })
        .collect();
    Ok(listed)
}
```

- The fields are declared in the spec's order, and serde writes them in declaration order.
- `awaiting_reply` is `Summary.awaiting_reply`, the number `tv ls` shows under AWAITING. Orphaned threads count too.
- No sessions, or no sessions directory yet, prints `[]`.

### Tests

- **`table.rs`:**
  - `parse_with` sets a title and a key on a bare array, and replaces a file's.
  - A `--key` naming a column with a repeated value fails, with the same `duplicate key value` message as a key in the file.
  - `fill` fills in a missing title and key, but not one the text has, `null` included.
  - `parse` and `from_path` behave as before; the existing tests cover them.
- **`session.rs`:** `from_home` with a relative `TV_HOME` gives an absolute `tv_home()` and a root under it. With `TV_HOME` unset or empty, `tv_home()` is `None`. Extend `from_home_appends_sessions`.
- **`agent.rs`:** `open_command` without `TV_HOME`, and with a path containing a space and a `'`: `TV_HOME='/tmp/it'\''s here' tv open k3f9x2`.
- **`tests/cli.rs`.** Add a helper that every successful agent call goes through, so every agent command's output is checked as one JSON line (part of ADI-229):

  ```rust
  /// Run `tv agent <args>`; assert it exited 0 and printed exactly one line,
  /// and return that line parsed as JSON.
  fn agent(home: &Path, args: &[&str], stdin: &str) -> serde_json::Value
  ```

  The cases:
  - `new` from stdin, with `--key number --title PRs` and `prs.json`'s rows as a bare array (build the array with `serde_json`). The output has `session`, `rows: 40`, `columns: 9`, and an `open_command` that is `TV_HOME='<home>' tv open <id>`. `tv ls` lists the session.
  - `new` from a file path, and from `-`.
  - `new` with nothing on stdin exits 1 (`parsing stdin`). The terminal case can't be tested without a terminal; see the manual check.
  - `new` of a saved file with comments: `ls` shows them under AWAITING. This is the same import `tv open` does.
  - `push` of a bare array keeps the key and title, and `get` shows both. Pushing a bare array with a repeated `number` then exits 1 with `duplicate key value` and `"key": null` on stderr, and `get` shows the table from before.
  - `push` with `"key": null` in a full table drops the key.
  - `push` with `--key` naming a missing column exits 1.
  - `push` of a table with `comments` exits 1, and the log is unchanged.
  - `push` to an unknown ID exits 1 with `no session`.
  - `push` over a `table.json` that doesn't parse succeeds, and `get` reads the new table.
  - `get` output parses back with `Table::parse` semantics: push it to a second session and `get` that; the two outputs are equal.
  - `sessions`:
    - With no sessions, `[]`.
    - With two sessions and a broken one (`write_session(…, "not json")`), three entries, the broken one with `null` rows.
    - `viewer_open` is true for a session with `open_viewer`.
    - `awaiting_reply` is 1 for a session with `USER_COMMENT` in its log.

## Part 2: `focus` (ADI-226)

```rust
/// What `focus` and `goto` print: the cell the user's cursor is on, or for
/// `goto` is being sent to, with its value and its row.
#[derive(Debug, PartialEq, Serialize)]
pub struct Focus {
    viewer_open: bool,
    row_id: Option<String>,
    column: Option<String>,
    /// `null` for an empty cell, or when the row or column isn't in the table.
    value: Value,
    /// `null` when the row isn't in the table: the viewer was last on a row
    /// that a push has since removed.
    row: Option<Row>,
}

impl Focus {
    fn at(table: &Table, viewer_open: bool, row_id: Option<String>, column: Option<String>) -> Focus {
        let index = row_id.as_deref().and_then(|row_id| table.row_index(row_id));
        let value = index
            .zip(column.as_deref())
            .and_then(|(index, column)| table.cell(index, column))
            .cloned()
            .unwrap_or_default();
        Focus {
            viewer_open,
            row: index.map(|index| table.rows[index].clone()),
            row_id,
            column,
            value,
        }
    }
}

fn focus(sessions: &Sessions, id: &str) -> anyhow::Result<Focus> {
    let session = sessions.open(id)?;
    let table = session.load_table()?;
    let (row_id, column) = session
        .read_view()?
        .map_or((None, None), |view| (view.row_id, view.column));
    Ok(Focus::at(&table, session.viewer_open(), row_id, column))
}
```

- **With no `view.json`** (never opened), every field but `viewer_open` is `null`, as the spec says.
- **A closed viewer's last position** is still reported, with `viewer_open: false`. The exit write keeps it on purpose (plan 06).
- **`value` is a plain `Value`,** which is `null` when missing. This is the lesson from plan 07's `Thread.value`: with an `Option`, a null cell and a missing cell would serialize differently.
- **`row`** is the row object as stored, including fields that aren't columns. It gives the agent context, which is the reason it's in the spec.
- **A broken `view.json`** is an error (exit 1), as `read_view` already makes it. Only a hand edit causes one, and the next viewer start rewrites the file.

Add `AgentCommand::Focus { id: String }` with the doc comment `/// What the user's cursor is on.`, and its arm in `run`. `Table::row_index` (Part 1's listing) arrives in this commit.

### Tests

- **`table.rs`:** `row_index` finds a keyed row by its key text (`"412"` for the number 412) and an unkeyed one by `#1`, and gives `None` for a missing ID.
- **`agent.rs`,** `Focus::at` on `prs.json`:
  - Row `404`, column `title` gives the title and the whole row.
  - Row `412`, which isn't in `prs.json`, gives `row: null` and `value: null` but keeps `row_id`.
  - A missing column gives `value: null` with the row.
  - `None` and `None` give nulls throughout.
- **`tests/cli.rs`:**
  - `focus` on a session with no `view.json` prints exactly `{"viewer_open":false,"row_id":null,"column":null,"value":null,"row":null}`.
  - `write_view` writes row `412`, so with `open_viewer` `focus` reports `viewer_open: true`, `row_id: "412"` and `row: null`.
  - Add `write_view_at(dir, pid, row_id, column)` for a view on `404`/`title`. `focus` then gives that title and `row.number == 404`. `write_view(dir, pid)` becomes `write_view_at(dir, pid, "412", "title")`, so only one function builds the JSON.

## Part 3: `comments`, `reply`, `comment` (ADI-227)

### `comments.rs`: the constructors build an `Add`

`reply` and `comment` print the message they wrote, so they need the `Add`, not an `Event` wrapped around it. Move `comment` and `reply` from `impl Event` to a new `impl Add`. The bodies are unchanged, minus the `Event::Add(…)` wrapper. Then replace the private `fn message(add)` with a `From`:

```rust
impl From<&Add> for Message {
    fn from(add: &Add) -> Message {
        Message {
            id: add.id.clone(),
            author: add.author,
            text: add.text.clone(),
            ts: add.ts.clone(),
        }
    }
}
```

- In `threads`, the call becomes `.map(|&add| Message::from(add))`: the items are `&&Add`.
- The viewer's `send_comment` wraps the result itself:

  ```rust
  let add = match thread {
      Some(thread) => Add::reply(id, Author::User, thread.id.clone(), text),
      None => Add::comment(id, Author::User, input.row_id, input.column, text),
  };
  self.pending.push(Event::Add(add));
  ```
- `constructors_fill_in_what_theyre_given` gets simpler: `let add = Add::comment(…)`, with no `let … else`.

### The commands

```rust
    /// Comment threads awaiting your reply. `--all` for every thread.
    Comments {
        id: String,
        #[arg(long)]
        all: bool,
    },
    /// Reply to a thread.
    Reply {
        id: String,
        thread: String,
        #[arg(allow_hyphen_values = true)]
        text: String,
    },
    /// Start a thread on a cell, to point the user at it.
    Comment {
        id: String,
        #[arg(long)]
        row: String,
        #[arg(long)]
        col: String,
        #[arg(allow_hyphen_values = true)]
        text: String,
    },
```

```rust
/// The session's comment log. Lines that were skipped are reported on
/// stderr, since stdout is the JSON.
fn read_log_and_warn(session: &Session) -> anyhow::Result<Log> {
    let log = session.read_log()?;
    if let Some(warning) = log.warning() {
        eprintln!("{COMMENTS_FILE}: {warning}");
    }
    Ok(log)
}

fn list_comments(sessions: &Sessions, id: &str, all: bool) -> anyhow::Result<Vec<Thread>> {
    let session = sessions.open(id)?;
    let table = session.load_table()?;
    let log = read_log_and_warn(&session)?;
    let threads = comments::threads(&log.events, &table);
    Ok(threads
        .into_iter()
        .filter(|thread| all || thread.awaiting_reply)
        .collect())
}

/// `text` without surrounding whitespace, or an error if that leaves nothing.
fn comment_text(text: &str) -> anyhow::Result<String> {
    let text = text.trim();
    if text.is_empty() {
        bail!("the comment is empty");
    }
    Ok(text.to_string())
}

fn reply(sessions: &Sessions, id: &str, thread: &str, text: &str) -> anyhow::Result<Message> {
    let text = comment_text(text)?;
    let session = sessions.open(id)?;
    let table = session.load_table()?;
    let log = read_log_and_warn(&session)?;
    // `threads` would drop a reply to anything else without a word.
    if !comments::threads(&log.events, &table)
        .iter()
        .any(|candidate| candidate.id == thread)
    {
        bail!("no thread \"{thread}\": `tv agent comments {id} --all` lists them");
    }
    let add = Add::reply(
        comments::new_id(&log.events),
        Author::Agent,
        thread.to_string(),
        text,
    );
    write_comment(&session, add)
}

fn comment(
    sessions: &Sessions,
    id: &str,
    row_id: &str,
    column: &str,
    text: &str,
) -> anyhow::Result<Message> {
    let text = comment_text(text)?;
    let session = sessions.open(id)?;
    let table = session.load_table()?;
    check_cell(&table, row_id, Some(column))?;
    let log = read_log_and_warn(&session)?;
    let add = Add::comment(
        comments::new_id(&log.events),
        Author::Agent,
        row_id.to_string(),
        column.to_string(),
        text,
    );
    write_comment(&session, add)
}

/// Append `add` to the session's log, and return it as the message the user
/// will see.
fn write_comment(session: &Session, add: Add) -> anyhow::Result<Message> {
    let message = Message::from(&add);
    session.append_events(&[Event::Add(add)])?;
    Ok(message)
}

/// An error unless `table` has row `row_id` and, if one is given, column
/// `column`. `comment` and `goto` check before they write anything.
fn check_cell(table: &Table, row_id: &str, column: Option<&str>) -> anyhow::Result<()> {
    if table.row_index(row_id).is_none() {
        let hint = match &table.key {
            Some(key) => format!("row IDs are the values of the key column \"{key}\""),
            None => "the table has no key, so row IDs are #0, #1, …".to_string(),
        };
        bail!("no row \"{row_id}\": {hint}");
    }
    if let Some(column) = column
        && table.column_index(column).is_none()
    {
        bail!("no column \"{column}\"");
    }
    Ok(())
}
```

`Table::column_index` (Part 1's listing) arrives in this commit, with `check_cell`. `key_column_index` becomes `self.column_index(self.key.as_deref()?)`, so there's one search by column name.

- **The output is the spec's.** `comments` prints `Thread`s, whose serde shape is the spec's (`cell` isn't serialized), in log order. They come from the same `threads` call as `tv save` and the viewer, so all three agree on `awaiting_reply` and `orphaned`.
- **Replying to an orphaned thread is allowed.** It still exists, and the agent may want to say why its row went away.
- **Appends go through `append_events`:** one `O_APPEND` write, then `sync_all`. The viewer picks them up on its next tick (plan 07), and the marker turns green.
- **ID collisions:** `new_id` avoids every ID in the log. A viewer appending at the same moment could draw the same one of 36⁵, which is negligible, and the first `add` would win.

### Tests

- **`comments.rs`:** the constructor test above, and `Message::from` copying the four fields.
- **`table.rs`:** `column_index` finds a column by name and gives `None` for a missing one.
- **`tests/cli.rs`.** Use a session from `prs.json` with `USER_COMMENT` (a user's thread on 404's title) and, where needed, `AGENT_REPLY`.
  - `comments` prints the one thread awaiting a reply. With `AGENT_REPLY` it prints `[]`, and `--all` prints the thread with both messages, `value` filled in.
  - `reply <id> c1 "on it"` prints a message with `author: "agent"` and a fresh `c…` ID. `comments` then prints `[]`. The log's last line has `"reply_to":"c1"`.
  - `reply` with text `-1 is wrong` works, through `allow_hyphen_values`.
  - `reply` to `c999`, to a reply's ID (`c2`), and with blank text each exit 1, and the log is unchanged.
  - `comment <id> --row 403 --col author "look"`:
    - It prints a message whose `id` then appears as a thread in `comments --all`, with `value: "fatima"` (403's author in `prs.json`; check the file) and `awaiting_reply: false`.
    - `comments` without `--all` doesn't list it: the agent wrote last.
  - `comment` on an unknown row or column exits 1 and writes no log. A session with no log still has no `comments.jsonl` afterwards.
  - `comment` on an unkeyed table uses `--row '#1'`.
  - A log with a `not json` line: `comments` still prints JSON on stdout, and stderr says `comments.jsonl: skipped lines that aren't events: 2`.

## Part 4: `goto` (ADI-228)

### The request file

`session.rs` gets a constant for the name, a type, and the two sides' IO:

```rust
pub const GOTO_FILE: &str = "goto.json";
// SESSION_FILES uses GOTO_FILE in place of the literal; its comment no longer
// needs to explain why an unused name is listed.

/// A cursor move `tv agent goto` asks the viewer for: `goto.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goto {
    pub row_id: String,
    /// `None` keeps the viewer's column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
}

impl Session {
    /// Ask the viewer to move its cursor: write `goto.json` atomically. A
    /// request the viewer hasn't applied yet is replaced.
    pub fn write_goto(&self, goto: &Goto) -> anyhow::Result<()> {
        files::write_json(&self.dir.join(GOTO_FILE), goto)
    }

    /// The pending `goto.json`, or `None` if there's none. It's deleted once
    /// read, before it's parsed, so a broken one is reported once rather
    /// than every tick.
    #[expect(
        clippy::disallowed_methods,
        reason = "deletes only goto.json, by name, right after reading it"
    )]
    pub fn take_goto(&self) -> anyhow::Result<Option<Goto>> {
        let path = self.dir.join(GOTO_FILE);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        let goto =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(Some(goto))
    }
}
```

`clippy.toml`'s `remove_file` reason becomes `"only Sessions::remove, files::write_atomic and Session::take_goto delete files"`.

**Read, then delete.** A second `goto` that lands between the two would be deleted unread, so the cursor would stay at the first one.
- The gap is the time between two syscalls in a row. `goto`s come from an agent's turns, seconds apart.
- Taking the file by renaming it first would close the gap. It would also cost a temp name, and handling for a temp left behind by a crash.
- Not worth it. Say this in the doc comment so a reviewer doesn't re-derive it.

### The agent side

```rust
    /// Move the user's cursor to a cell.
    Goto {
        id: String,
        #[arg(long)]
        row: String,
        /// Without it, the cursor keeps its column.
        #[arg(long)]
        col: Option<String>,
    },
```

```rust
fn goto(
    sessions: &Sessions,
    id: &str,
    row_id: &str,
    column: Option<&str>,
) -> anyhow::Result<Focus> {
    let session = sessions.open(id)?;
    let table = session.load_table()?;
    check_cell(&table, row_id, column)?;
    let viewer_open = session.viewer_open();
    // Without `--col` the cursor keeps its column: the open viewer's, which
    // `view.json` names, or the one a new viewer starts on.
    let focus_column = match column {
        Some(column) => Some(column.to_string()),
        None if viewer_open => session.read_view()?.and_then(|view| view.column),
        None => table
            .columns
            .get(table.start_column())
            .map(|column| column.name.clone()),
    };
    session.write_goto(&Goto {
        row_id: row_id.to_string(),
        column: column.map(str::to_string),
    })?;
    Ok(Focus::at(
        &table,
        viewer_open,
        Some(row_id.to_string()),
        focus_column,
    ))
}
```

A closed viewer's `view.json` doesn't say where the next viewer starts: `App::new` doesn't restore it, and starts on the key column. So the column comes from `view.json` only while a viewer is open. `table.rs` gets that start rule, and `App::new` uses it in place of its own `key_column_index().unwrap_or(0)`, so the two can't disagree:

```rust
/// The column a new viewer's cursor starts on: the key column, which is
/// drawn first, or else the first column.
pub fn start_column(&self) -> usize {
    self.key_column_index().unwrap_or(0)
}
```

- It prints the focus the move will give: ADI-228's "same shape as `tv agent focus`".
  - `view.json` is read before `goto.json` is written, so a broken `view.json` fails the command without leaving a request behind.
  - An open viewer whose `view.json` names no column (its table had none) gives `column: null`.
- An unknown row or column exits 1 with `check_cell`'s message, and no file is written.

### The viewer side

`sync.rs` gets one more step, after the two polls, so a push followed by a `goto` to a new row works in a single tick:

```rust
pub fn tick(&mut self, app: &mut App, now: Instant) {
    self.write_pending(app);
    self.poll_table(app, now);
    self.poll_comments(app);
    self.poll_goto(app);
    self.write_view_if_moved(app, now);
}

/// Apply a `goto.json` from `tv agent goto` once the viewer is ready for
/// one. Until then the file stays where it is.
fn poll_goto(&self, app: &mut App) {
    if !app.ready_for_goto() {
        return;
    }
    match self.session.take_goto() {
        Ok(None) => {}
        Ok(Some(goto)) => app.go_to(&goto.row_id, goto.column.as_deref()),
        Err(error) => app.set_message(file_error(GOTO_FILE, &error)),
    }
}
```

- `write_view_if_moved` comes after it, so `view.json` records the new cell once the 100ms debounce passes. A `focus` a moment later agrees with what `goto` printed.
- The module doc's sentence about `goto.json` being "one more step" becomes a description of it.

In `app.rs`, the rule about modes lives with the modes, so `SessionSync` doesn't have to know them:

```rust
/// Whether `tv agent goto` may move the cursor now: only in the grid. In
/// a popup, the record view, an input line or the help overlay, moving it
/// would change what the human is reading or typing about.
pub fn ready_for_goto(&self) -> bool {
    self.mode == Mode::Grid
}

/// Move the cursor where `tv agent goto` asked: row `row_id`, and column
/// `column` or the current one. The status line says the agent moved it,
/// since a cursor jumping on its own is puzzling. A row or column the
/// table doesn't have (a push since the request) moves nothing, and the
/// status line says why.
pub fn go_to(&mut self, row_id: &str, column: Option<&str>) {
    let Some(row) = self.table.row_index(row_id) else {
        self.set_message(format!("goto: no row \"{row_id}\""));
        return;
    };
    let col = match column {
        None => self.cursor.col,
        Some(name) => {
            let Some(col) = self.table.column_index(name) else {
                self.set_message(format!("goto: no column \"{name}\""));
                return;
            };
            col
        }
    };
    self.move_to(row, col);
    self.set_message("the agent moved the cursor here".to_string());
}
```

`move_to` clamps and scrolls the cell into view.

### Tests

- **`session.rs`:**
  - `write_goto` then `take_goto` returns the request and leaves no file, and a second `take_goto` is `None`. A `Goto` without a column writes no `column` field.
  - `take_goto` of `not json` is an error, and the file is gone.
  - `remove_deletes_a_pending_goto_too`: `Sessions::remove` on a session with `goto.json` succeeds. It's already in `SESSION_FILES`, but nothing tests it.
- **`table.rs`:** `start_column` is the key column's index on a keyed table (`prs.json`: 0; a table keyed on its second column: 1), and 0 without a key.
- **`app.rs`:**
  - `ready_for_goto` is true in the grid and false in each other mode.
  - `go_to("404", Some("title"))` on `prs.json` moves to row 1, column 1, with the message.
  - Without a column it keeps the cursor's column.
  - An unknown row or column leaves the cursor and says `goto: no row "…"` or `goto: no column "…"`, quoted the way `check_cell` quotes them.
  - On a 40-row table, a `go_to` past the first screen scrolls it into view.
- **`sync.rs`,** with the fixture and `other_side().write_goto(…)`. I ran the first two in the scratch copy.
  - A `goto.json` moves the cursor on the next tick, sets the message, and is gone.
  - In `Mode::Detail` the tick leaves the file and the cursor alone. After switching back to `Mode::Grid`, the next tick applies it.
  - A `goto.json` that doesn't parse gives a `goto.json: …` message and is deleted. The tick after that shows nothing new.
  - A push adding row `c` and a `goto` to `c`, both written before one tick: after that tick the cursor is on `c`.
  - After a `goto`, `view.json` names the new cell once the debounce has passed (tick at `now`, then at `now + 150ms`, like the existing debounce tests).
- **`tests/cli.rs`:**
  - `goto <id> --row 404 --col title` prints a focus with the title and `viewer_open: false`, and leaves a `goto.json` with both fields.
  - `--row 404` alone, with an open viewer's `view.json` on column `author` (`write_view_at` with `std::process::id()`), prints `column: "author"` and writes no `column` field.
  - The same `view.json` with `NO_SUCH_PID`, a closed viewer, prints `column: "number"` and 404's number: the column the next viewer starts on.
  - An unknown row or column exits 1 and leaves no `goto.json`.
  - `tv rm` of a session with a pending `goto.json` works.

## Part 5: the round trip (ADI-229)

Each part above already tests its own commands, and the `agent` helper checks that every one prints a single line of JSON. This commit adds the end-to-end test the issue describes. It's one test, run in order against one `TV_HOME`:

1. `new` from stdin: `prs.json`'s rows as a bare array, with `--key number --title PRs`.
2. `push` the same rows with PR 404's title changed and PR 409 removed. `get` shows the change, 39 rows, and the key.
3. `focus` with no viewer: all nulls.
4. `comment --row 404 --col title "retitled"`: `comments --all` has one thread, `value` is the new title, and `awaiting_reply` is false.
5. Act as the user: append a reply to that thread by hand, with `author: user` and `reply_to` set to the `id` step 4 printed. `comments` now lists it as awaiting. Use a new `append_comment(dir, line)` helper that opens the log with `append(true)`: `write_comments` calls `fs::write`, which would wipe out step 4's comment.
6. `reply` to it: `comments` is `[]`, and `--all` shows three messages in order (agent, user, agent).
7. `tv save <id> <home>/out.json`, in the test's temp dir as `save_writes_the_table_to_a_file` does: tests run in the crate root, and they don't delete files to clean up. Then `new` that file, which imports the comments into a second session, as `tv open` would with it. Its `comments --all` equals the first session's, compared as JSON (`value`, `awaiting_reply` and `orphaned` included), and its `get` equals the first's.

The duplicate-key push and the no-viewer `focus` from the issue are already in Parts 1 and 2. This commit's message names them, so the issue's list can be checked against it.

## Manual check

You need two terminals, with the same `TV_HOME` and directory in both. `tv` isn't installed, so put the build on `PATH` in both too; the printed `open_command` runs `tv`:

```sh
cd ~/tv && cargo build
export PATH="$PWD/target/debug:$PATH"
export TV_HOME=$(mktemp -d)
python3 -c 'import json; print(json.dumps(json.load(open("examples/prs.json"))["rows"]))' > /tmp/rows.json
tv agent new /tmp/rows.json --key number --title "Open PRs"
# prints {"session":"<id>","open_command":"TV_HOME='…' tv open <id>","rows":40,"columns":9}
```

1. **Opening.** Run the printed `open_command` in terminal 1. It opens the table, with `Open PRs` in the title bar.
2. **Focus.** Move to PR 404's title. In terminal 2, `tv agent focus <id>` prints `viewer_open: true`, `row_id: "404"`, `column: "title"`, the title, and the row.
3. **Push.** Edit `/tmp/rows.json` to drop PR 409 and change 404's title, then `tv agent push <id> /tmp/rows.json` (a bare array, no `--key`).
   - The viewer reloads with the cursor still on 404's title, which shows the new title. It isn't highlighted: the changed cell is the cursor cell, and `cell_style` keeps the reload highlight off it.
   - `tv agent get <id>` still has `"key":"number"` and the title.
4. **A rejected push.** Push the rows with a second `"number": 404` row. It exits 1 with `duplicate key value "404"`, and the viewer doesn't change.
5. **Agent comment.** `tv agent comment <id> --row 403 --col author "is this the right reviewer?"`.
   - A green `▸` appears on 403's author, because the agent wrote last.
   - `]` jumps to it.
   - `tv agent comments <id>` prints `[]`.
6. **User comment.** In the viewer, press `c` on 404's title, type `why the rename?`, then `Enter`. The marker is yellow.
   - `tv agent comments <id>` lists that thread.
   - `tv agent reply <id> <thread-id> "to match the issue"`: within a quarter second the marker turns green and the status line's `awaiting reply` count goes away.
7. **Goto.** `tv agent goto <id> --row 372 --col title`.
   - The cursor jumps to PR 372's title, scrolling if needed, and the status line says `the agent moved the cursor here`.
   - `tv agent focus <id>` agrees.
8. **Goto waits outside the grid.** Press `Enter` on any cell to open the popup, then `tv agent goto <id> --row 403`.
   - Nothing moves, and `ls $TV_HOME/sessions/<id>` shows `goto.json`.
   - `Esc`: the cursor jumps to 403 in the same column, and `goto.json` is gone.
9. **Goto with no viewer.** `q` the viewer, then `tv agent goto <id> --row 392`. It prints `viewer_open: false` and `"column":"number"`, the column a new viewer starts on. Run the `open_command` again: the viewer opens on 392's number.
10. **Sessions and cleanup.**
    - `tv agent sessions` lists the session with `viewer_open`, `awaiting_reply: 0`, and `modified_at`.
    - `q`, then `tv agent goto <id> --row 403`. With no viewer to apply it, `ls $TV_HOME/sessions/<id>` shows `goto.json`.
    - `tv rm <id>`: it's removed, pending `goto.json` included.
    - `unset TV_HOME`.
11. **No input.** `tv agent new` in an interactive shell, with no file or pipe, exits at once with `no table: give a FILE, or pipe JSON to stdin`.

## Acceptance

```sh
make check
cargo tree --locked -e normal --prefix none | sort -u | wc -l   # 89, unchanged
```

Plus the manual check above.

## Deliverables

- **`src/agent.rs` (new):**
  - `AgentCommand`, `TableInput`, `run`.
  - `Created`, `Listed`, `Focus`.
  - `new`, `push`, `list_sessions`, `focus`, `list_comments`, `reply`, `comment`, `goto`.
  - Helpers: `read_stdin`, `open_command`, `shell_quote`, `read_log_and_warn`, `comment_text`, `write_comment`, `check_cell`.
  - Tests.
- **`src/main.rs`:** `mod agent`, `Command::Agent`, and its one-line arm.
- **`src/table.rs`:**
  - `Meta`, `parse_with` and `from_path_with`. `parse` and `from_path` call them with no meta.
  - `row_index`, `column_index` and `start_column`. `key_column_index` becomes `self.column_index(self.key.as_deref()?)`.
- **`src/comments.rs`:** `Add::{comment, reply}`, moved from `Event`, and `From<&Add> for Message` replacing `fn message`.
- **`src/session.rs`:**
  - `GOTO_FILE` (used in `SESSION_FILES`), `Goto`, and `Session::{write_goto, take_goto}`.
  - `Sessions.tv_home`, filled in by `from_home`, and `Sessions::tv_home()`.
- **`clippy.toml`:** the `remove_file` reason names `Session::take_goto`.
- **`src/viewer/app.rs`:** `ready_for_goto`, `go_to`, `send_comment` using `Add`, and `App::new` starting on `table.start_column()`.
- **`src/viewer/sync.rs`:** `poll_goto` in `tick`, and the module doc.
- **`tests/cli.rs`:** the `agent`, `write_view_at` and `append_comment` helpers, and the tests above.
- **Comments that describe the agent CLI as future work.** Update each in the commit that makes it true:
  - `src/session.rs`'s module doc ends "Keep that rule when adding other ways to create sessions (`tv agent new`, ADI-225)". `new` goes through `create`, so the reminder goes (commit 2).
  - `tests/cli.rs`'s `write_session` says "No command makes one without the viewer until `tv agent new`". It's still useful because it can write a `table.json` no command would, such as a broken one. Say that instead (commit 2).
  - `src/comments.rs`'s `Thread` doc says "`tv agent comments` will print it": now it does (commit 4).
- **`SPEC.md`:**
  - "Agent CLI":
    - Each command prints one line of compact JSON.
    - `open_command` includes `TV_HOME` when it's set.
    - `push` keeps the title and key when the input names neither, and refuses `comments`.
    - `focus`'s `row` is `null` when the row is gone.
    - `reply` refuses an unknown thread. `reply` and `comment` print the new message.
    - `sessions` gives `null`s for an unreadable table.
    - A new `tv agent goto` subsection, moved from the "Changes after review" bullet: it waits for the grid, works with no viewer open, and the status line says the agent moved the cursor.
  - "Not doing": the `Agent moving the human's cursor` row points at the new `tv agent goto` subsection instead of "Changes after review". ADI-228 asks for this.
  - "Live updates": the viewer also applies a `goto.json` on its tick, in the grid only.
  - "Sessions": the directory listing gains `goto.json  # written by the agent CLI, deleted by the viewer once applied`.
  - Module layout: `agent.rs  # tv agent subcommands: their clap definitions and JSON output`. `sync.rs` also applies `goto.json`. `agent_help.txt` stays listed for ADI-203.
- **Six commits:**
  1. `Plans: agent CLI (ADI-225..229)`: this file, after its review.
  2. `tv agent new, push, get, sessions (ADI-225)`: Part 1. The callers that keep `dead_code` quiet:
     - `Meta`, `parse_with` and `from_path_with`: `TableInput::read`, plus `parse` and `from_path`.
     - `open_command` and `Sessions::tv_home`: `Created::of`.
  3. `tv agent focus (ADI-226)`: Part 2, with `Table::row_index` for `Focus::at`.
  4. `tv agent comments, reply, comment (ADI-227)`: Part 3, with `Table::column_index` for `check_cell`, and `key_column_index` switched to it. The `Add` move is in this commit, which is the first to need it.
  5. `tv agent goto, applied by the viewer (ADI-228)`: Part 4, with `Table::start_column` for `goto` and `App::new`. `check_cell`, `row_index` and `column_index` exist by then.
  6. `Agent CLI round-trip test (ADI-229)`: Part 5.

## Out of scope

- **`tv agent help` and the guide:** ADI-203, with the points listed under "What this plan defers".
- **Deleting or editing comments from the agent's side.** The spec has no such command, and the user deletes with `d`.
- **`patch`, `wait`, and notifications.** All three are on the spec's "Not doing" list.
- **Restoring the cursor from `view.json` when a viewer opens.** `goto` lets the agent point the user at a cell, which covers the reason to want it.
- **Two viewers on one session.** Whichever ticks first takes a `goto`. The spec leaves this case out.
- **Locking between two agents pushing to one session.** The last write wins, atomically.
