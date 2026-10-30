# Plan: input formats

Linear: none yet. File a parent issue before this runs if it should be tracked; each Part below would be a sub-issue.

Depends on: `main` at `d884b5c` (PR #11, which moved the spec to `docs/SPEC.md` and added README's Install and Quick start). It doesn't depend on the rest of ADI-204 (publish). Both this plan and `plans/10-publish.md` edit `README.md`, so whichever merges second rebases that file.

## Goal

tv opens the data people already have, not only its own JSON format:

- **CSV and TSV:** spreadsheet exports, `psql --csv`, `sqlite3 -csv`, cells copied from Excel or Google Sheets (the clipboard holds TSV, so `pbpaste | tv -` works).
- **More JSON:** JSON Lines (structured logs, `docker ps --format json`), objects one after another (`jq '.items[]'`), arrays one after another (`gh api --paginate`), wrapped lists (`kubectl get -o json`, `{"items": [...]}`), and a single record.
- **Markdown tables:** the tables agents print in chat, and README tables. A wide table an agent printed wraps and becomes unreadable, which is the problem tv exists to solve.

Each format has a converter that turns the input into tv's JSON table format. Everything after that is the code tv already has: inference, key validation, sessions, the viewer, live reload, comments, `get` and `save`. None of it changes.

## Context for the executing agent

- Read `docs/SPEC.md`: "Not doing" (its "CSV / other input formats" row is what this reverses), "Table format", "Human CLI", `tv agent new` and `push`, and "Module layout". Read `docs/schema.md` too.
- **The seam.** Every input goes through `table.rs`:
  - `tv open <file>` → `Table::from_path`
  - `tv -` → `Table::parse`
  - `tv agent new` / `push` → `Table::parse_with` and `Table::from_path_with`
  - A session's `table.json` → `Table::from_path` (in `Session::load_table`)

  This plan adds `src/formats.rs`, whose `to_json` turns any input into the JSON value `table.rs` already reads. It splits `table.rs`'s entry points into strict ones for `table.json` and input ones for everything users and agents give (Decision 1).
- Build on what's there:
  - `table.rs`: the `parse_with` body becomes `from_json`, unchanged except that it takes a `Value`. `infer_type`'s rule becomes `values_type`, which CSV's typing reuses (Part 2).
  - `files.rs`: `write_json`, which `Table::save` uses. Reading gets a partner there, `files::read_text`.
  - `tests/cli.rs`: `tv`, `agent`, `new_session`, `assert_fails_with` and `stderr`.
- **No new crates.** The CSV and Markdown parsers are hand-written (Decision 5). `cargo tree --locked -e normal --prefix none | sort -u | wc -l` stays at 89, and README's `-e normal,build,dev` count stays at 96.
- **No deletes.** Nothing here removes files. `clippy.toml` doesn't change.
- **Five code commits.** `dead_code` fires in a binary crate, so everything a commit adds needs a non-test caller in that commit. Each Part below only adds functions its own `to_json` branch calls.
- **Scratch check.** I built this in a scratch copy of `main` at three points, each clean under `cargo clippy --all-targets -- -D warnings` (pedantic) and `cargo fmt --check`. The copy was made at `256bc51`; PR #11 changed no code.
  - Part 1 alone: 287 unit tests and 46 CLI tests pass (`main` has 276 and 45).
  - Parts 1 and 2: 300 and 48.
  - All five Parts: 304 and 49, including the guide tests with Part 5's new `sqlite3` example.

  Parts 1–3 without Part 4 weren't built separately. Part 4 adds five lines, one test and one CLI test. I rebuilt all three points after the anti-pattern check, with the same counts. I also ran the built binary on `jq '.rows[]' examples/prs.json`, two `jq -c` pages back to back, a `kubectl`-style list, a record with two arrays, `jq @csv` output with a BOM and CRLF, a Markdown table, `sqlite3`-style `id|email`, `not json`, an error message, empty input and `[1,2]`. Each did what this plan says.
- **A clippy trap.** In `scalar` (Part 2), `value.to_string() == text` trips `cmp_owned`, and clippy's suggested fix, `value == text`, compiles but compares the value with a JSON *string*, so no number would ever match. The code binds `let written = value.to_string();` first, and a comment says why. Don't take clippy's suggestion.
- **Existing tests keep passing unchanged,** apart from the `parse_with`/`from_path_with` renames in `table.rs`'s tests (Part 1). Two of them now fail for a different reason, and both still pass:
  - `bad_json_on_stdin_exits_1_and_makes_no_session` and `agent_push_rejects_and_keeps_the_table` pipe `not json`. That fails as "not a table" (Decision 3) rather than as bad JSON, still under `parsing stdin`.
  - `other_errors_dont_blame_the_kept_key` passes `5`, which fails as "not a table" too, so the message still doesn't mention the kept key.
  - The tests that write `not json` into `table.json` (`ls_lists_sessions_and_survives_a_broken_one` and others) still see a broken table, because session files stay strict (Decision 1).

## Decisions

1. **Converters produce tv's JSON, and only input goes through them.**
   - `formats::to_json(text)` returns the full format (`{"columns": …, "rows": …}`) or an array of rows, and `table.rs` does the rest as before.
   - `Table::parse` and `Table::from_path` stay strict JSON. They read a session's `table.json`, which tv always writes as JSON. Reading it leniently could only turn a corrupted file into a wrong table, instead of the error `ls` and `sessions` show today.
   - `Table::parse_input` and `Table::from_input_path` (renamed from `parse_with` and `from_path_with`) read any format. `tv open <file>`, `tv -`, `agent new` and `agent push` use them. The new names keep anyone from reaching for `parse_with` and expecting `parse`.
2. **The format is decided by content, not by extension or a flag.** Stdin has no name, and one rule for files and stdin is easier to explain. The first non-blank line decides:
   - `{` or `[` → JSON;
   - `|` → a Markdown table;
   - anything else → CSV, TSV or the like.

   A UTF-8 byte order mark is skipped first: Excel writes one, and today it breaks JSON too. Empty input is an error. A `--format` flag can come later if detection ever guesses wrong (Out of scope).
3. **One line of text with no delimiter isn't a table.** Without this rule, an error message piped in by mistake (`Error: not found`), or a JSON scalar like `5`, would become a one-column session with no rows. Multi-line text, or a line with a comma, still becomes a table; `new` prints `"columns"`, which shows it.
4. **JSON shapes.** In order:
   - **Several values in a row** (JSON Lines, `jq '.[]'` output, `gh api --paginate`'s one-array-per-page output): each object is a row, and each array's items are rows. serde_json's `StreamDeserializer` reads values back to back with or without newlines between them, so pretty-printed objects work too.
   - **One array:** rows, as today.
   - **One object with `rows`:** the full format, as today.
   - **One object without `rows`:** if its *only* array is a non-empty array of objects, those are the rows. That covers `kubectl get -o json` (`items`), the GitHub search API (`items`), Stripe (`data`) and Slack (`members`). Otherwise the object is one row, which the record view (`r`) shows a field per line. Without this rule, a one-line JSON Lines file would fail.

     "Only array" is what keeps records from being unwrapped. A GitHub issue has `labels` and `assignees`, so it's one row, and a repo's `topics` holds strings, so it's one row too. It can still misfire: `gh issue view --json title,labels` has one array and shows the labels. `jq` fixes that, and it beats today's `missing field rows` error.

   Today every object without `rows` is an error, so no input that works today changes meaning.
5. **CSV is parsed by about 40 lines of our own,** following RFC 4180:
   - A field that starts with `"` is quoted. It can hold the delimiter and line breaks, and `""` inside it is one `"`.
   - A `"` anywhere else is an ordinary character. This is the lenient choice the `csv` crate also makes.
   - Lines end in `\n` or `\r\n`. Blank lines are skipped.
   - A quote that's never closed is an error naming the line it opened on.

   The `csv` crate would add 2 crates (`csv`, `csv-core`; 96 → 98 by README's count), with no build scripts. I measured it in a scratch copy. It's by the author of `memchr`, which is already in the tree. It's a fair fallback if edge cases pile up. But the parser is small and well specified, and the Markdown parser has to be ours anyway.
6. **The delimiter is whichever of `,`, tab, `;` and `|` the header line has most of.** A tie goes to the earlier one in that order, and a line with none is one column of CSV.
   - `;` is how European versions of Excel write CSV.
   - `|` is `sqlite3 -header`'s default output.
   - Counting characters in the raw line is a heuristic. A quoted header full of commas in a TSV could fool it, but rarely.
7. **Cell types: a column is numbers only if every non-empty cell writes back as exactly the same text.** CSV cells are all text, so tv decides:
   - Parse the cell as JSON. If it's a number or a boolean whose `to_string()` equals the cell, it converts. `412`, `-2.5`, `19.99` and `true` do. `02134` doesn't (not JSON), nor `1.50` (it would show as `1.5`), `123456789012345678901` (too many digits for a float), `+12` or `TRUE`.
   - All-or-nothing per column: one cell that doesn't convert keeps the whole column as text. So the column's type is consistent, `tv agent get | jq` sees one type per column, and nothing is shown differently from the file.
   - "All one type" is `table.rs`'s own rule for inferring a column's type. It's split out of `infer_type` as `values_type`, so CSV's typing and the table's inference can't disagree.
   - Empty and missing cells are null: dimmed, and ignored by type inference.
   - The cost: a money column written `1.50` is text, so it's left-aligned. Out of scope lists a cheap fix if that bothers anyone.
8. **Header names, short rows and long rows.**
   - Rows are keyed by column name, so an empty header cell becomes `column_<n>` (counting from 1). A repeat gets `_2`, `_3`, …, skipping names already taken, so `a,a,a_2,a` gives `a`, `a_2`, `a_2_2`, `a_3`.
   - Columns are declared from the header, so a header with no rows still shows its columns.
   - A short row's missing cells are null.
   - A row with more cells than the header is an error naming the line, unless the extra cells are empty (a trailing delimiter). Extra cells usually mean the delimiter guess was wrong, and an error says so better than a garbled grid.
   - The first line is always the header. Headerless CSV is Out of scope.
9. **Markdown tables:**
   - A header row, a `|---|` row (with optional `:` for alignment, ignored, since numbers are right-aligned anyway), then rows.
   - Every non-blank line must start with `|`, so text after the table is an error naming the line. The closing `|` is optional. `\|` is a `|` inside a cell.
   - Cells are trimmed, typed as in Decision 7, and formatting such as `**bold**` and backticks stays as written.
   - Differences from GitHub's spec: GFM ignores extra cells in a row, while tv uses Decision 8's rule. GFM also allows a table without a leading `|`; tv reads that as CSV.
10. **A file's name is its title when nothing else gives one.** CSV has nowhere to put a title. Without this, every CSV session is untitled in the viewer, `tv ls` and `tv agent sessions`.
    - Order: `--title`, then the file's own `title` (or its `null`), then a kept session title (`push`), then the file name.
    - It applies to JSON files too, so a bare-array `rows.json` is titled `rows.json`.
    - The viewer's `:w` default follows the title, so it becomes `./sales-csv.json`.
11. **Output stays JSON.** `get`, `save` and `:w` write tv's format, the only one that holds comments, the key and column settings. Writing CSV is Out of scope.

## What this plan defers, and to where

Nothing goes to another issue. "Out of scope" lists what's left for later.

## Part 1: the seam, and more JSON

### `src/formats.rs` (new)

The module doc describes all five Parts, so it's written once.

```rust
//! Input formats: what users and agents hand tv, turned into the JSON that
//! `Table` reads. JSON passes through, with a few more shapes accepted, and
//! JSON Lines, CSV, TSV and Markdown tables become rows. A session stores the
//! result as JSON, so nothing after this knows what the input was. See
//! "Input formats" in `docs/schema.md`.

use anyhow::bail;
use serde_json::{Map, Value};

/// `text` as the JSON `Table` reads: the table format, or an array of rows.
pub fn to_json(text: &str) -> anyhow::Result<Value> {
    // Excel and other Windows tools start UTF-8 files with a byte order
    // mark, which none of the formats expects.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.trim().is_empty() {
        bail!("the input is empty");
    }
    json(text)
}

/// One JSON value, or several in a row: JSON Lines, `jq`'s output, or
/// `gh api --paginate`'s pages. Each object is a row, and each array's
/// items are rows, except that a lone object goes to `one_object`.
fn json(text: &str) -> anyhow::Result<Value> {
    let mut values: Vec<Value> = serde_json::Deserializer::from_str(text)
        .into_iter()
        .collect::<Result<_, _>>()?;
    if let [Value::Object(object)] = values.as_mut_slice() {
        return Ok(one_object(std::mem::take(object)));
    }
    let rows = values.into_iter().flat_map(|value| match value {
        Value::Array(items) => items,
        row => vec![row],
    });
    Ok(Value::Array(rows.collect()))
}

/// A lone object. With `rows`, it's the table format. Without, it's data: if
/// its only array is a non-empty array of objects, as in an API's
/// `{"items": [...]}`, those are the rows. Otherwise the object is one row.
fn one_object(mut object: Map<String, Value>) -> Value {
    if object.contains_key("rows") {
        return Value::Object(object);
    }
    let arrays: Vec<String> = object
        .iter()
        .filter(|(_, value)| value.is_array())
        .map(|(name, _)| name.clone())
        .collect();
    if let [name] = arrays.as_slice()
        && is_rows(&object[name])
    {
        return object[name].take();
    }
    Value::Array(vec![Value::Object(object)])
}

/// A non-empty array whose items are all objects.
fn is_rows(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|items| !items.is_empty() && items.iter().all(Value::is_object))
}
```

Notes:
- `Deserializer::into_iter` reads values back to back. A broken one is an error with its line and column (`EOF while parsing a value at line 3 column 0`). A non-object row is rejected later by serde, as today (`invalid type: integer`, `expected a map`).
- Only a lone object is special. A lone array goes through the same flattening as several values and comes out unchanged, so it needs no case of its own. `as_mut_slice` lets the pattern match one object in place, and `std::mem::take` moves it out, leaving an empty map behind in a `Vec` that's about to be dropped.
- `arrays` holds owned names, so the borrow of `object` ends before `object[name].take()` borrows it mutably. `Value::take` swaps in `Null` and returns the array without copying it.
- `if let … && …` is a let chain (edition 2024, Rust 1.88+; the toolchain is 1.99).

### `src/table.rs`

Add `use crate::formats;` after `use crate::files;`. Replace `parse`, `parse_with`, `from_path` and `from_path_with` with the following. `from_json`'s body is `parse_with`'s from `if value.is_array()` on, unchanged.

```rust
    /// Parse tv's own JSON: the table format, or a bare array of rows. This
    /// reads a session's `table.json`, where taking other formats would only
    /// hide a broken file. Input from users and agents goes through
    /// `parse_input`.
    pub fn parse(text: &str) -> anyhow::Result<Table> {
        Table::from_json(
            serde_json::from_str(text)?,
            Meta::default(),
            Meta::default(),
        )
    }

    /// Parse input in any format `formats::to_json` reads, with `set`
    /// replacing its `title` and `key`, and `fill` supplying them where the
    /// input has neither a value nor `null`. `set` applies before
    /// validation, so a `--key` is checked like one in the file. A key from
    /// `fill` gets the same check just after, so that its error, and no
    /// other, can say where the key came from.
    pub fn parse_input(text: &str, set: Meta, fill: Meta) -> anyhow::Result<Table> {
        Table::from_json(formats::to_json(text)?, set, fill)
    }

    /// The table in `value`, as `parse` and `parse_input` describe.
    fn from_json(mut value: Value, set: Meta, fill: Meta) -> anyhow::Result<Table> {
        if value.is_array() {
            // … the rest of today's `parse_with`, unchanged …
    }

    /// A session's `table.json`: `parse` on the file's text.
    pub fn from_path(path: impl AsRef<Path>) -> anyhow::Result<Table> {
        let path = path.as_ref();
        Table::parse(&files::read_text(path)?)
            .with_context(|| format!("parsing {}", path.display()))
    }

    /// A file a user or agent names: `parse_input` on its text, with `set`
    /// and `fill` as there.
    pub fn from_input_path(path: impl AsRef<Path>, set: Meta, fill: Meta) -> anyhow::Result<Table> {
        let path = path.as_ref();
        let text = files::read_text(path)?;
        Table::parse_input(&text, set, fill).with_context(|| format!("parsing {}", path.display()))
    }
```

### `src/files.rs`

Before `expand_tilde`, next to `write_json` and `write_atomic`:

```rust
/// `path`'s contents, with an error that names it.
pub fn read_text(path: &Path) -> anyhow::Result<String> {
    fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}
```

Both `from_path` and `from_input_path` read through it. That keeps the `reading <path>` error that `a_missing_file_exits_1_and_makes_no_session` checks. `skills.rs` has the same expression inline; it's left alone here.

### `src/main.rs`

- `mod formats;` after `mod files;`.
- `use table::{Meta, Table, single_line};`
- `load_stdin`'s last line: `Table::parse_input(&text, Meta::default(), Meta::default()).context("parsing stdin")`.
- In `open`:

  ```rust
          Target::File(path) => sessions.create(&Table::from_input_path(
              &path,
              Meta::default(),
              Meta::default(),
          )?)?,
  ```

### `src/agent.rs`

In `TableInput::read`, `Table::parse_with` becomes `Table::parse_input`, and `Table::from_path_with` becomes `Table::from_input_path`.

### Tests

**`table.rs`:** rename every `Table::parse_with(` to `Table::parse_input(` and `Table::from_path_with(` to `Table::from_input_path(`. Rename the tests `parse_with_sets_a_title_and_key` → `parse_input_sets_a_title_and_key` and `from_path_with_applies_meta` → `from_input_path_applies_meta`. Then add:

```rust
    #[test]
    fn parse_reads_only_json_and_parse_input_reads_any_format() {
        let lines = "{\"id\": \"a\"}\n{\"id\": \"b\"}\n";
        assert!(Table::parse(lines).is_err());
        let table = Table::parse_input(lines, meta(None, Some("id")), Meta::default()).unwrap();
        assert_eq!(table.row_id(1), "b");
    }
```

**`formats.rs`:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn error(text: &str) -> String {
        format!("{:#}", to_json(text).unwrap_err())
    }

    // JSON

    #[test]
    fn the_table_format_and_arrays_pass_through() {
        let table = json!({"title": "T", "rows": [{"a": 1}]});
        assert_eq!(to_json(&table.to_string()).unwrap(), table);
        assert_eq!(to_json(r#"[{"a": 1}]"#).unwrap(), json!([{"a": 1}]));
    }

    #[test]
    fn json_lines_are_rows() {
        let text = "{\"a\": 1}\n{\"a\": 2, \"b\": true}\n";
        assert_eq!(
            to_json(text).unwrap(),
            json!([{"a": 1}, {"a": 2, "b": true}])
        );
    }

    #[test]
    fn pretty_objects_one_after_another_are_rows() {
        // What `jq '.rows[]'` prints.
        let text = "{\n  \"a\": 1\n}\n{\n  \"a\": 2\n}\n";
        assert_eq!(to_json(text).unwrap(), json!([{"a": 1}, {"a": 2}]));
    }

    #[test]
    fn arrays_one_after_another_are_joined() {
        // What `gh api --paginate` prints: one array per page.
        let text = r#"[{"a": 1}, {"a": 2}][{"a": 3}]"#;
        assert_eq!(
            to_json(text).unwrap(),
            json!([{"a": 1}, {"a": 2}, {"a": 3}])
        );
    }

    #[test]
    fn a_broken_json_line_is_an_error() {
        let message = error("{\"a\": 1}\n{\"a\": \n");
        assert!(message.contains("line 3"), "{message}");
    }

    #[test]
    fn an_objects_only_array_of_objects_is_its_rows() {
        let list = json!({"kind": "List", "items": [{"a": 1}], "metadata": {"n": 1}});
        assert_eq!(to_json(&list.to_string()).unwrap(), json!([{"a": 1}]));
    }

    #[test]
    fn any_other_object_is_one_row() {
        // Two arrays, as a GitHub issue has `labels` and `assignees`.
        let issue = json!({"title": "x", "labels": [{"name": "bug"}], "assignees": []});
        assert_eq!(to_json(&issue.to_string()).unwrap(), json!([issue]));
        // One array, but not of objects, or empty.
        for object in [
            json!({"topics": ["rust"]}),
            json!({"items": []}),
            json!({"a": 1}),
        ] {
            assert_eq!(to_json(&object.to_string()).unwrap(), json!([object]));
        }
    }

    #[test]
    fn an_object_with_rows_is_left_for_the_table_format() {
        let text = r#"{"rows": 5, "items": [{"a": 1}]}"#;
        assert_eq!(
            to_json(text).unwrap(),
            json!({"rows": 5, "items": [{"a": 1}]})
        );
    }

    #[test]
    fn empty_input_is_an_error() {
        for text in ["", " \n\n", "\u{feff}"] {
            assert_eq!(error(text), "the input is empty");
        }
    }

    #[test]
    fn a_byte_order_mark_is_skipped() {
        assert_eq!(to_json("\u{feff}[{\"a\": 1}]").unwrap(), json!([{"a": 1}]));
    }
}
```

**`tests/cli.rs`**, after `agent_new_reads_a_file_or_dash`. It's one test over a list of inputs that all hold the same two rows, so Parts 2 and 3 each add their formats as lines of data. Each input gets a fresh session: its 40 rows becoming 2 shows the push took effect.

```rust
#[test]
fn agent_push_reads_every_format_and_keeps_the_key() {
    let home = tempfile::tempdir().unwrap();
    let inputs = [
        // JSON Lines, and `jq`'s pretty objects one after another
        "{\"number\": 1, \"title\": \"a\"}\n{\"number\": 2, \"title\": \"b\"}\n",
        "{\n  \"number\": 1,\n  \"title\": \"a\"\n}\n{\n  \"number\": 2,\n  \"title\": \"b\"\n}\n",
    ];
    let expected = serde_json::json!([{"number": 1, "title": "a"}, {"number": 2, "title": "b"}]);
    for input in inputs {
        let id = new_session(home.path());
        agent(home.path(), &["push", &id], input);
        let table = agent(home.path(), &["get", &id], "");
        assert_eq!(
            (table["key"].as_str(), table["title"].as_str()),
            (Some("number"), Some("PRs")),
            "{input}"
        );
        assert_eq!(table["rows"], expected, "{input}");
    }
}
```
## Part 2: CSV and TSV

### `src/table.rs`

`infer_type`'s rule moves into `values_type`, so CSV's typing can use the same rule:

```rust
/// `Number` or `Bool` if every non-null value is one; otherwise `String`.
fn infer_type(rows: &[Row], column: &str) -> ColumnType {
    values_type(rows.iter().filter_map(|row| row.get(column)))
}

/// `infer_type` for any values. CSV's cells use it too (`formats.rs`).
pub fn values_type<'a>(values: impl IntoIterator<Item = &'a Value>) -> ColumnType {
    let mut kinds = values
        .into_iter()
        .filter(|value| !value.is_null())
        .map(scalar_type);
    match kinds.next() {
        Some(Some(first)) if kinds.all(|kind| kind == Some(first)) => first,
        _ => ColumnType::String,
    }
}
```

### `src/formats.rs`

The imports become:

```rust
use anyhow::bail;
use serde_json::{Map, Value, json};

use crate::table::{ColumnType, values_type};
```

The tests' own `use serde_json::json;` goes, since `use super::*` now brings it. `to_json` sniffs the first line:

```rust
/// `text` as the JSON `Table` reads: the table format, or an array of rows.
/// The first non-blank line decides the format: `{` or `[` starts JSON, and
/// anything else is CSV, TSV or the like.
pub fn to_json(text: &str) -> anyhow::Result<Value> {
    // Excel and other Windows tools start UTF-8 files with a byte order
    // mark, which none of the formats expects.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(first) = text.lines().map(str::trim).find(|line| !line.is_empty()) else {
        bail!("the input is empty");
    };
    if first.starts_with(['{', '[']) {
        json(text)
    } else {
        delimited(text, delimiter(first))
    }
}
```

After `is_rows`:

```rust
/// One line of CSV, TSV or Markdown, split into cells.
struct Record {
    /// The line it starts on, counting from 1, for errors.
    line: usize,
    cells: Vec<String>,
}

/// Whichever of comma, tab, semicolon and `|` the header line has most of.
/// A tie goes to the earlier one, so a line with none is one column of CSV.
fn delimiter(header: &str) -> char {
    [',', '\t', ';', '|']
        .into_iter()
        .min_by_key(|&delimiter| std::cmp::Reverse(header.matches(delimiter).count()))
        .unwrap_or(',')
}

/// CSV, TSV and the like: a header line, then a line per row.
fn delimited(text: &str, delimiter: char) -> anyhow::Result<Value> {
    let records = split_records(text, delimiter)?;
    let Some((header, rows)) = records.split_first() else {
        bail!("the input is empty");
    };
    // Without this, any line of text would be a table with one column and
    // no rows: an error message piped in by mistake, say.
    if header.cells.len() == 1 && rows.is_empty() {
        bail!("not a table: one line of text, with nothing to split it into columns");
    }
    table_from_records(header, rows)
}

/// Split `text` into records, as RFC 4180 describes CSV. A field that starts
/// with `"` is quoted: it can hold the delimiter and line breaks, and `""`
/// in it is one `"`. A `"` anywhere else is just a character. Lines end in
/// `\n` or `\r\n`, and blank lines are skipped.
fn split_records(text: &str, delimiter: char) -> anyhow::Result<Vec<Record>> {
    let mut records = Vec::new();
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut line = 1;
    let mut record_line = 1;
    // The line a quoted field started on, while inside it.
    let mut quote_line = None;
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if quote_line.is_some() {
            match character {
                '"' if chars.next_if_eq(&'"').is_some() => cell.push('"'),
                '"' => quote_line = None,
                _ => {
                    if character == '\n' {
                        line += 1;
                    }
                    cell.push(character);
                }
            }
            continue;
        }
        match character {
            '"' if cell.is_empty() => quote_line = Some(line),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                cells.push(std::mem::take(&mut cell));
                push_record(&mut records, record_line, std::mem::take(&mut cells));
                line += 1;
                record_line = line;
            }
            _ if character == delimiter => cells.push(std::mem::take(&mut cell)),
            _ => cell.push(character),
        }
    }
    if let Some(line) = quote_line {
        bail!("line {line}: a quoted field has no closing quote");
    }
    cells.push(cell);
    push_record(&mut records, record_line, cells);
    Ok(records)
}

