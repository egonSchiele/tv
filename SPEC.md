# tv — a table viewer you share with an agent

## Problem

Agents often show tabular data by printing it to the terminal. Wide tables wrap and become unreadable, and there's no way to point at a specific cell and ask about it.

`tv` is a terminal table viewer that a human and an agent share:

- The agent pushes a table; the human sees it in a proper viewer with scrolling, colors, and truncation.
- The agent can see which cell the human is looking at.
- The human can comment on a cell; the agent reads and replies to comments.
- Either side can save the table (with comments) as JSON and reopen it later.

The agent learns the tool by running `tv agent help`. A user should be able to tell any agent: *"Use `tv` to show me this. Run `tv agent help` to learn how."*

## Principles

- **Simple over clever.** Each feature must earn its place; effort vs. payoff is weighed explicitly (see "Not doing").
- **Files, not servers.** A session is a directory. The agent CLI reads and writes files; the viewer polls them. No daemon, no sockets, no async.
- **Agent output is JSON.** Every `tv agent` command except `help` prints JSON to stdout. Errors go to stderr as plain text with a non-zero exit code.
- **The agent is turn-based.** It never gets notified. The human says "check my comments" and the agent runs a command.

## Not doing (v1)

| Idea | Why not |
|---|---|
| Network / remote sessions | High effort; both parties are on the same machine. |
| Agent notifications / `wait` command | Human telling the agent to check comments is enough. Revisit if that proves annoying. |
| Editing cells in the viewer | The agent owns the data; the human comments. |
| Sorting / filtering / search in viewer | Moderate effort; the agent can re-push a sorted/filtered table on request. Search (`/`) is the first candidate for v2. |
| Row patch API | Agent can `get`, modify (e.g. with `jq`), and `push`. Add `patch` if full re-pushes prove slow. |
| Multi-cell selection | Cursor-only covers "this one". |
| CSV / other input formats | JSON only. Agents can convert. |
| ~~Agent moving the human's cursor~~ | **Reversed 2026-10-28.** See "Changes after review" below. |
| Auto-opening the viewer (tmux splits etc.) | Environment-specific. The agent just tells the human what to run. |

## Concepts

- **Table**: columns + rows, described by the JSON format below.
- **Session**: a directory containing the current table, the viewer's cursor state, and the comment log.
- **Viewer**: `tv open`, the full-screen terminal UI the human uses.
- **Agent CLI**: `tv agent …`, non-interactive commands for agents.

## Table format

This one format is used as input to `new`/`push` and as output of `save` and `get`.

```json
{
  "version": 1,
  "title": "Open pull requests",
  "key": "number",
  "columns": [
    { "name": "number", "type": "number" },
    { "name": "title", "width": 50 },
    { "name": "author" },
    { "name": "additions", "type": "number" }
  ],
  "rows": [
    { "number": 412, "title": "Fix login redirect", "author": "sam", "additions": 12 }
  ],
  "comments": []
}
```

| Field | Required | Notes |
|---|---|---|
| `version` | no | Defaults to `1`. |
| `title` | no | Shown in the viewer's title bar. |
| `key` | no, strongly encouraged | Column whose values identify rows. Must be unique per row (push is rejected otherwise). Comments and cursor position follow rows by key across updates. Without a key, rows are identified by index (`"#0"`, `"#1"`, …) and comments may land on the wrong row after a re-push. |
| `columns` | no | If omitted, inferred from the union of row object keys in first-seen order. |
| `columns[].name` | yes | Matches the row object key. |
| `columns[].type` | no | `string`, `number`, `bool`. Inferred from values if omitted. Affects alignment only (numbers right-aligned). |
| `columns[].width` | no | Max display width. Default cap is 40. |
| `rows` | yes | Array of objects. Missing fields render as empty. Fields not in `columns` are ignored (when columns are explicit). |
| `comments` | no | Only present in saved files. Same shape as `tv agent comments --all` output. Imported on open. |

**Shorthand:** a bare JSON array of objects is also accepted as a table (`[{...}, {...}]`), so agents can pipe API output straight in:

```sh
gh pr list --json number,title,author | tv agent new --key number --title "Open PRs"
```

**Cell values:** strings, numbers, booleans, and null render directly. Nested objects/arrays render as compact JSON in the grid and pretty-printed JSON in the cell detail view.

## Sessions

Location: `~/.tv/sessions/<id>/` (override root with `TV_HOME`).

Session IDs are 6 lowercase alphanumeric characters (e.g. `k3f9x2`), short enough to type.

