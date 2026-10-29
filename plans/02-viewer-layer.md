# Plan: ADI-198 Viewer layer: schema and basic grid

Linear: https://linear.app/aditya-bhargava/issue/ADI-198
Sub-issues: ADI-209 (schema + `table.rs`), ADI-210 (`layout.rs`), ADI-211 (static grid render).
Depends on: ADI-197 (Rust setup) merged.

## Goal

`tv open examples/prs.json` draws a static, correctly laid out grid: title bar, bold header, rows, truncation with `…`, right-aligned numbers, dimmed nulls, pinned key column. `q` quits. No cursor yet. Everything is unit- and snapshot-tested.

## Context for the executing agent

- Read `SPEC.md`: "Table format", "Viewer / Layout", "Viewer / Rendering", "Implementation".
- Three modules, built in order: `table.rs` (data), `viewer/layout.rs` (pure math), `viewer/render.rs` + `viewer/mod.rs` (drawing). Keep `render` pure: it takes `&App` and a `Frame`, never mutates.
- Check the ratatui version in `Cargo.lock` and read its docs for that version. The ratatui API changed between 0.2x releases (e.g. `Frame::size()` vs `Frame::area()`, `Terminal::draw` closure signature). Do not assume from memory.
- Work in three commits matching the three sub-issues so each can be reviewed alone.

## Part 1: `src/table.rs` (ADI-209)

### Types

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

// Every `Table` that exists has been normalized and validated: serde builds a
// `RawTable` and then runs `TryFrom<RawTable>` (inference + validation). No caller
// can get a `Table` that skipped those steps, and no caller has to remember them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "RawTable")]
pub struct Table {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default)]
    pub columns: Vec<Column>,
    pub rows: Vec<serde_json::Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Value>,   // opaque until ADI-223 defines the type
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ColumnType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColumnType { String, Number, Bool }
```

`RawTable` is a private struct with the same fields and serde attributes as `Table` (minus `try_from`). It exists only as the input to `TryFrom`.

Keep rows as `serde_json::Map` rather than a custom `Value` enum. Cells are looked up by column name; `serde_json::Value` already covers string/number/bool/null/nested. This avoids a conversion layer.

### Functions

- `Table::parse(text: &str) -> anyhow::Result<Table>`: parse to a `serde_json::Value`. If the top-level value is an array, wrap it as `{ "rows": <array> }` (the shorthand). Then `serde_json::from_value::<Table>`, which runs `TryFrom<RawTable>`. (Don't use `#[serde(untagged)]` for the shorthand; its error messages lose the detail.)
- `Table::from_reader` / `Table::from_path` convenience wrappers.
- `impl TryFrom<RawTable> for Table` (error type `String`, which serde wraps into its error):
  1. If `columns` is empty, infer: iterate rows, collect object keys in first-seen order (a `Vec` plus a `HashSet` for the seen check; no `indexmap` dependency).
  2. For each column with `kind == None`, infer from values: all non-null values are numbers → `Number`; all bools → `Bool`; otherwise `String`. All-null → `String`.
  3. Validate `key`: if set, the column must exist in `columns`; every row must have a non-null value for it; values must be unique when rendered via `cell_text`. Error messages name the offending row index and value, e.g. `duplicate key value "412" at rows 3 and 7`.