/// Add a record, unless it's a blank line: one empty cell.
fn push_record(records: &mut Vec<Record>, line: usize, cells: Vec<String>) {
    if !matches!(cells.as_slice(), [only] if only.is_empty()) {
        records.push(Record { line, cells });
    }
}

/// The table for a header and the rows under it, whose cells are all text.
/// Columns are declared, so a column with no values still shows. Empty and
/// missing cells are null. A column whose cells are all numbers, or all
/// booleans, holds those (see `scalar`); any other column holds text.
fn table_from_records(header: &Record, rows: &[Record]) -> anyhow::Result<Value> {
    let names = column_names(&header.cells);
    for row in rows {
        // A trailing delimiter leaves empty cells past the header's, which
        // are harmless. Anything else past it has no column to go in.
        let extra = row.cells.get(names.len()..).unwrap_or_default();
        if extra.iter().any(|cell| !cell.is_empty()) {
            bail!(
                "line {}: {} cells, but the header has {}",
                row.line,
                row.cells.len(),
                names.len()
            );
        }
    }
    let typed: Vec<bool> = (0..names.len())
        .map(|index| all_scalars(rows.iter().filter_map(|row| row.cells.get(index))))
        .collect();
    let rows: Vec<Value> = rows
        .iter()
        .map(|row| {
            let cells = names
                .iter()
                .zip(&typed)
                .enumerate()
                .map(|(index, (name, &typed))| {
                    (name.clone(), cell_value(row.cells.get(index), typed))
                });
            Value::Object(cells.collect())
        })
        .collect();
    let columns: Vec<Value> = names.iter().map(|name| json!({ "name": name })).collect();
    Ok(json!({ "columns": columns, "rows": rows }))
}