```
~/.tv/sessions/k3f9x2/
  table.json      # written by agent CLI (atomic: write temp file, rename)
  view.json       # written by viewer (atomic)
  comments.jsonl  # append-only event log, written by both sides
```

A session's `table.json` never holds comments: they live only in `comments.jsonl`. Opening a file that has `comments` moves them into the new session's log, and `tv save` writes them back out from it.

### `view.json`

Written by the viewer when the cursor moves (debounced ~100ms) and on start/exit.

```json
{
  "pid": 81234,
  "row_id": "412",
  "column": "title",
  "updated_at": "2026-10-28T19:02:11Z"
}
```

A viewer is considered open if `pid` is alive. The exit write leaves `pid` out (and keeps the final position), so only a crashed viewer leaves a pid behind that the system could later reuse. If two viewers have the same session open, the last write wins. This case isn't handled further.

### `comments.jsonl`

One JSON event per line. Appends are small single writes, safe enough for two local writers.

```json
{"op":"add","id":"c7h2k1","author":"user","row_id":"412","column":"title","text":"Why is this one still open?","ts":"2026-10-28T19:03:00Z"}
{"op":"add","id":"c9m4p0","author":"agent","reply_to":"c7h2k1","text":"Waiting on review from infra.","ts":"2026-10-28T19:04:12Z"}
{"op":"delete","id":"c7h2k1","ts":"2026-10-28T19:05:00Z"}
```

- A **thread** is a top-level comment (anchored to a cell by `row_id` + `column`) plus its replies.
- `delete` on a top-level comment deletes the whole thread.
- A thread is **awaiting reply** if its last message is from `user`. This is derived from the log, so there's no read/resolved state to manage.
- A thread is **orphaned** if its row or column no longer exists in the table. Orphaned threads are kept and reported to the agent; the viewer shows a count in the status bar.
- Comment IDs are `c` + 5 random alphanumeric characters.
- A `delete` of a reply removes just that reply. A `delete` counts wherever it is in the log, before or after its `add`.
- A reply joins the thread its `reply_to` names wherever it is in the log, so one logged before its comment still attaches. A reply to anything but a top-level comment is dropped.
- If two `add`s have the same ID, the first in the log wins.
- A line that doesn't parse is skipped rather than failing the whole log, which only grows; the viewer and `tv save` say which lines were skipped.

### Live updates

The viewer polls the mtimes of `table.json` and `comments.jsonl` every 250ms and reloads on change.

On table reload:
- Cursor stays on the same row ID and column name; if either is gone, clamp to the nearest valid position.
- Cells whose value changed, and new rows, are highlighted for ~5 seconds.
- If `table.json` fails to parse, keep showing the last good table and display the error in the status bar.

## Human CLI

| Command | Behavior |
|---|---|
| `tv open` | Open the most recently modified session. |
| `tv open <id>` | Open a session. |
| `tv open <file.json>` | Create a new session from the file (including comments) and open it. |
| `tv ls` | List sessions: id, title, rows × cols, whether a viewer is open, how many threads are awaiting a reply, last modified. |
| `tv save <id> <file.json>` | Write table + comments to a file. |
| `tv rm <id>… [--force]` | Delete sessions. Refuses a session whose viewer is open, unless `--force`. |

An argument to `tv open` is treated as a file if it contains `.` or `/`, and as a session ID otherwise.

## Viewer

### Layout

```
┌ Open pull requests ─────────────────────────── k3f9x2 ┐
│ number │ title                      │ author │ additi…│
│    412 │ Fix login redirect         │ sam    │     12 │
│    415 │ Add retry to upload client…│ jo     │    340▸│
│ …                                                     │
├───────────────────────────────────────────────────────┤
│ row 2/38  col 4/9  ·  2 awaiting reply  ·  ? for help │
└───────────────────────────────────────────────────────┘
```

- **Title bar:** table title and session ID. The ID is shown so the human can tell the agent which session to use.
- **Header row:** bold; always visible.
- **Pinned column:** the `key` column, if set, is always shown at the left during horizontal scroll.
- **Status bar:** cursor position, comment counts, transient messages (save confirmations, parse errors).

### Rendering

- Column width is `min(cap, max(header width, widest cell))`, where cap is `columns[].width` or 40. Measure with display width (`unicode-width`), not byte or char count.
- Truncated content ends with `…`. Newlines in cells render as `↵`.
- Horizontal scrolling moves by whole columns so the cursor column is always fully visible.
- Numbers are right-aligned. Nulls are dimmed.
- Cursor cell: reversed. Cursor row: subtle background.
- Comment marker: `▸` in the cell's last character, its right padding, so it never hides text. Yellow means awaiting reply; green means the agent replied last. Without color, awaiting is bold and replied is dim.
- Changed cells (after reload): highlighted background for ~5s.
- Respect `NO_COLOR`.
- Handle terminal resize.
- Empty table: show "No rows" under the header.