- `Table::row_id(&self, index: usize) -> String`: `cell_text` of the key value (the same rendering the uniqueness check uses, so the two can't disagree), or `#<index>` when there is no key. `serde_json` already prints the integer `412` as `412`.
- `Table::cell(&self, row: usize, col: &str) -> Option<&Value>`.
- `pub fn cell_text(value: Option<&Value>) -> String`: grid rendering of a cell. `None` or `Null` → empty string (render decides dimming); strings as-is with `\n` replaced by `↵`; numbers via `Value::to_string`; bools `true`/`false`; objects and arrays via `serde_json::to_string` (compact).
- `Table::key_column_index(&self) -> Option<usize>`.

### Tests (in `table.rs`, `#[cfg(test)]`)

- Full-format parse round-trips through serde (parse → to_string → parse gives equal columns/rows).
- Shorthand array parses and infers columns in first-seen order across rows with different key sets.
- Type inference: numbers, bools, mixed → string, all-null → string.
- `key` pointing at a missing column errors.
- Duplicate key values error and the message mentions both row indices.
- Missing key value in one row errors.
- `row_id` with and without key; integer key renders as `412`, not `412.0`.
- `cell_text` for null, nested object, string with newline.

Also write `docs/schema.md`: a human-readable copy of the "Table format" section of SPEC.md with the inference and validation rules spelled out. This is what `tv agent help` will point agents at later.

## Part 2: `src/viewer/layout.rs` (ADI-210)

Pure functions. No ratatui types in signatures, only numbers.

```rust
pub const DEFAULT_WIDTH_CAP: u16 = 40;
pub const CELL_PADDING: u16 = 1; // one space each side

/// Display width of a string, measured with unicode-width.
pub fn display_width(s: &str) -> u16;

/// Width of every column: min(cap, max(header, widest cell)).
pub fn column_widths(table: &Table) -> Vec<u16>;

pub const SEPARATOR: &str = "│";

/// Screen width one column takes: padding on both sides plus the separator.
/// `visible_columns` budgets with this and render draws with the same constants,
/// so the two can't disagree.
pub fn column_span(width: u16) -> u16;

/// Column indices to draw, left to right: the pinned column (if any) first,
/// then the non-pinned columns starting at `first_col` that fully fit in
/// `total_width`. Columns are never partially shown.
pub fn visible_columns(
    widths: &[u16],
    pinned: Option<usize>,
    first_col: usize,
    total_width: u16,
) -> Vec<usize>;

pub enum Align { Left, Right }

/// Make `s` exactly `width` display cells: truncate with `…` if it's too long,
/// then pad on the side `align` says. This is the only place cell text is sized.
pub fn fit(s: &str, width: u16, align: Align) -> String;
```

Rules:

- `column_widths` measures `cell_text` of every cell with `display_width`, never `.len()`. Cap is `column.width.unwrap_or(DEFAULT_WIDTH_CAP)`. Minimum width is 1 so the `…` fits.
- `fit` must handle wide chars (CJK, emoji) that would straddle the boundary: drop the whole char, so the padding brings the output to exactly `width`.
- `visible_columns`: the pinned column (if any) is always first. The rest are the columns from `first_col` onward with the pinned one filtered out, taken while they fit. If even the pinned column does not fit, return it alone; render clips it.

### Tests

- Cap applies; header wider than cells wins; explicit `width` overrides default cap.
- `display_width("日本語") == 6`, emoji width per unicode-width.
- `fit("hello world", 5, Left) == "hell…"`, `fit("日本語", 3, Left) == "日…"` (width exactly 3), `fit("日本語", 4, Left) == "日… "` (the wide char that straddles is dropped, then padded), `fit("12", 4, Right) == "  12"`.
- `visible_columns` with pinned column and narrow terminal: pinned is first; the list stops before the first column that would be partial; `first_col` equal to the pinned index doesn't draw it twice.
- Table with no rows: widths equal header widths.

## Part 3: `src/viewer/mod.rs`, `app.rs`, `render.rs` (ADI-211)

### `app.rs`

Minimal for this ticket. Later tickets add cursor, modes, highlights.

```rust
pub struct App {
    table: Table,
    widths: Vec<u16>,           // always layout::column_widths(&table)
    pub session_id: Option<String>, // None until sessions exist; title bar shows nothing on the right
    pub first_col: usize,       // horizontal scroll, fixed at 0 for now
    pub first_row: usize,       // vertical scroll, fixed at 0 for now
    pub should_quit: bool,
}

impl App {
    pub fn new(table: Table) -> Self;
    /// The only way to change the table, so `widths` can never go stale.
    pub fn set_table(&mut self, table: Table);
    pub fn table(&self) -> &Table;
    pub fn widths(&self) -> &[u16];
    pub fn handle_key(&mut self, key: KeyEvent); // `q` and Ctrl-C quit for now
}
```

`table` and `widths` are private because `widths` is derived from `table`. If both were public, every later ticket that reloads the table would have to remember to recompute the widths in the right order.

### `render.rs`

`pub fn draw(frame: &mut Frame, app: &App)`. Layout with `ratatui::layout::Layout`, vertical: title bar (1 row with top border), header (1), body (fill), status bar (1 with bottom border). Use a single `Block` with borders around everything and the title in the top border, left-aligned, and the session id (when there is one) right-aligned in the top border, as in the SPEC layout sketch.

Grid body. Split the "what" from the "how" so later tickets (cursor, markers, highlights) only change the "what":

- Compute `visible = layout::visible_columns(app.widths(), key_idx, app.first_col, inner_width)` once per frame.
- **What:** `fn cell_style(app: &App, row: usize, col: usize) -> Style` is the one place that decides a body cell's style. For now: `DIM` when the value is null (visible later when the cursor lands on it; harmless now). ADI-212's cursor and row styles go here, with nothing else changing.
- **How:** `fn grid_line(cells: impl Iterator<Item = (String, Align, Style)>, visible: &[usize], widths: &[u16]) -> Line` is the only code that pads, calls `layout::fit`, and inserts `layout::SEPARATOR`. The header row and every body row both go through it. The header passes column names with `BOLD`; body rows pass `cell_text`, alignment from the column type (`Number` → `Right`, else `Left`), and `cell_style`.
- Use manual `Line`/`Span`s rather than `ratatui::widgets::Table`. That gives exact control over per-cell styles, which later tickets need.
- Rows beyond the body height are not drawn (no vertical scroll yet; `first_row` stays 0).
- Empty table: single line `No rows` under the header.

Status bar for now: `rows N  cols M  ·  q to quit`.

Colors: use only `Modifier::BOLD`, `DIM`, `REVERSED` in this ticket. No colors yet, so `NO_COLOR` is trivially respected; ADI-213 adds colors and the `NO_COLOR` check.

### `mod.rs`

```rust
pub fn run(table: Table) -> anyhow::Result<()>
```

- `let mut terminal = ratatui::init();` This enables raw mode, enters the alternate screen, and installs a panic hook that restores the terminal, so a crash never leaves the shell in raw mode. Don't write that setup by hand.
- Run the loop in a separate function and call `ratatui::restore()` after it returns, whether it returned `Ok` or `Err`.
- Loop: `terminal.draw(|frame| render::draw(frame, &app))`; `event::poll(TICK)` with `const TICK: Duration = Duration::from_millis(250);` (the spec's poll interval, reused by ADI-218's reload). On `Event::Key` call `app.handle_key`. On `Event::Resize` do nothing special; the next draw uses the new size. Break when `app.should_quit`.
- Use `ratatui::crossterm::event`, not a direct `crossterm` import (ADI-197 removed the direct dependency).

### `main.rs`

`main` goes back to returning `anyhow::Result<()>` (see the comment ADI-197 left there). `Command::Open { path }` → `let table = Table::from_path(&path)?;` → `viewer::run(table)`. Parse errors come back as `Err` before the TUI starts. Returning `Err` from `main` prints `Error: …` to stderr and exits 1, so you don't need manual error handling.

### Snapshot tests (`tests/render.rs` or in `render.rs`)

No `insta` (we keep dependencies few). Write a small helper instead, `src/snapshot.rs` behind `#[cfg(test)]`:

```rust
/// Compare `actual` to `src/viewer/snapshots/<name>.txt`. With
/// `UPDATE_SNAPSHOTS=1`, write the file instead and pass.
pub fn assert_snapshot(name: &str, actual: &str);
```

- Build the path from `env!("CARGO_MANIFEST_DIR")` so it works from any working directory.
- A missing file fails with a message saying to run `make snapshots`.
- A mismatch fails through `assert_eq!`, which prints both versions.
- Add `snapshots:` to the Makefile (`UPDATE_SNAPSHOTS=1 cargo test`) and to its `.PHONY` line.

Render with `ratatui::backend::TestBackend` and pass `terminal.backend().to_string()` to the helper. Keep the tests in `render.rs`'s test module:

- `examples/prs.json` at 100×20.
- Same at 50×12 (truncation and column clipping).
- A table with a key whose first column is wide and the terminal is narrow, to show the pinned column surviving.
- CJK and emoji cells at a narrow width.
- Empty table.

Run `make snapshots` once, eyeball every `.txt` file for alignment, and commit them. After that, `git diff` is the review step whenever a snapshot changes.

## Acceptance

```sh
make check                       # all green
make run                         # grid appears, q quits, shell is restored
cargo run -- open examples/prs.json; echo $?   # 0 after q
echo 'not json' > /tmp/bad.json && cargo run -- open /tmp/bad.json; echo $?  # error on stderr, exit 1, no TUI flash
```

Resize the terminal while the viewer is open: the grid redraws to the new width with no artifacts.

## Deliverables

- `src/table.rs`, `docs/schema.md`
- `src/viewer/{mod.rs,app.rs,render.rs,layout.rs}`
- `src/main.rs` updated
- `src/snapshot.rs` and snapshot files under `src/viewer/snapshots/`
- Three commits: `table: schema, parsing, inference, validation`, `layout: column widths and visible range`, `viewer: static grid render`

## Out of scope

Cursor, highlighting, scrolling by key, status line details, command line (ADI-199). Sessions (ADI-201).
