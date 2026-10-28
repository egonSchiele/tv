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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

Keep rows as `serde_json::Map` rather than a custom `Value` enum. Cells are looked up by column name; `serde_json::Value` already covers string/number/bool/null/nested. This avoids a conversion layer.

### Functions

- `Table::parse(text: &str) -> anyhow::Result<Table>`: parse JSON. If the top-level value is an array, wrap it as `{ "rows": <array> }` (the shorthand). Then call `normalize`.
- `Table::from_reader` / `Table::from_path` convenience wrappers.
- `fn normalize(&mut self) -> anyhow::Result<()>`:
  1. If `columns` is empty, infer: iterate rows, collect object keys in first-seen order (use an `IndexSet` or a `Vec` + `HashSet`).
  2. For each column with `kind == None`, infer from values: all non-null values are numbers → `Number`; all bools → `Bool`; otherwise `String`. All-null → `String`.
  3. Validate `key`: if set, the column must exist in `columns`; every row must have a non-null value for it; values must be unique when rendered via `cell_text`. Error messages name the offending row index and value, e.g. `duplicate key value "412" at rows 3 and 7`.
- `Table::row_id(&self, index: usize) -> String`: key value rendered as text (numbers without trailing `.0`; strings as-is), or `#<index>` when there is no key.
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

pub struct VisibleColumns {
    pub pinned: Option<usize>,      // key column index, drawn first
    pub scrolled: std::ops::Range<usize>, // other columns fully visible after `first_col`
}

/// Which columns fit in `total_width` given a horizontal scroll offset
/// `first_col` (index of the first non-pinned column to show). Accounts for
/// separators (1 char) and padding. Columns are never partially shown.
pub fn visible_columns(
    widths: &[u16],
    pinned: Option<usize>,
    first_col: usize,
    total_width: u16,
) -> VisibleColumns;

/// Truncate `s` to `width` display cells, ending with `…` if cut.
pub fn truncate(s: &str, width: u16) -> String;
```

Rules:

- `column_widths` measures `cell_text` of every cell with `display_width`, never `.len()`. Cap is `column.width.unwrap_or(DEFAULT_WIDTH_CAP)`. Minimum width is 1 so the `…` fits.
- `truncate` must handle wide chars (CJK, emoji) that would straddle the boundary: drop the whole char and pad with a space if needed so output width is exactly `width`.
- `visible_columns`: pinned column (if any) always included and excluded from the scrolled range. If even the pinned column does not fit, show it alone, clipped by render. If `first_col` is the pinned index, skip to the next.

### Tests

- Cap applies; header wider than cells wins; explicit `width` overrides default cap.
- `display_width("日本語") == 6`, emoji width per unicode-width.
- `truncate("hello world", 5) == "hell…"`, `truncate("日本語", 3) == "日…"` (width exactly 3), no-op when it fits.
- `visible_columns` with pinned column and narrow terminal: pinned always present; scrolled range stops before the first column that would be partial.
- Table with no rows: widths equal header widths.

## Part 3: `src/viewer/mod.rs`, `app.rs`, `render.rs` (ADI-211)

### `app.rs`

Minimal for this ticket. Later tickets add cursor, modes, highlights.

```rust
pub struct App {
    pub table: Table,
    pub widths: Vec<u16>,       // from layout::column_widths, recomputed when table changes
    pub session_id: Option<String>, // None until sessions exist; title bar shows "file" or nothing
    pub first_col: usize,       // horizontal scroll, fixed at 0 for now
    pub first_row: usize,       // vertical scroll, fixed at 0 for now
    pub should_quit: bool,
}

impl App {
    pub fn new(table: Table) -> Self;
    pub fn handle_key(&mut self, key: KeyEvent); // only `q` for now
}
```

### `render.rs`

`pub fn draw(frame: &mut Frame, app: &App)`. Layout with `ratatui::layout::Layout`, vertical: title bar (1 row with top border), header (1), body (fill), status bar (1 with bottom border). Use a single `Block` with borders around everything and the title in the top border, left-aligned, and the session id (or file name) right-aligned in the top border, as in the SPEC layout sketch.

Grid body:

- Compute `visible = layout::visible_columns(&app.widths, key_idx, app.first_col, inner_width)`.
- Draw with `ratatui::widgets::Table` if its column/constraint API maps cleanly; otherwise draw rows manually with `Paragraph`/`Line`/`Span` per row. Manual spans give exact control over per-cell styles, which later tickets need (cursor, markers, highlights), so prefer manual spans from the start.
- Header: bold. Separator `│` between columns.
- Cell: `layout::truncate(cell_text, width)`, padded to width; numbers (`ColumnType::Number`) right-aligned, others left. Null cells rendered as empty with `Modifier::DIM` applied (visible later when cursor lands on them; harmless now).
- Rows beyond the body height are not drawn (no vertical scroll yet; `first_row` stays 0).
- Empty table: single line `No rows` under the header.

Status bar for now: `rows N  cols M  ·  q to quit`.

Colors: use only `Modifier::BOLD`, `DIM`, `REVERSED` in this ticket. No colors yet, so `NO_COLOR` is trivially respected; ADI-213 adds colors and the `NO_COLOR` check.

### `mod.rs`

```rust
pub fn run(table: Table) -> anyhow::Result<()>
```

- Enable raw mode, enter alternate screen, create `Terminal<CrosstermBackend<Stdout>>`.
- Install a panic hook that restores the terminal before printing the panic, so a crash never leaves the shell in raw mode.
- Loop: `terminal.draw(|f| render::draw(f, &app))`; `event::poll(Duration::from_millis(250))`; on `Event::Key` call `app.handle_key`; on `Event::Resize` do nothing special (next draw uses the new size); break when `app.should_quit`.
- Restore terminal on exit (also on `Err`).

### `main.rs`

`Command::Open { path }` → `Table::from_path(&path)?` → `viewer::run(table)`. On parse error print the anyhow error to stderr and exit 1 (no TUI).

### Snapshot tests (`tests/render.rs` or in `render.rs`)

Use `ratatui::backend::TestBackend` + `insta::assert_snapshot!(terminal.backend())`:

- `examples/prs.json` at 100×20.
- Same at 50×12 (truncation and column clipping).
- A table with a key whose first column is wide and the terminal is narrow, to show the pinned column surviving.
- CJK and emoji cells at a narrow width.
- Empty table.

Run `cargo insta review` or `cargo insta accept` once, commit the `.snap` files, and eyeball every snapshot for alignment before accepting.

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
- Snapshot files under `src/viewer/snapshots/` or `tests/snapshots/`
- Three commits: `table: schema, parsing, inference, validation`, `layout: column widths and visible range`, `viewer: static grid render`

## Out of scope

Cursor, highlighting, scrolling by key, status line details, command line (ADI-199). Sessions (ADI-201).
