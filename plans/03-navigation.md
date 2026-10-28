# Plan: ADI-199 User navigation: cursor, highlighting, status line, command line

Linear: https://linear.app/aditya-bhargava/issue/ADI-199
Sub-issues: ADI-212 (cursor + scroll), ADI-213 (highlighting + flicker-free), ADI-214 (status line + help overlay), ADI-215 (`:` command line), ADI-216 (cell detail + record view).
Depends on: ADI-198 merged.

## Goal

The viewer becomes interactive. Cursor moves with arrows and vim keys, the view scrolls to keep it visible, the cursor cell/row/column are highlighted in three tiers, the status line shows position and hints, `?` shows help, `:` opens a command line, `Enter` shows cell detail, `r` shows the record view. Colors respect `NO_COLOR`. No flicker.

## Context for the executing agent

- Read `SPEC.md`: "Viewer" (all), and "Changes after review" for the command line decision.
- All state changes go in `viewer/app.rs`. `render.rs` stays pure. `layout.rs` stays free of ratatui types.
- Five commits, one per sub-issue, in the order below. Each must pass `make check`.
- Flicker: ratatui keeps a double buffer and writes only changed cells. Draw the full frame on every loop iteration and never call `terminal.clear()` except after leaving a mode that drew outside the normal layout (not needed here). Do not sleep in the loop; `event::poll(250ms)` is the only wait.

## Part 1: Cursor and scrolling (ADI-212)

### State

```rust
pub struct Cursor { pub row: usize, pub col: usize }   // indices into table.rows / table.columns

pub struct App {
    // existing fields
    pub cursor: Cursor,
    pub first_row: usize,   // vertical scroll offset
    pub first_col: usize,   // index of first non-pinned column shown
    pub body_height: u16,   // last known body height, set by render via a hook (see below)
}
```

Render needs to tell `App` how many rows fit so page-down and scroll-into-view work. Keep render pure by having the event loop compute the body height from `terminal.size()` using the same `Layout` constants (extract `pub fn body_rows(total_height: u16) -> u16` into `layout.rs`) and store it on `App` before handling keys.

### Keys (grid mode)

| Key | Action |
|---|---|
| `↓`/`j`, `↑`/`k` | row ±1 |
| `→`/`l`, `←`/`h` | col ±1 |
| `PgDn`/`PgUp`, `Ctrl-f`/`Ctrl-b` | row ± body_height |
| `g` / `G` | first / last row |
| `0` / `$` | first / last column |
| `q` | quit |

### Scroll rules (`App::scroll_into_view`)

- Vertical: if `cursor.row < first_row`, `first_row = cursor.row`. If `cursor.row >= first_row + body_height`, `first_row = cursor.row - body_height + 1`.
- Horizontal: if the cursor is on the pinned column, no change. Otherwise, if `cursor.col < first_col`, `first_col = cursor.col`. If `cursor.col` is not in `layout::visible_columns(...).scrolled`, increment `first_col` until it is. Loop terminates because a single column always fits (clipped) when it is first.
- Clamp cursor to table bounds on every move. Empty table: cursor stays at (0,0), moves are no-ops.

### Tests

Unit tests on `App` with a 50-row, 10-column synthetic table and `body_height = 10`: page down from row 0 → row 10, first_row 10; `G` → row 49, first_row 40; `$` → last col, `first_col` advanced so it is visible; `h` from col 0 is a no-op; cursor on pinned col never changes `first_col`.

## Part 2: Highlighting and colors (ADI-213)

### Theme

```rust
pub struct Theme {
    pub header: Style,
    pub cursor_cell: Style,
    pub cursor_row: Style,
    pub cursor_col: Style,
    pub null: Style,
    pub border: Style,
    pub status: Style,
}

impl Theme {
    pub fn detect() -> Self  // NO_COLOR set (any value) → Theme::mono(), else Theme::color()
}
```

- `color()`: `cursor_cell` = `REVERSED | BOLD`; `cursor_row` and `cursor_col` = `bg(Color::Indexed(236))` or `Color::DarkGray`, a dim background clearly lighter than default; `null` = `DIM`; `header` = `BOLD`.
- `mono()`: `cursor_cell` = `REVERSED`; `cursor_row`/`cursor_col` = `UNDERLINED`? No: underline on a whole row is noisy. Use `BOLD` for row/col in mono, `REVERSED` for cell. Everything else default + `DIM` for null.
- Pass `&Theme` into `draw` via `App` (store `app.theme`).

### Render changes

Per cell, pick style: cursor cell > (cursor row or cursor col) > plain. Apply `patch` so `DIM` for null and `BOLD` for header still combine. The pinned column cell on the cursor row gets the row style.

### Tests