/// Header cells as column names. A row holds one value per name, so an empty
/// one becomes `column_<n>` (counting from 1), and a repeat gets `_2`, `_3`, ….
fn column_names(header: &[String]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (index, cell) in header.iter().enumerate() {
        let base = if cell.is_empty() {
            format!("column_{}", index + 1)
        } else {
            cell.clone()
        };
        let mut name = base.clone();
        let mut suffix = 2;
        while names.contains(&name) {
            name = format!("{base}_{suffix}");
            suffix += 1;
        }
        names.push(name);
    }
    names
}

/// Whether a column's non-empty cells are all numbers, or all booleans: each
/// one is a `scalar`, and `values_type` gives them a type.
fn all_scalars<'a>(cells: impl Iterator<Item = &'a String>) -> bool {
    let values: Option<Vec<Value>> = cells
        .filter(|cell| !cell.is_empty())
        .map(|cell| scalar(cell))
        .collect();
    values.is_some_and(|values| values_type(&values) != ColumnType::String)
}

/// A cell's value: null if it's empty or missing, its `scalar` in a `typed`
/// column, and its text otherwise.
fn cell_value(cell: Option<&String>, typed: bool) -> Value {
    match cell.map(String::as_str) {
        None | Some("") => Value::Null,
        Some(text) if typed => scalar(text).unwrap_or_else(|| text.into()),
        Some(text) => text.into(),
    }
}

