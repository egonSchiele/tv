# Plan: ADI-199 User navigation: cursor, highlighting, status line, command line

Linear: https://linear.app/aditya-bhargava/issue/ADI-199
Sub-issues: ADI-212 (cursor + scroll), ADI-213 (highlighting + flicker-free), ADI-214 (status line + help overlay), ADI-215 (`:` command line), ADI-216 (cell detail + record view).
Depends on: ADI-198 merged.

## Goal

The viewer becomes interactive:
- Arrows and vim keys move the cursor, and the view scrolls to keep it visible.
- The cursor cell, row and column are highlighted.
- The status line shows the position and key hints.
- `?` shows help, `:` opens a command line, `Enter` shows cell detail, and `r` shows the record view.
- Colors respect `NO_COLOR`. No flicker.

## Context for the executing agent

- Read `SPEC.md`: "Viewer" (all), and "Changes after review" for the command line decision.
- All state changes go in `viewer/app.rs`. `render.rs` stays pure. `layout.rs` stays free of ratatui types.
- Build on what ADI-198 left, and don't duplicate it:
  - `layout::fit` is the only code that sizes text.
  - `Column::display_name` is how a header reads.
  - `render::cell_style` is the one place that decides a body cell's style.
  - `render::grid_line` lays out every grid row.
- Five commits, one per sub-issue, in the order below. Each must pass `make check`. This is a binary crate, so clippy's `dead_code` check fails on anything a commit adds but doesn't use yet. Add each type, field and variant in the commit that first uses it.
- Flicker: ratatui keeps a double buffer and writes only the cells that changed. Draw the full frame on every loop iteration and never call `terminal.clear()`. Don't sleep in the loop; `event::poll(TICK)` is the only wait.
- Accept `KeyEventKind::Repeat` as well as `Press`, so holding a key repeats on terminals that report repeats separately.

## Part 1: Cursor and scrolling (ADI-212)

### One source for the screen layout

Today `render::draw` builds the border and the vertical `Layout` inline. Move that into one function that both `draw` and the event loop call:

```rust
pub struct Areas { pub header: Rect, pub body: Rect, pub divider: Rect, pub status: Rect }

/// Where each part of the screen goes. `draw` uses it, and the event loop uses
/// it to tell `App` how big the grid is, so the two can't disagree.
pub fn areas(screen: Rect) -> Areas;
```

Each loop iteration, before drawing, the event loop calls `app.set_viewport(width, height)` with the grid's size, taken from `areas(terminal.size())`: the header's width (the inner width) and the body's height. This replaces the old plan's `layout::body_rows`, which would have repeated render's row arithmetic. `set_viewport` stores the size and calls `scroll_into_view`, so a resize keeps the cursor visible.

### State

```rust
pub struct Cursor { pub row: usize, pub col: usize }   // indices into table.rows / table.columns
pub struct Viewport { pub width: u16, pub height: u16 } // grid size in cells, from the last frame

pub struct App {
    // existing fields
    pub cursor: Cursor,
    pub first_row: usize,   // vertical scroll offset
    pub first_col: usize,   // index of the first non-pinned column shown
    viewport: Viewport,     // set only through set_viewport
}

impl App {
    /// The grid's columns for the current scroll and viewport. Render draws
    /// these, and scroll_into_view checks them, so both agree on what's on screen.
    pub fn visible_columns(&self) -> Vec<VisibleColumn>;
}
```

### Keys: one table

Keys live in a table of bindings, not in `match` arms spread across `handle_key`. `handle_key` looks up the action, and the help overlay (Part 3) prints the same table, so the overlay can't list a key that does nothing or miss one that works.

```rust
pub struct Key { pub code: KeyCode, pub ctrl: bool }
pub struct Binding<A> { pub keys: &'static [Key], pub action: A, pub help: &'static str }

pub enum Action { Down, Up, Left, Right, PageDown, PageUp, FirstRow, LastRow, FirstCol, LastCol, Quit }

pub const GRID_BINDINGS: &[Binding<Action>] = &[ /* … */ ];

fn lookup<A: Copy>(bindings: &[Binding<A>], key: KeyEvent) -> Option<A>;
fn key_label(key: Key) -> String;   // "↓", "PgDn", "Ctrl-f", "j"
```