Snapshots with cursor at (0,0), at the bottom-right after `G$`, and in the middle. `TestBackend` records styles, so the snapshot shows `REVERSED` etc. Add one snapshot with `NO_COLOR` forced via `Theme::mono()` (do not read the real env in tests).

## Part 3: Status line and help overlay (ADI-214)

### Status line content

```
 row 2/38  col 4/9  ·  ? help  : cmd  q quit                  <transient message>
```

- Left: `row {cursor.row+1}/{rows}  col {cursor.col+1}/{cols}`. Zero rows: `no rows`.
- Middle: fixed hints.
- Right: `app.message: Option<(String, Instant)>`; shown until 3s old; the event loop's 250ms tick clears expired messages (`App::tick()`).

### Modes

```rust
pub enum Mode { Grid, Help, Command(CommandLine), Detail, Record(RecordState) }
```

`App::handle_key` dispatches on `self.mode`. `Esc` from any non-Grid mode returns to Grid.

### Help overlay

`?` → `Mode::Help`. Render a centered `Paragraph` in a bordered `Block` titled "Keys", using `Clear` under it, listing every key from SPEC.md "Keys" plus `:` commands. Any key closes it.

## Part 4: Command line (ADI-215)

### State

```rust
pub struct CommandLine { pub buffer: String, pub cursor: usize }

pub enum Command { Help, Quit, Write(Option<String>), Unknown(String) }

impl std::str::FromStr for Command { ... }   // "help"|"h" → Help, "q"|"quit" → Quit, "w [path]" → Write, else Unknown
```

### Behavior

- `:` in Grid → `Mode::Command` with empty buffer. The status line is replaced by `:` + buffer, with the terminal cursor shown at the right position (`frame.set_cursor_position`).
- Printable chars insert at cursor; `Backspace` deletes before cursor; `←`/`→` move; `Home`/`End`; `Ctrl-u` clears.
- `Enter` parses with `Command::from_str` and runs `App::run_command`. `Esc` cancels.
- `Help` → `Mode::Help`. `Quit` → `should_quit`. `Write` → set `message = "save not implemented yet"` (ADI-218 implements it; leave the arm in place). `Unknown(s)` → `message = format!("unknown command: {s}")`.

### Tests

`Command::from_str` table test. App test: `:`, type `q`, `Enter` → `should_quit`. `:`, `Esc` → back in Grid with no side effects. Snapshot of the command line mid-edit.

## Part 5: Cell detail and record view (ADI-216)

### Cell detail (`Enter`)

`Mode::Detail`. Render a centered bordered popup (about 80% × 60% of the frame, `Clear` under it) titled `{column} · row {row_id}`. Body: the raw value. Strings wrapped with `Wrap { trim: false }`; nested JSON via `serde_json::to_string_pretty`; null shown as `null` dimmed. Leave a reserved empty area at the bottom with the heading `Comments` and the text `none yet`; ADI-224 fills it. `Esc`, `Enter`, or `q` closes.

### Record view (`r`)

`Mode::Record(RecordState { field: usize })`. Render full-body (replace the grid, keep title and status bar): one line per column, `name` right-aligned in a column as wide as the longest name, then `│`, then value (`cell_text`, truncated to remaining width). The current field line uses `theme.cursor_row`; the current cell uses `theme.cursor_cell`. Status line shows `row {n}/{total}  field {f}/{cols}  ·  j/k field  n/p row  Esc back`.

Keys: `j`/`k`/`↓`/`↑` move field (and sync `cursor.col`), `n`/`p` move row (sync `cursor.row`, clamp), `Esc`/`r`/`q` back to Grid. Returning to Grid calls `scroll_into_view` so the grid cursor matches.

### Tests

Snapshots: detail with a long wrapped string, detail with a nested object, record view mid-table.

## Flicker and resize check (manual)

Hold `j` down for a few seconds on `examples/prs.json`: the grid should scroll smoothly with no blank frames. Resize the terminal to very narrow (30 cols) and back. Open and close `?` and `Enter` popups; nothing should leave residue (that's what `Clear` is for).

## Acceptance

```sh
make check
make run    # exercise every key in the SPEC table plus : commands and ? overlay
NO_COLOR=1 make run   # no colors, cursor still obvious
```

## Deliverables

- `src/viewer/app.rs`: `Cursor`, `Mode`, `Theme`, `CommandLine`, `Command`, `tick`, `scroll_into_view`, key handling
- `src/viewer/render.rs`: grid with styles, status line, help overlay, command line, detail popup, record view
- `src/viewer/layout.rs`: `body_rows`
- `src/viewer/mod.rs`: body height computed before key handling, `tick` on poll timeout
- Snapshots for each view
- Five commits, in sub-issue order

## Out of scope

`:w` implementation (ADI-218), comments in detail view (ADI-224), change highlighting after reload (ADI-221), `goto.json` handling (ADI-228).