/// A cell's text as a JSON number or boolean, if it is one and writes back
/// as exactly the same text: `412`, `-2.5`, `true`. Not `02134` (not JSON),
/// `1.50` (it would show as `1.5`), or a number with more digits than a
/// float keeps. So a column is never shown differently from the file.
fn scalar(text: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(text).ok()?;
    let is_scalar = matches!(value, Value::Number(_) | Value::Bool(_));
    // Not `value == text`, which clippy suggests: that compares the value
    // with a JSON string, so no number would ever match.
    let written = value.to_string();
    (is_scalar && written == text).then_some(value)
}
```

Notes:
- `split_records` walks characters with a small state: inside a quoted field or not. `Peekable::next_if_eq` takes the next character only if it matches, which handles `""` and `\r\n` without lookahead bookkeeping.
- `quote_line` does two jobs: it says whether we're inside quotes, and it remembers the line for the error.
- `min_by_key` with `Reverse` returns the *first* of the largest counts; `max_by_key` would return the last, and break the tie order.
- `all_scalars` collects into `Option<Vec<Value>>`, which is `None` as soon as one cell isn't a scalar. `values_type` then checks that they're all one type: the same check the table runs on every column without a declared type.
- `scalar` parses each cell twice (once in `all_scalars`, once in `cell_value`). Even a large CSV is fine; caching it isn't worth the code.
- `std::cmp::Reverse` and `std::mem::take` are written out in full, as `viewer/app.rs` does with `std::mem::take`, rather than imported for one use each.

### Tests

**`formats.rs`:** a helper beside `error`, since most CSV and Markdown tests check only the rows:

```rust
    fn rows(text: &str) -> Value {
        to_json(text).unwrap()["rows"].clone()
    }