Match `Char` keys on the character alone and ignore Shift, because terminals send `G` and `$` with or without the Shift modifier. Later parts add variants (`Help`, `CommandLine`, `Detail`, `Record`) and rows to the table.

| Key | Action |
|---|---|
| `↓`/`j`, `↑`/`k` | row ±1 |
| `→`/`l`, `←`/`h` | col ±1 |
| `PgDn`/`Ctrl-f`, `PgUp`/`Ctrl-b` | page down / up |
| `g` / `G` | first / last row |
| `0` / `$` | first / last column |
| `q`, `Ctrl-c` | quit |

Ctrl-c also quits from every mode, so it is checked before the per-mode dispatch.

### Movement and scroll rules

- Every move clamps the cursor to `rows.saturating_sub(1)` and `columns.saturating_sub(1)`, so empty tables and zero-column tables can't underflow, and then calls `scroll_into_view`.
- Page down moves both the cursor and `first_row` down by one page (`viewport.height`, at least 1). `first_row` is clamped to `rows - height`. Page up is the mirror image. Like vim's `Ctrl-f`, the view jumps by whole pages.
- `scroll_into_view`, vertical: if `cursor.row < first_row`, set `first_row = cursor.row`. If `cursor.row >= first_row + height`, set `first_row = cursor.row - height + 1`.
- `scroll_into_view`, horizontal: if the cursor is on the pinned column, no change. Otherwise, if `cursor.col < first_col`, set `first_col = cursor.col`. Then, while no entry of `self.visible_columns()` has `index == cursor.col`, increment `first_col`. The loop ends because `visible_columns` always includes the first non-pinned column.

### Render

- The grid uses `app.visible_columns()`.
- `cell_style` gives the cursor cell `REVERSED`, so the cursor is visible in this commit; Part 2 replaces that with the theme.
- The status line is unchanged until Part 3.

### Tests

Unit tests on `App` with a 50-row, 10-column synthetic table and a 60×10 viewport:
- Page down from row 0 goes to row 10 with `first_row` 10.
- `G` goes to row 49 with `first_row` 40.
- `$` goes to the last column, with `first_col` advanced so it's visible.
- `h` from column 0 does nothing.
- A cursor on the pinned column never changes `first_col`.
- Shrinking the viewport keeps the cursor visible.
- Moves on an empty table and on a zero-column table do nothing and don't panic.
- `lookup` matches `G` with and without Shift.

## Part 2: Highlighting and colors (ADI-213)

### Theme (`viewer/theme.rs`)

```rust
pub struct Theme {
    pub header: Style,
    pub cursor_cell: Style,
    pub cursor_line: Style,   // the cursor's row and column (one tier, so one style)
    pub null: Style,
}

impl Theme {
    pub fn color() -> Self;  // cursor_cell REVERSED|BOLD, cursor_line bg Indexed(236), null DIM, header BOLD
    pub fn mono() -> Self;   // cursor_cell REVERSED, cursor_line BOLD, null DIM, header BOLD
    pub fn detect() -> Self; // NO_COLOR set and non-empty (per no-color.org) → mono, else color
}
```

`App` has a `pub theme: Theme` field. `App::new` uses `Theme::color()`, and `viewer::run` sets `Theme::detect()`. Tests set the theme directly and never read the environment.

### Render changes

- **`cell_style`** (the existing one place): use `cursor_cell` if the cell is the cursor; otherwise `cursor_line` if it's on the cursor's row or column; otherwise plain. Then patch `null` on top, so a dimmed null keeps its highlight.
- **`grid_line`** takes a `row_style` for the separators, so a highlighted row is continuous instead of striped at each `│`. The header passes the plain style.
- **Header** cells use `theme.header` instead of a hard-coded `BOLD`.

### Style-aware snapshots