### Keys

| Key | Action |
|---|---|
| Arrows / `hjkl` | Move cursor |
| `PgUp` / `PgDn` | Page up/down |
| `g` / `G` | First / last row |
| `0` / `$` | First / last column |
| `Enter` | Cell detail: full value (wrapped; nested JSON pretty-printed) and the cell's comment thread |
| `r` | Record view: current row as a vertical field/value list. `j`/`k` move between fields, `n`/`p` next/previous row, `Esc` back |
| `c` | Comment on current cell (single-line input; `Enter` submits, `Esc` cancels). If the cell already has a thread, adds to it; with several, to the newest. Also works in cell detail. |
| `d` (in cell detail) | Delete this cell's threads (asks; `y` deletes) |
| `]` / `[` | Jump to next / previous cell with a comment thread, by row and then left to right on screen. Wraps around. |
| `s` | Save: prompts for a path, defaulting to `./<title-slug>.json` |
| `?` | Help overlay listing keys |
| `q` | Quit (session is kept) |

## Agent CLI

All commands print JSON unless noted. `<id>` is required; agents find IDs from `new` output, from the user, or from `tv agent sessions`.

### `tv agent help`

Prints the agent guide as plain text (see below). The guide is embedded in the binary with `include_str!`.

### `tv agent new [FILE] [--title T] [--key K]`

Create a session from FILE or stdin. `--title` and `--key` override or fill in the file's values.

```json
{ "session": "k3f9x2", "open_command": "tv open k3f9x2", "rows": 38, "columns": 9 }
```

### `tv agent push <id> [FILE] [--title T] [--key K]`

Replace the session's table. Same input and output as `new`. Rejected if key values are missing or duplicated.

### `tv agent get <id>`

Print the current table (table format, without comments).

### `tv agent focus <id>`

```json
{
  "viewer_open": true,
  "row_id": "412",
  "column": "title",
  "value": "Fix login redirect",
  "row": { "number": 412, "title": "Fix login redirect", "author": "sam", "additions": 12 }
}
```

`row` gives the agent context about what the human is looking at. If the viewer has never been opened, `row_id`/`column`/`value`/`row` are null.

### `tv agent comments <id> [--all]`

By default lists only threads awaiting reply. `--all` lists every thread.

```json
[
  {
    "id": "c7h2k1",
    "row_id": "412",
    "column": "title",
    "value": "Fix login redirect",
    "awaiting_reply": true,
    "orphaned": false,
    "messages": [
      { "id": "c7h2k1", "author": "user", "text": "Why is this one still open?", "ts": "2026-10-28T19:03:00Z" }
    ]
  }
]
```

### `tv agent reply <id> <thread-id> <text>`

Append an agent reply to a thread. Prints the new message.

### `tv agent comment <id> --row <row-id> --col <column> <text>`

Start a thread on a cell, for when the agent wants to point the human at something. The human finds it with `]`.

### `tv agent sessions`

```json
[
  { "session": "k3f9x2", "title": "Open pull requests", "rows": 38, "columns": 9,
    "viewer_open": true, "awaiting_reply": 1, "modified_at": "2026-10-28T19:05:00Z" }
]
```

### Agent guide (content of `tv agent help`)

Short and example-driven. It should cover:

1. **What tv is**, in one paragraph.
2. **When to use it:** tables with more than ~6 columns or ~20 rows, or whenever the user asks.
3. **Quick start:** pipe JSON into `tv agent new` with `--key` and `--title`, then tell the user to run the printed `open_command`.
4. **Always set a key** if you might update the table, so comments and the cursor follow rows.
5. **Updating:** `push` a full new table; for small edits, `get | jq … | push`.
6. **Comments:** when the user says to check comments, run `comments`, reply to each thread with `reply`, and push table changes if the comments asked for them.
7. **Context:** when the user says "this one" or "here", run `focus`.
8. **Finding a session:** if you don't have an ID, run `sessions`.
9. **Don't poll.** Only check comments or focus when the user asks or refers to the table.
10. **Reference:** one line per command with its arguments.

## Implementation

### Stack

- Rust, single binary.
- `ratatui` (crossterm backend, via `ratatui::crossterm`): terminal UI.
- `clap` (derive): CLI parsing.
- `serde` + `serde_json`: formats.
- `anyhow`: errors.
- `unicode-width`: column measurement.
- Dev: `tempfile` (CLI tests).