```

Add the CSV assertion to `a_byte_order_mark_is_skipped`:

```rust
        assert_eq!(
            to_json("\u{feff}a,b\n1,2").unwrap()["columns"][0]["name"],
            "a"
        );
```

Then, after it:

```rust
    // CSV and TSV

    #[test]
    fn csv_has_a_header_then_rows() {
        let table = to_json("name,age\nsam,41\njo,7\n").unwrap();
        assert_eq!(
            table,
            json!({
                "columns": [{"name": "name"}, {"name": "age"}],
                "rows": [{"name": "sam", "age": 41}, {"name": "jo", "age": 7}]
            })
        );
    }

    #[test]
    fn the_header_line_picks_the_delimiter() {
        assert_eq!(rows("a\tb\n1\t2"), json!([{"a": 1, "b": 2}]));
        assert_eq!(rows("a;b\n1,5;2"), json!([{"a": "1,5", "b": 2}]));
        assert_eq!(rows("a|b\n1|2"), json!([{"a": 1, "b": 2}]));
        // Most wins: a tab-separated header with a comma in one name.
        assert_eq!(
            rows("a,x\tb\tc\n1\t2\t3"),
            json!([{"a,x": 1, "b": 2, "c": 3}])
        );
        // None: one column.
        assert_eq!(
            rows("name\nsam\njo"),
            json!([{"name": "sam"}, {"name": "jo"}])
        );
    }

    #[test]
    fn quoted_fields_hold_delimiters_quotes_and_newlines() {
        let text = "a,b\r\n\"x, y\",\"say \"\"hi\"\"\"\r\n\"two\nlines\",5 \"in\"\r\n";
        assert_eq!(
            rows(text),
            json!([
                {"a": "x, y", "b": "say \"hi\""},
                {"a": "two\nlines", "b": "5 \"in\""}
            ])
        );
    }

    #[test]
    fn an_unclosed_quote_names_its_line() {
        assert_eq!(
            error("a,b\n1,2\n3,\"oops\n4,5\n"),
            "line 3: a quoted field has no closing quote"
        );
    }

    #[test]
    fn a_column_is_numbers_only_if_every_cell_reads_back_the_same() {
        let text = "n,zip,price,big,flag,mixed\n\
                    1,02134,1.50,123456789012345678901,true,1\n\
                    -2.5,10001,2.25,1,false,true\n";
        let parsed = rows(text);
        assert_eq!(
            parsed[0],
            json!({"n": 1, "zip": "02134", "price": "1.50",
                   "big": "123456789012345678901", "flag": true, "mixed": "1"})
        );
        assert_eq!(
            parsed[1],
            json!({"n": -2.5, "zip": "10001", "price": "2.25",
                   "big": "1", "flag": false, "mixed": "true"})
        );
    }

    #[test]
    fn empty_and_missing_cells_are_null() {
        assert_eq!(
            rows("a,b,c\n1,,x\n2\n"),
            json!([{"a": 1, "b": null, "c": "x"}, {"a": 2, "b": null, "c": null}])
        );
    }

    #[test]
    fn blank_lines_are_skipped_and_errors_count_them() {
        assert_eq!(rows("\na,b\n\n1,2\n\n"), json!([{"a": 1, "b": 2}]));
        assert_eq!(
            error("a,b\n\n1,2,3\n"),
            "line 3: 3 cells, but the header has 2"
        );
    }

    #[test]
    fn extra_cells_are_an_error_unless_empty() {
        assert_eq!(rows("a,b\n1,2,\n"), json!([{"a": 1, "b": 2}]));
    }

    #[test]
    fn empty_and_repeated_names_are_made_unique() {
        let table = to_json("a,,a,a_2,a\n1,2,3,4,5\n").unwrap();
        let names: Vec<&str> = table["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|column| column["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["a", "column_2", "a_2", "a_2_2", "a_3"]);
    }

    #[test]
    fn a_header_alone_is_a_table_with_no_rows() {
        let table = to_json("a,b\n").unwrap();
        assert_eq!(
            table,
            json!({"columns": [{"name": "a"}, {"name": "b"}], "rows": []})
        );
    }

    #[test]
    fn one_line_of_plain_text_is_not_a_table() {
        for text in ["not json", "5", "\"quoted, still one cell\"\n"] {
            assert!(error(text).starts_with("not a table"), "{text}");
        }
    }