`TestBackend`'s text output has no styles, and we don't use `insta`. Extend `src/snapshot.rs` with a `buffer_view(&Buffer) -> String` that prints the text lines, then a `--- styles` section listing each line's runs of non-default style, for example `row 2: 1..9 REVERSED | BOLD, 10..62 bg=Indexed(236)`. Render tests snapshot this instead of the plain text, so highlight changes show up in `git diff`. It replaces ADI-198's single bold-and-dim assertion test. Comment markers and change highlights in later tickets reuse it.

### Tests

Snapshots:
- Cursor at (0,0).
- Cursor at the bottom-right after `G` and `$`.
- Cursor mid-table.
- The mid-table position with `Theme::mono()`.

## Part 3: Status line and help overlay (ADI-214)

### Modes

```rust
pub enum Mode { Grid, Help }   // Part 4 adds Command(CommandLine); Part 5 adds Detail and Record
```

`handle_key` checks Ctrl-c first, then dispatches on `self.mode`. In Help mode any key closes the overlay.

### Status line

```
 row 2/38  col 4/9  ·  ? help  : command  q quit
 row 2/38  col 4/9  ·  unknown command: frob        (while a message is showing)
```

- **Left:** `row {cursor.row+1}/{rows}  col {cursor.col+1}/{cols}`, or `no rows` for an empty table.
- **After it:** fixed hints, or the transient message while one is showing. The message takes the place of the hints rather than being right-aligned, so the two can never overlap on a narrow screen.
- **Message storage:** `message: Option<Message { text, shown_at: Instant }>`, set with `App::set_message(text)`.
- **Clearing:** the event loop calls `App::tick(Instant::now())` on every iteration. `tick` takes the time as an argument, so tests can pass a time 4s later instead of sleeping. It clears messages older than `MESSAGE_DURATION` (3s).
- **Which commit:** the message slot and `tick` land in Part 4, with the first thing that sets a message, so Part 3 adds nothing unused.

### Help overlay

`?` sets `Mode::Help`. Render a centered popup (`Rect::centered`, `Clear` under it, a bordered `Block` titled `Keys`). It lists every row of `GRID_BINDINGS` as `{labels}  {help}`, with the labels from `key_label`. Part 4 adds the `:` commands under it.

### Tests

- `?` opens Help and any key closes it.
- A snapshot of the help overlay. (The message tests are in Part 4.)

## Part 4: Command line (ADI-215)

### State

```rust
/// `cursor` counts characters, not bytes, so typing `é` can't split a UTF-8
/// sequence. Convert to a byte offset with `char_indices` only at the edit.
pub struct CommandLine { pub text: String, pub cursor: usize }

pub enum ColonCommand { Help, Quit, Write(Option<String>), Unknown(String) }

impl std::str::FromStr for ColonCommand {
    type Err = std::convert::Infallible;   // anything unrecognised is `Unknown`
    // "help" | "h" → Help, "q" | "quit" → Quit, "w [path]" | "write [path]" → Write, else Unknown
}

impl ColonCommand {
    /// Listed in the help overlay. Kept next to `from_str` so the two are edited together.
    pub const HELP: &[(&str, &str)];
}
```

The ticket asks for `FromStr` so that adding a command is one match arm. It's named `ColonCommand` to avoid confusion with clap's `Command` in `main.rs`.

### Behavior

- `:` in Grid enters `Mode::Command` with an empty line. The status line is replaced by `:` + text, with the terminal cursor placed using `frame.set_cursor_position`. Its x is the status area's x, plus 1 for the `:`, plus `display_width` of the text before the cursor.
- Printable characters insert at the cursor; `Backspace` deletes before it; `←`/`→` move; `Home`/`End`; `Ctrl-u` clears.
- `Enter` runs the parsed command, unless the text is blank, in which case it just closes the line. `Esc` cancels.
- `Help` → `Mode::Help`. `Quit` → `should_quit`. `Unknown(text)` → message `unknown command: {text}`.
- `Write(path)` → a message saying save isn't implemented yet and naming the path. ADI-218 fills in the body.

### Tests

