//! Input formats: what users and agents hand tv, turned into the JSON that
//! `Table` reads. JSON passes through, with a few more shapes accepted, and
//! JSON Lines, CSV, TSV and Markdown tables become rows. A session stores the
//! result as JSON, so nothing after this knows what the input was. See
//! "Input formats" in `docs/schema.md`.

use anyhow::bail;
use serde_json::{Map, Value, json};

use crate::table::{ColumnType, values_type};

/// `text` as the JSON `Table` reads: the table format, or an array of rows.
/// The first non-blank line decides the format: `{` or `[` starts JSON, `|`
/// starts a Markdown table, and anything else is CSV, TSV or the like.
pub fn to_json(text: &str) -> anyhow::Result<Value> {
    // Excel and other Windows tools start UTF-8 files with a byte order
    // mark, which none of the formats expects.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(first) = text.lines().map(str::trim).find(|line| !line.is_empty()) else {
        bail!("the input is empty");
    };
    if first.starts_with(['{', '[']) {
        json(text)
    } else if first.starts_with('|') {
        markdown(text)
    } else {
        delimited(text, delimiter(first))
    }
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

/// One line of CSV, TSV or Markdown, split into cells.
struct Record {
    /// The line it starts on, counting from 1, for errors.
    line: usize,
    cells: Vec<String>,
}

impl Record {
    /// A blank line: one empty cell.
    fn is_blank(&self) -> bool {
        matches!(self.cells.as_slice(), [only] if only.is_empty())
    }
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
    let mut records = split_records(text, delimiter)?
        .into_iter()
        .skip_while(Record::is_blank);
    let Some(header) = records.next() else {
        bail!("the input is empty");
    };
    // In a one-column table, a blank line is a row whose cell is empty: it's
    // how `sqlite3 -csv` and `psql --csv` write NULL there. With more
    // columns, it's only a blank line.
    let one_column = header.cells.len() == 1;
    let rows: Vec<Record> = records
        .filter(|record| one_column || !record.is_blank())
        .collect();
    // Without this, a line of text with no delimiter in it, such as a
    // one-word message piped in by mistake, would be a table with one column
    // and no rows. A line with a delimiter is still a header with no rows,
    // as an empty query result should be.
    if one_column && rows.iter().all(Record::is_blank) {
        bail!("not a table: one line of text, with nothing to split it into columns");
    }
    table_from_records(&header, &rows)
}

/// Split `text` into records, as RFC 4180 describes CSV. A field that starts
/// with `"` is quoted: it can hold the delimiter and line breaks, and `""`
/// in it is one `"`. A `"` anywhere else is just a character. Lines end in
/// `\n` or `\r\n`, and a blank line is a record of one empty cell.
///
/// Unlike RFC 4180, spaces around an unquoted field are dropped, since
/// people and agents often write `name, age`. A quoted field keeps its
/// spaces, and spaces before its opening quote don't stop it being quoted.
fn split_records(text: &str, delimiter: char) -> anyhow::Result<Vec<Record>> {
    // A final line break ends the last line rather than starting another.
    let text = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text);
    let mut records = Vec::new();
    let mut cells = Vec::new();
    let mut cell = Cell::default();
    let mut line = 1;
    let mut record_line = 1;
    // The line a quoted field started on, while inside it.
    let mut quote_line = None;
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if quote_line.is_some() {
            match character {
                '"' if chars.next_if_eq(&'"').is_some() => cell.text.push('"'),
                '"' => quote_line = None,
                _ => {
                    if character == '\n' {
                        line += 1;
                    }
                    cell.text.push(character);
                }
            }
            continue;
        }
        let at_start = cell.text.is_empty() && !cell.quoted;
        match character {
            ' ' if at_start => {}
            '"' if at_start => {
                cell.quoted = true;
                quote_line = Some(line);
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                cells.push(std::mem::take(&mut cell).finish());
                records.push(Record {
                    line: record_line,
                    cells: std::mem::take(&mut cells),
                });
                line += 1;
                record_line = line;
            }
            _ if character == delimiter => cells.push(std::mem::take(&mut cell).finish()),
            _ => cell.text.push(character),
        }
    }
    if let Some(line) = quote_line {
        bail!("line {line}: a quoted field has no closing quote");
    }
    cells.push(cell.finish());
    records.push(Record {
        line: record_line,
        cells,
    });
    Ok(records)
}