```

**`table.rs`:**

```rust
    #[test]
    fn csv_columns_get_types() {
        let table =
            Table::parse_input("id,n\na,1\nb,2\n", Meta::default(), Meta::default()).unwrap();
        assert_eq!(column_names(&table), ["id", "n"]);
        let kinds: Vec<_> = table.columns.iter().map(|column| column.kind).collect();
        assert_eq!(kinds, [Some(ColumnType::String), Some(ColumnType::Number)]);
    }

    #[test]
    fn a_csv_key_is_validated_like_any_other() {
        let error =
            Table::parse_input("id\na\na\n", meta(None, Some("id")), Meta::default()).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("duplicate key value \"a\""), "{message}");
    }
```

**`tests/cli.rs`:** add CSV and TSV to `agent_push_reads_every_format_and_keeps_the_key`'s inputs:

```rust
        // CSV and TSV
        "number,title\n1,a\n2,b\n",
        "number\ttitle\n1\ta\n2\tb\n",
```

Then two tests after it. `tv <file>` can't be tested past loading (the viewer needs a terminal), so the file test is a failure that stops before it:

```rust
#[test]
fn agent_new_reads_csv_and_get_prints_json() {
    let home = tempfile::tempdir().unwrap();
    let csv = "id,name,score\n7,sam,1.5\n9,\"jo, jr\",\n";
    let created = agent(home.path(), &["new", "--key", "id"], csv);
    assert_eq!(
        (created["rows"].as_u64(), created["columns"].as_u64()),
        (Some(2), Some(3))
    );
    let id = created["session"].as_str().unwrap();
    let table = agent(home.path(), &["get", id], "");
    assert_eq!(table["key"], "id");
    assert_eq!(
        table["rows"],
        serde_json::json!([
            {"id": 7, "name": "sam", "score": 1.5},
            {"id": 9, "name": "jo, jr", "score": null}
        ])
    );
    assert_eq!(table["columns"][2]["type"], "number");
}

#[test]
fn a_broken_csv_file_says_where_and_makes_no_session() {
    let home = tempfile::tempdir().unwrap();
    let file = home.path().join("bad.csv");
    fs::write(&file, "a,b\n1,2\n3,\"oops\n").unwrap();
    let output = tv(home.path(), &[file.to_str().unwrap()], "");
    assert_fails_with(&output, "line 3: a quoted field has no closing quote");
    assert!(stderr(&output).contains("bad.csv"), "{}", stderr(&output));
    assert!(!home.path().join("sessions").exists());
}
```

## Part 3: Markdown tables

### `src/formats.rs`

`to_json` gains a branch, and its doc says so:

```rust
/// `text` as the JSON `Table` reads: the table format, or an array of rows.
/// The first non-blank line decides the format: `{` or `[` starts JSON, `|`
/// starts a Markdown table, and anything else is CSV, TSV or the like.
```

```rust
    if first.starts_with(['{', '[']) {
        json(text)
    } else if first.starts_with('|') {
        markdown(text)
    } else {
        delimited(text, delimiter(first))
    }
```

After `push_record`:

```rust
/// A Markdown pipe table: a header row, a `|---|` row under it, then the
/// rows. Every non-blank line starts with `|`, the closing `|` is optional,
/// and `\|` is a `|` inside a cell. Cells are trimmed, and formatting such
/// as `**bold**` stays as written.
fn markdown(text: &str) -> anyhow::Result<Value> {
    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(row) = line.strip_prefix('|') else {
            bail!(
                "line {}: every line of a Markdown table starts with |",
                index + 1
            );
        };
        records.push(Record {
            line: index + 1,
            cells: markdown_cells(row),
        });
    }
    let [header, divider, rows @ ..] = records.as_slice() else {
        bail!("a Markdown table needs a header row and a |---| row under it");
    };
    if !divider.cells.iter().all(|cell| is_divider(cell)) {
        bail!(
            "line {}: a Markdown table needs a |---| row under its header",
            divider.line
        );
    }
    table_from_records(header, rows)
}

/// A Markdown row's cells, from the text after its opening `|`.
fn markdown_cells(row: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut chars = row.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\\' if chars.next_if_eq(&'|').is_some() => cell.push('|'),
            '|' => cells.push(std::mem::take(&mut cell)),
            _ => cell.push(character),
        }
    }
    // The line is trimmed, so text after the last `|` is a cell, and nothing
    // after it means that `|` closed the row.
    if !cell.is_empty() {
        cells.push(cell);
    }
    cells.iter().map(|cell| cell.trim().to_string()).collect()
}

/// A divider row's cell: dashes, with an optional `:` at either end for
/// alignment, which tv ignores (numbers are right-aligned anyway).
fn is_divider(cell: &str) -> bool {
    let cell = cell.strip_prefix(':').unwrap_or(cell);
    let cell = cell.strip_suffix(':').unwrap_or(cell);
    !cell.is_empty() && cell.chars().all(|character| character == '-')
}
```

Notes:
- `markdown_cells` builds cells the way `split_records` does: a `cell` being filled, pushed with `std::mem::take` at each separator. The two can't share code, because CSV has quotes and Markdown has `\|`.
- A divider row is any row whose cells are all dashes. A lone `|` has no cells, so it passes too, and the table still reads correctly.
- `let [header, divider, rows @ ..] = … else` is a slice pattern: it matches two or more records and names the rest.
- Line numbers count from the start of the input, blank lines included, like CSV's.

### Tests

**`formats.rs`**, after the CSV tests:

```rust
    // Markdown

    #[test]
    fn markdown_tables_are_read() {
        let text = "\
| name | age | note |
|:-----|----:|:----:|
| sam  | 41  | **a** \\| b |
| jo   |     |
";
        assert_eq!(
            to_json(text).unwrap(),
            json!({
                "columns": [{"name": "name"}, {"name": "age"}, {"name": "note"}],
                "rows": [
                    {"name": "sam", "age": 41, "note": "**a** | b"},
                    {"name": "jo", "age": null, "note": null}
                ]
            })
        );
    }

    #[test]
    fn markdown_without_closing_pipes() {
        assert_eq!(
            rows("| a | b\n|---|---\n| 1 | x\n"),
            json!([{"a": 1, "b": "x"}])
        );
    }

    #[test]
    fn markdown_needs_the_divider_and_leading_pipes() {
        assert_eq!(
            error("| a |\n| 1 |\n"),
            "line 2: a Markdown table needs a |---| row under its header"
        );
        assert_eq!(
            error("| a |\n"),
            "a Markdown table needs a header row and a |---| row under it"
        );
        assert_eq!(
            error("| a |\n|---|\n| 1 |\nSome text after.\n"),
            "line 4: every line of a Markdown table starts with |"
        );
    }