- A table test for `ColonCommand::from_str`.
- `:`, `q`, `Enter` quits. `:`, `Esc` goes back to Grid with no side effects.
- Editing with multi-byte input (`é`, `日本`), including Backspace and ← in the middle.
- A snapshot of the command line mid-edit, including the cursor position.
- A message clears after `tick` at +4s and stays at +1s; a snapshot of the status line with a message.

## Part 5: Cell detail and record view (ADI-216)

Both views read the grid cursor directly. Record view moves `cursor.col` between fields and `cursor.row` between rows, so there's no separate `RecordState` to keep in sync, and going back to the grid needs no syncing. `Mode` gains `Detail` and `Record` as plain variants.

### `Table::row_id`

Add `row_id(index) -> String`, deferred from ADI-198. It returns `cell_text` of the key value, or `#<index>` without a key. Tests: with and without a key, and an integer key renders as `412`.

### Cell detail (`Enter`)

`Mode::Detail` shows a centered popup, about 80% × 60% of the screen, with `Clear` under it:
- **Title:** `{display_name} · row {row_id}`.
- **Body:** strings exactly as they are, with newlines kept; nested JSON via `serde_json::to_string_pretty`; numbers and bools as written; null or missing as a dimmed `null`. Wrap with `Wrap { trim: false }`.
- **Comments area:** reserved at the bottom, with the heading `Comments` and the text `none yet`. ADI-224 fills it.
- **Keys:** `Esc`, `Enter` or `q` closes it.
- **Empty table:** `Enter` sets the message `no rows`.

### Record view (`r`)

`Mode::Record` replaces the header and body areas; the title and status bar stay:
- **Layout:** one line per column. The name is right-aligned with `fit` to the width of the longest `display_name`, capped at `DEFAULT_WIDTH_CAP`. Then comes `│`, then the value, sized with `fit` to the remaining width.
- **Styles:** the current field's line uses `theme.cursor_line`, its value uses `theme.cursor_cell`, and nulls use `theme.null`.
- **Scrolling:** if there are more fields than lines, the list scrolls so the current field stays visible. That's computed in render from the cursor, so it needs no state.
- **Status line:** `row {n}/{total}  field {f}/{cols}  ·  j/k field  n/p row  Esc back`.
- **Keys:** a `RECORD_BINDINGS` table, which the help overlay also lists in its own section. `j`/`k`/`↓`/`↑` move the field (the cursor column), `n`/`p` move the row, and `Esc`/`r`/`q` go back to the grid. Moves use the same clamp and `scroll_into_view` as the grid, so the grid is already scrolled to the cursor when you return.
- **Empty table:** `r` sets the message `no rows`.

### Tests

- Snapshots: detail with a long wrapped string, detail with a nested object, and the record view mid-table.
- An `App` test: record view `n` and `j`, then `Esc`, leaves the grid cursor on the same cell.

## Flicker and resize check (manual)

- Hold `j` for a few seconds on `examples/prs.json`. The grid should scroll smoothly, with no blank frames.
- Resize the terminal to very narrow (30 columns) and back.
- Open and close the `?` and `Enter` popups. Nothing should be left on screen (that's what `Clear` is for).

## Acceptance

```sh
make check
make run    # exercise every key in the help overlay plus : commands
NO_COLOR=1 make run   # no colors, cursor still obvious
```

## Deliverables

- `src/viewer/app.rs`: `Cursor`, `Viewport`, `Mode`, `Key`/`Binding`/`Action` tables, `CommandLine`, `ColonCommand`, `tick`, `scroll_into_view`, key handling
- `src/viewer/theme.rs`: `Theme`
- `src/viewer/render.rs`: `areas`, grid with styles, status line, help overlay, command line, detail popup, record view
- `src/viewer/mod.rs`: viewport set before each draw, `tick` every iteration
- `src/snapshot.rs`: `buffer_view` with style runs
- `src/table.rs`: `row_id`
- Snapshots for each view
- Five commits, in sub-issue order

## Out of scope

`:w` implementation (ADI-218), comments in detail view (ADI-224), change highlighting after reload (ADI-221), `goto.json` handling (ADI-228).