/// A field as `split_records` reads it.
#[derive(Default)]
struct Cell {
    text: String,
    quoted: bool,
}

impl Cell {
    /// The field's text: as written if it was quoted, and without trailing
    /// spaces if not. Leading ones were never added.
    fn finish(self) -> String {
        if self.quoted {
            self.text
        } else {
            self.text.trim_end_matches(' ').to_string()
        }
    }
}

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

/// 2^53. Every integer up to this is exact as a float, so any JSON reader
/// gets it back unchanged. Past it, one that reads numbers as floats, like
/// JavaScript's `JSON.parse`, rounds.
const MAX_EXACT_INTEGER: u64 = 1 << 53;

/// A cell's text as a JSON number or boolean, if it is one and writes back
/// as exactly the same text: `412`, `-2.5`, `true`. Not `02134` (not JSON)
/// or `1.50` (it would show as `1.5`). Not an integer past 2^53 either, such
/// as a 19-digit ID: tv would show it right, but `tv agent get` would hand
/// an agent a number its JSON reader may round. So a column is never shown,
/// or handed on, differently from the file.
fn scalar(text: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(text).ok()?;
    let is_scalar = match &value {
        Value::Bool(_) => true,
        Value::Number(number) => {
            // `None` for a float, which round-tripping already covers.
            let integer = number
                .as_u64()
                .or_else(|| number.as_i64().map(i64::unsigned_abs));
            integer.is_none_or(|integer| integer <= MAX_EXACT_INTEGER)
        }
        _ => false,
    };
    // Not `value == text`, which clippy suggests: that compares the value
    // with a JSON string, so no number would ever match.
    let written = value.to_string();
    (is_scalar && written == text).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(text: &str) -> String {
        format!("{:#}", to_json(text).unwrap_err())
    }

    fn rows(text: &str) -> Value {
        to_json(text).unwrap()["rows"].clone()
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
        assert_eq!(
            to_json("\u{feff}a,b\n1,2").unwrap()["columns"][0]["name"],
            "a"
        );
    }

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
    fn spaces_around_unquoted_cells_are_dropped() {
        assert_eq!(
            rows("name, age\nsam , 41\n"),
            json!([{"name": "sam", "age": 41}])
        );
        // A quoted cell keeps its spaces, and spaces before its opening quote
        // don't stop it being quoted.
        assert_eq!(
            rows("a, b\n\" x \", \"y, z\"\n"),
            json!([{"a": " x ", "b": "y, z"}])
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
    fn integers_past_2_to_the_53_stay_text() {
        assert_eq!(
            rows("a,b,c\n9007199254740992,1234567890123456789,-9007199254740993\n"),
            json!([{
                "a": 9_007_199_254_740_992_u64,
                "b": "1234567890123456789",
                "c": "-9007199254740993"
            }])
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
    fn a_blank_line_is_an_empty_row_in_one_column() {
        // How `sqlite3 -csv` and `psql --csv` write NULL in a one-column
        // result, at the end too.
        let expected =
            json!([{"email": "a@x"}, {"email": null}, {"email": "b@x"}, {"email": null}]);
        assert_eq!(rows("email\na@x\n\nb@x\n\n"), expected);
        assert_eq!(rows("email\r\na@x\r\n\"\"\r\nb@x\r\n\"\"\r\n"), expected);
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
        for text in [
            "not json",
            "not json\n\n",
            "5",
            "\"quoted, still one cell\"\n",
        ] {
            assert!(error(text).starts_with("not a table"), "{text}");
        }
    }

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
}