Keep dependencies few to limit supply-chain risk (see "Dependencies" in README.md). Things the standard library covers without a crate:

- Home directory: `std::env::home_dir()`.
- Random IDs: `std::hash::RandomState`, which the OS seeds. IDs only need to avoid collisions, not resist guessing; check for an existing session directory and retry.
- Timestamps: a small function that formats `SystemTime::now()` as UTC RFC 3339 (`2026-10-28T19:02:11Z`), with unit tests against known dates.
- CLI tests: `std::process::Command` with `env!("CARGO_BIN_EXE_tv")`.
- Snapshot tests: a small helper that compares text to a file under `snapshots/` (see Testing).

### Module layout

```
src/
  main.rs          # clap definitions, dispatch
  table.rs         # Table/Column/Value types, parsing, shorthand, inference, validation, row IDs
  files.rs         # atomic writes, ~ expansion, slugs (shared by :w and sessions)
  session.rs       # paths, ID generation, listing, most-recent lookup, safe removal, view.json and viewer-open check, reading and appending comments.jsonl
  time.rs          # RFC 3339 timestamps (ls, view.json, comments)
  comments.rs      # comment events and threads, the log's line format, folding events into threads
  agent.rs         # tv agent subcommands
  agent_help.txt   # guide text, include_str!'d
  viewer/
    mod.rs         # terminal setup/teardown, event loop (key events + 250ms tick)
    app.rs         # App state: table, cursor, scroll, mode (Grid/Detail/Record/Command/Comment/ConfirmDelete/Help), highlights, comment threads
    render.rs      # draw functions; pure, take &App
    layout.rs      # column widths, visible column range given pinned column and terminal width
    sync.rs        # the viewer's file IO: writes view.json and comments, polls table.json and comments.jsonl
    diff.rs        # after a reload: where the cursor goes, which cells changed
```

Keep `render` pure (state in, frame out) and keep all mutation in `app.rs`. This is the standard ratatui pattern and avoids borrow-checker trouble.

### Testing

- Unit tests: table inference and validation, row ID derivation, comment folding (awaiting/orphaned/deleted), column width and visible-range calculation.
- Render snapshots: ratatui `TestBackend` rendered to text and compared to a committed `.txt` file (`make snapshots` rewrites them; review with `git diff`) for the grid, truncation, pinned column, comment markers, and record view.
- CLI tests: `std::process::Command` on `env!("CARGO_BIN_EXE_tv")` with `TV_HOME` pointed at a temp dir; exercise new → push → comment → reply → save → open-file round trip.

## Milestones

Each milestone is usable on its own.

1. **Static viewer.** `tv open file.json`, without sessions yet: render, navigate, widths, truncation, pinned key column, colors, resize.
2. **Sessions + agent push.** Session dirs, `agent new/push/get/sessions`, `tv open <id>`, `tv ls/rm`, live reload with cursor preservation, parse-error handling.
3. **Focus.** Viewer writes `view.json`; `agent focus`.
4. **Comments.** Event log, viewer input and markers, cell detail view, `]`/`[`, delete, `agent comments/reply/comment`.
5. **Polish.** Save (viewer `s` and `tv save`), open-from-file with comments, record view, change highlighting, help overlay, `tv agent help` guide text.

## Open questions

- ~~Session cleanup~~: resolved, see below.
- ~~Name collision~~: resolved, see below.

## Changes after review (2026-10-28)

Decisions made while planning. Work is tracked in the Linear project "table viewer" (P-ADI-8, issues ADI-197 through ADI-233). Per-ticket plans live in `plans/`.

- **Agent can move the cursor.** `tv agent goto <id> --row R [--col C]` writes `goto.json` into the session dir; the viewer applies it on the next poll and deletes the file. Reverses the "Not doing" entry above.
- **Command line.** `:` opens a vim-style command line in the viewer. `:w [path]` saves, `:help`, `:q`. Single-key bindings from the Keys table remain as shortcuts. The command line is the extension point for future commands.
- **Session pruning.** No automatic pruning. `tv session prune [--older-than 7d] [--closed] [--dry-run]` is explicit.
- **Skills.** `tv skills` lists bundled agent skills; `tv skills show <name>` prints one; `tv skills install <name> --to <dir>` writes it out.
- **Name.** The binary stays `tv`. The crates.io name `tv` is taken by tidy-viewer, so publish the crate under a different name (e.g. `tv-table`) with `[[bin]] name = "tv"`.
- **Build tooling.** `make` builds, `make run` opens `examples/prs.json`, `make test` runs tests, `make check` runs fmt + clippy + tests (what CI runs).