```

**`tests/cli.rs`:** add a Markdown table to `agent_push_reads_every_format_and_keeps_the_key`'s inputs:

```rust
        // A Markdown table
        "| number | title |\n|---|---|\n| 1 | a |\n| 2 | b |\n",
```

## Part 4: a file's name is its title

### `src/table.rs`

`use std::ffi::OsStr;` after `use std::collections::…`. `from_input_path` becomes:

```rust
    /// A file a user or agent names: `parse_input` on its text, with `set`
    /// and `fill` as there. The file's name is the title of last resort:
    /// CSV has nowhere to put one, and `tv ls` needs something to tell
    /// sessions apart.
    pub fn from_input_path(path: impl AsRef<Path>, set: Meta, fill: Meta) -> anyhow::Result<Table> {
        let path = path.as_ref();
        let text = files::read_text(path)?;
        let name = path.file_name().and_then(OsStr::to_str);
        let fill = Meta {
            title: fill.title.or(name),
            ..fill
        };
        Table::parse_input(&text, set, fill).with_context(|| format!("parsing {}", path.display()))
    }
```

Notes:
- Putting the name in `fill` gives Decision 10's order for free: `set` beats the file's own `title`, which beats `fill`, and `fill.title.or(name)` puts a kept session title ahead of the name.
- `fill` borrows `name` from `path`, a shorter lifetime than the caller's `Meta<'_>`. That's fine: `Meta` only holds `&str`s, so Rust shortens the lifetime to fit.
- Only `from_input_path` does this. `from_path` reads `table.json` and must never title a table `table.json`.

### Tests

**`table.rs`:**

```rust
    #[test]
    fn from_input_path_titles_a_file_by_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sales.csv");
        std::fs::write(&path, "a,b\n1,2\n").unwrap();
        let title = |set: Meta, fill: Meta| Table::from_input_path(&path, set, fill).unwrap().title;
        assert_eq!(
            title(Meta::default(), Meta::default()).as_deref(),
            Some("sales.csv")
        );
        // A title passed in, or kept from the session, comes first.
        assert_eq!(
            title(meta(Some("Set"), None), Meta::default()).as_deref(),
            Some("Set")
        );
        assert_eq!(
            title(Meta::default(), meta(Some("Kept"), None)).as_deref(),
            Some("Kept")
        );
        // So does the file's own, and its `null`.
        let own = Table::from_input_path("examples/prs.json", Meta::default(), Meta::default());
        assert_eq!(own.unwrap().title.as_deref(), Some("Open pull requests"));
        std::fs::write(&path, r#"{"title": null, "rows": []}"#).unwrap();
        assert_eq!(title(Meta::default(), Meta::default()), None);
    }
```

**`tests/cli.rs`:**

```rust
#[test]
fn agent_new_titles_a_file_by_its_name() {
    let home = tempfile::tempdir().unwrap();
    let file = home.path().join("sales.csv");
    fs::write(&file, "region,total\neast,10\n").unwrap();
    let path = file.to_str().unwrap();
    let created = agent(home.path(), &["new", path], "");
    let table = agent(
        home.path(),
        &["get", created["session"].as_str().unwrap()],
        "",
    );
    assert_eq!(table["title"], "sales.csv");
    let created = agent(home.path(), &["new", path, "--title", "Sales"], "");
    let table = agent(
        home.path(),
        &["get", created["session"].as_str().unwrap()],
        "",
    );
    assert_eq!(table["title"], "Sales");
}
```

## Part 5: docs, help and the guide

### Help text

- **`src/main.rs`,** `Cli::target`:

  ```rust
      /// A file to open (`.` or `/` in the name), `-` to read stdin, or a
      /// session ID. With none, the most recent session. Same as `tv open`.
      /// Files and stdin can be JSON, JSON Lines, CSV, TSV or a Markdown table.
  ```

- **`src/agent.rs`:**
  - `New`'s doc: `/// Create a session from FILE, or from stdin.`
  - `TableInput::file`'s doc:

    ```rust
        /// A file: JSON (the table format, an array of rows, or JSON Lines), CSV,
        /// TSV or a Markdown table. Stdin without one, or with `-`.
    ```

  - `read_stdin`'s error: `"no table: give a FILE, or pipe one to stdin"`.

### `skills/tv-tables/SKILL.md`

Quick start, replacing its first paragraph and code block:

````markdown
Pipe rows into `tv agent new`, or give it a file. It reads JSON (an array of
objects, or one object per line), CSV, TSV and Markdown tables, so most
command output goes straight in:

```sh
gh pr list --json number,title,headRefName | tv agent new --key number --title "Open PRs"
sqlite3 -csv -header app.db 'select id, email from users' | tv agent new --key id --title "Users"
tv agent new rows.json --key id --title "Failing tests"
```
````

In "Updating the table":

```markdown
For a small edit, `get` the table, change it, and push it back. `get` prints
JSON, whatever format the table came in:
```

The guide tests parse the new `sqlite3` line's `tv` command; they pass in the scratch copy.

### `docs/schema.md`

Add a section after "Shorthand":

````markdown
## Input formats

`tv open`, `tv -`, `tv agent new` and `tv agent push` read more than this format. Each input becomes a table in this format, which is what a session stores and what `tv agent get` and `tv save` print. The first non-blank line decides how the input is read:

| First character | Read as |
|---|---|
| `{` or `[` | JSON, below |
| `\|` | A Markdown table |
| anything else | CSV, TSV or the like. The delimiter is whichever of `,`, tab, `;` and `\|` the first line has most of. |

A UTF-8 byte order mark is skipped. Empty input is an error.

### JSON

- **One object with `rows`:** the full format.
- **One array:** the shorthand.
- **Several values in a row** (JSON Lines, `jq '.[]'` output, `gh api --paginate`): each object is a row, and each array's items are rows.
- **One object without `rows`:** if its only array is a non-empty array of objects, as in `{"kind": "List", "items": [...]}`, those are the rows. Otherwise the object is one row, and the record view (`r`) shows it a field per line.

### CSV and TSV

- The first line is the header.
- Quoting follows RFC 4180: a field that starts with `"` can hold the delimiter and line breaks, and `""` inside it is one `"`. Lines end in `\n` or `\r\n`. Blank lines are skipped.
- Empty and missing cells are null.
- A column holds numbers if every non-empty cell is a JSON number that writes back exactly as written (`412`, `-2.5`), and booleans likewise (`true`, `false`). Any other column holds text, so `02134` and `1.50` show as written.
- An empty header cell is named `column_<n>`, counting from 1. A repeated name gets `_2`, `_3`, ….
- A row with more cells than the header is an error, unless the extra cells are empty (a trailing delimiter).
- One line with no delimiter is an error rather than a one-column table, so a stray message piped in doesn't become a session.

### Markdown tables

- A header row, a `|---|` row under it (`:` for alignment is allowed and ignored), then the rows.
- Every non-blank line starts with `|`. The closing `|` is optional. `\|` is a `|` inside a cell.
- Cells are trimmed and typed like CSV's. Formatting such as `**bold**` stays as written.

### Titles

A file that names no title is titled with its file name, such as `sales.csv`. `--title`, the file's own `title`, and on `push` the session's title, all come first.
````

Also replace the page's opening paragraph, which says the JSON format is the input to `new` and `push`, with: "`tv` writes tables as JSON, in the format below: it's what `tv save` and `tv agent get` print. It reads that format, and the [input formats](#input-formats) further down."

### `docs/SPEC.md`

- **"Not doing":** the CSV row becomes `| ~~CSV / other input formats~~ | **Reversed 2026-10-29.** See "Input formats" under "Table format". |`, as the `goto` row was.
- **"Table format":** after the Shorthand paragraph, a paragraph:

  ```markdown
  **Input formats:** input can also be JSON Lines (or any JSON values one after another), an object wrapping one array of rows, a single record, CSV, TSV or a Markdown table. The first non-blank line decides how it's read. Each becomes this format, which is what a session stores, so nothing after loading knows what the input was. A file that names no title is titled with its file name. Details are in "Input formats" in `docs/schema.md`.
  ```

- **"Human CLI":** the `tv open <file.json>` row becomes `tv open <file>`: "Create a new session from the file and open it. Any input format; a saved file's comments are imported."
- **`tv agent new`:** after "Comments in the file are imported…", add: "FILE and stdin can be in any input format (see "Table format"). A file that names no title is titled with its file name."
- **Module layout:** after `table.rs`, add `formats.rs       # input formats: JSON Lines and other JSON shapes, CSV, TSV and Markdown tables, turned into the JSON table.rs reads`.
- **"Testing":** the first bullet adds "input formats".
- **"Changes after review":** add `- **Input formats:** CSV, TSV, JSON Lines and Markdown tables, under "Table format". Reverses the "Not doing" entry.`

### `README.md`

Quick start's first two lines become:

```sh
gh pr list --json number,title,author | tv -   # JSON, JSON Lines, CSV, TSV or Markdown
tv sales.csv                                   # or a file; see docs/schema.md
```

And its "[docs/schema.md](docs/schema.md) describes the table format." becomes "…describes the table format and the input formats tv reads."

## Manual check

Build, and put the build on `PATH`:

```sh
cd ~/tv && cargo build
export PATH="$PWD/target/debug:$PATH"
export TV_HOME=$(mktemp -d)
```

1. **CSV file.**

   ```sh
   printf 'name,age,zip\nsam,41,02134\njo,7,10001\n' > /tmp/people.csv && tv /tmp/people.csv
   ```

   - The title bar says `people.csv`.
   - `age` is right-aligned, and `zip` is left-aligned with `02134` shown as written.
   - `:w` offers `./people-csv.json`.
2. **Spreadsheet paste.** Copy a block of cells, header row included, from Numbers, Excel or Google Sheets. `pbpaste | tv -` shows them as a table.
3. **JSON streams.**
   - `jq '.rows[]' examples/prs.json | tv -` shows 40 rows.
   - `jq '{kind: "List", items: .rows}' examples/prs.json | tv -` shows 40 rows.
   - `jq '.rows[0]' examples/prs.json | tv -` shows one row, and `r` lists its fields.
4. **Markdown.** Copy a table an agent printed (or one from a README) and run `pbpaste | tv -`. The cells line up, and `**bold**` shows as written.
5. **Errors.**
   - `echo "not json" | tv -` fails with "not a table".
   - `printf 'a,b\n1,"x\n' | tv -` fails naming line 2.
   - `tv ls` shows no session from either.
6. **Agent.**
   - `tv agent new /tmp/people.csv --key name` works, and `tv agent get <id>` prints JSON with `"age": 41`.
   - `printf 'name\tage\nsam\t42\n' | tv agent push <id>` keeps the key and the `people.csv` title.
7. `unset TV_HOME`.

## Acceptance

```sh
make check
cargo tree --locked -e normal --prefix none | sort -u | wc -l   # 89, unchanged
```

Plus the manual check above.

## Deliverables

- **`src/formats.rs` (new):** `to_json`, `json`, `one_object`, `is_rows`, `Record`, `delimiter`, `delimited`, `split_records`, `push_record`, `markdown`, `markdown_cells`, `is_divider`, `table_from_records`, `column_names`, `all_scalars`, `cell_value`, `scalar`, and their tests with the `error` and `rows` helpers.
- **`src/table.rs`:** the strict `parse`/`from_path`, `parse_input`/`from_input_path` (renamed, and reading any format), `from_json`, `values_type` (split out of `infer_type`), the file-name title, the renamed tests and four new ones.
- **`src/files.rs`:** `read_text`.
- **`src/main.rs`:** `mod formats`, the `Meta` import, `load_stdin` and `open` on the input functions, and `Cli::target`'s doc.
- **`src/agent.rs`:** the renamed calls, the `New` and `TableInput::file` docs, and `read_stdin`'s message.
- **`tests/cli.rs`:** four new tests. `agent_push_reads_every_format_and_keeps_the_key` gains inputs in Parts 2 and 3.
- **`skills/tv-tables/SKILL.md`, `docs/schema.md`, `docs/SPEC.md`, `README.md`:** as in Part 5.
- **Six commits:**
  1. `Plans: input formats`: this file, after its review.
  2. `Input: JSON Lines, wrapped lists and single records`: Part 1 (287 unit, 46 CLI tests).
  3. `Input: CSV and TSV`: Part 2 (300, 48).
  4. `Input: Markdown tables`: Part 3 (303, 48).
  5. `Input: a file's name is its title`: Part 4 (304, 49).
  6. `Docs: input formats`: Part 5. The guide test covers the new example.

## Out of scope

- **A `--format` flag.** Detection by content covers the formats here. Add the flag if detection guesses wrong in practice.
- **Writing other formats** (`tv save x.csv`, `get --csv`). Output stays JSON (Decision 11).
- **Headerless CSV** (`--no-header`). Every format here has a header. Wait for someone to need it.
- **Right-aligning text columns that look numeric** (`1.50`, `+12`, `1,234`). Cheap later: set the column's `type` to `number` without converting its cells. `type` only affects alignment. Left out to keep this plan to one rule.
- **Encodings other than UTF-8** (Latin-1 or UTF-16 CSVs from old Excel). Reading fails with `stream did not contain valid UTF-8`, and `iconv` converts.
- **`mysql -B`'s `\N` and backslash escapes.** They show as written.
- **Flattening nested objects** (`author.login` as its own column). It's a different feature: it changes the data rather than reading a format.
- **Formats that need crates or guessing.** Each has an easy way in through the formats above:
  - Excel, Parquet, SQLite and Arrow: `sqlite3 -json`, `duckdb -json`, or Save As CSV.
  - YAML and TOML: `yq -o json`.
  - Space-aligned command output (`kubectl get`, `docker ps`): those tools' JSON modes.
  - logfmt: niche.
- **Large files.** Input is read into memory whole, as it is today.
