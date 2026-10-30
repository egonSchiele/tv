//! The table format: types, parsing (including the bare-array shorthand),
//! column and type inference, and validation. See `docs/schema.md`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::comments::Thread;
use crate::files;
use crate::formats;

pub type Row = Map<String, Value>;

// Every `Table` that exists has been normalized and validated: serde builds a
// `RawTable` and then runs `TryFrom<RawTable>` (inference + validation). No caller
// can get a `Table` that skipped those steps, and no caller has to remember them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "RawTable")]
pub struct Table {
    pub version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
    /// The file format's `comments`: read from a saved file so `create` can
    /// move them into the session's log, and filled from the log when saving.
    /// Empty in a session's own `table.json`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Thread>,
}

/// The table as written in JSON, before inference and validation. It exists
/// only as the input to `TryFrom`.
#[derive(Deserialize)]
struct RawTable {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    columns: Vec<Column>,
    rows: Vec<Row>,
    #[serde(default)]
    comments: Vec<Thread>,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    // `type` is a keyword in Rust, so the field is `kind` and serde renames it.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ColumnType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColumnType {
    String,
    Number,
    Bool,
}

impl Column {
    /// The column name as the header shows it (one line, like cell text).
    pub fn display_name(&self) -> String {
        single_line(&self.name)
    }
}

/// A table's `title` and `key`, as `tv agent new` and `push` set them.
#[derive(Debug, Default, Clone, Copy)]
pub struct Meta<'a> {
    pub title: Option<&'a str>,
    pub key: Option<&'a str>,
}

impl Table {
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
            let mut wrapper = Map::new();
            wrapper.insert("rows".to_string(), value);
            value = Value::Object(wrapper);
        }
        // `get` is `None` for a field the text doesn't have, and for a
        // scalar, which serde then rejects below.
        let title = set
            .title
            .or(fill.title.filter(|_| value.get("title").is_none()));
        let kept_key = fill
            .key
            .filter(|_| set.key.is_none() && value.get("key").is_none());
        if let Value::Object(fields) = &mut value {
            for (name, chosen) in [("title", title), ("key", set.key)] {
                if let Some(chosen) = chosen {
                    fields.insert(name.to_string(), chosen.into());
                }
            }
        }
        let mut table: Table = serde_json::from_value(value)?;
        // A key the caller didn't ask for is puzzling in an error, so say
        // where it came from and how to drop it.
        if let Some(key) = kept_key {
            validate_key(key, &table.columns, &table.rows)
                .map_err(anyhow::Error::msg)
                .with_context(|| {
                    format!(
                        "kept the session's key \"{key}\", since the table names none; \
                         pass --key, or push the full format with \"key\": null to drop it"
                    )
                })?;
            table.key = Some(key.to_string());
        }
        Ok(table)
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

    /// Write the table, comments included, in the full format. Inferred
    /// columns and types are written out, so a shorthand file saves as a
    /// full one.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        files::write_json(path, self)
    }

    pub fn cell(&self, row: usize, column: &str) -> Option<&Value> {
        self.rows.get(row)?.get(column)
    }

    /// How a row is named to people and agents: the key value as the grid
    /// shows it (the same text the uniqueness check compares), or `#<index>`
    /// when the table has no key.
    pub fn row_id(&self, index: usize) -> String {
        match &self.key {
            Some(key) => cell_text(self.cell(index, key)),
            None => format!("#{index}"),
        }
    }

    /// Each row's ID (`row_id`) and its index. Row IDs are unique: a key's
    /// values are checked (`validate_key`), and `#<index>` can't repeat.
    pub fn row_indices(&self) -> HashMap<String, usize> {
        (0..self.rows.len())
            .map(|index| (self.row_id(index), index))
            .collect()
    }

    /// The index of the row whose ID (`row_id`) is `row_id`. `row_indices`
    /// is for looking up many rows; this is for one.
    pub fn row_index(&self, row_id: &str) -> Option<usize> {
        (0..self.rows.len()).find(|&index| self.row_id(index) == row_id)
    }

    /// The index of the column named `name`.
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|column| column.name == name)
    }

    pub fn key_column_index(&self) -> Option<usize> {
        self.column_index(self.key.as_deref()?)
    }

    /// The column a new viewer's cursor starts on: the key column, which is
    /// drawn first, or else the first column.
    pub fn start_column(&self) -> usize {
        self.key_column_index().unwrap_or(0)
    }
}

impl TryFrom<RawTable> for Table {
    type Error = String;

    fn try_from(raw: RawTable) -> Result<Self, Self::Error> {
        let mut columns = raw.columns;
        if columns.is_empty() {
            columns = infer_columns(&raw.rows);
        }
        for column in &mut columns {
            if column.kind.is_none() {
                column.kind = Some(infer_type(&raw.rows, &column.name));
            }
        }
        if let Some(key) = &raw.key {
            validate_key(key, &columns, &raw.rows)?;
        }
        Ok(Table {
            version: raw.version,
            title: raw.title,
            key: raw.key,
            columns,
            rows: raw.rows,
            comments: raw.comments,
        })
    }
}

/// The union of the rows' keys, in first-seen order.
fn infer_columns(rows: &[Row]) -> Vec<Column> {
    let mut seen = HashSet::new();
    let mut columns = Vec::new();
    for name in rows.iter().flat_map(Map::keys) {
        if seen.insert(name) {
            columns.push(Column {
                name: name.clone(),
                kind: None,
                width: None,
            });
        }
    }
    columns
}

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

fn scalar_type(value: &Value) -> Option<ColumnType> {
    match value {
        Value::Number(_) => Some(ColumnType::Number),
        Value::Bool(_) => Some(ColumnType::Bool),
        _ => None,
    }
}

fn validate_key(key: &str, columns: &[Column], rows: &[Row]) -> Result<(), String> {
    if !columns.iter().any(|column| column.name == key) {
        return Err(format!("key column \"{key}\" is not one of the columns"));
    }
    let mut first_row_for_value: HashMap<String, usize> = HashMap::new();
    for (index, row) in rows.iter().enumerate() {
        let value = row.get(key).filter(|value| !value.is_null());
        if value.is_none() {
            return Err(format!("row {index} has no value for key column \"{key}\""));
        }
        let text = cell_text(value);
        if let Some(first) = first_row_for_value.get(&text) {
            return Err(format!(
                "duplicate key value \"{text}\" at rows {first} and {index}"
            ));
        }
        first_row_for_value.insert(text, index);
    }
    Ok(())
}

/// How a cell reads in the grid: one line, nested values as compact JSON.
/// Missing and null cells are empty; render decides how to show them.
pub fn cell_text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => single_line(text),
        Some(other) => other.to_string(),
    }
}

/// Newlines become `↵`. Other control characters (tabs, carriage returns)
/// become spaces, since the terminal would otherwise move the cursor.
pub fn single_line(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '\n' => '↵',
            character if character.is_control() => ' ',
            character => character,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn column_names(table: &Table) -> Vec<&str> {
        table
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect()
    }

    fn error_message(text: &str) -> String {
        format!("{:#}", Table::parse(text).unwrap_err())
    }

    #[test]
    fn full_format_round_trips() {
        let text = std::fs::read_to_string("examples/prs.json").unwrap();
        let table = Table::parse(&text).unwrap();
        let again = Table::parse(&serde_json::to_string(&table).unwrap()).unwrap();
        assert_eq!(again.columns, table.columns);
        assert_eq!(again.rows, table.rows);
        assert_eq!(again.key.as_deref(), Some("number"));
        assert_eq!(again.title.as_deref(), Some("Open pull requests"));
    }

    /// Save `table` to a temp file and load it back. Returns the saved JSON too.
    fn save_and_reload(table: &Table) -> (Table, Value) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saved.json");
        table.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        (
            Table::parse(&text).unwrap(),
            serde_json::from_str(&text).unwrap(),
        )
    }

    #[test]
    fn save_round_trips_the_example() {
        let table = Table::from_path("examples/prs.json").unwrap();
        let (reloaded, _) = save_and_reload(&table);
        assert_eq!(reloaded.title, table.title);
        assert_eq!(reloaded.key, table.key);
        assert_eq!(reloaded.columns, table.columns);
        assert_eq!(reloaded.rows, table.rows);
    }

    #[test]
    fn shorthand_saves_in_the_full_format() {
        let table = Table::parse(r#"[{"n": 1, "s": "a"}, {"n": 2, "s": null}]"#).unwrap();
        let (reloaded, json) = save_and_reload(&table);
        assert_eq!(reloaded.columns, table.columns);
        assert_eq!(reloaded.rows, table.rows);
        let columns = json["columns"].as_array().unwrap();
        assert_eq!(columns.len(), 2);
        assert!(columns.iter().all(|column| column["type"].is_string()));
    }

    /// A table with one comment thread, as a saved file has it.
    const WITH_COMMENTS: &str = r#"{"key": "a", "rows": [{"a": 1}], "comments": [
        {"id": "c1", "row_id": "1", "column": "a", "messages": [
            {"id": "c1", "author": "user", "text": "hi", "ts": "2026-10-28T19:03:00Z"}
        ]}
    ]}"#;

    #[test]
    fn comments_parse_as_threads() {
        let table = Table::parse(WITH_COMMENTS).unwrap();
        assert_eq!(table.comments.len(), 1);
        assert_eq!(table.comments[0].id, "c1");
        assert_eq!(table.comments[0].messages[0].text, "hi");
    }

    #[test]
    fn a_thread_without_messages_is_an_error() {
        let message = error_message(
            r#"{"rows": [], "comments": [{"id": "c1", "row_id": "1", "column": "a"}]}"#,
        );
        assert!(message.contains("messages"), "{message}");
    }

    #[test]
    fn save_keeps_comments() {
        let table = Table::parse(WITH_COMMENTS).unwrap();
        let (reloaded, json) = save_and_reload(&table);
        assert_eq!(reloaded.comments, table.comments);
        assert_eq!(json["comments"][0]["messages"][0]["text"], "hi");
    }

    #[test]
    fn shorthand_infers_columns_in_first_seen_order() {
        let table = Table::parse(r#"[{"b": 1, "a": 2}, {"c": 3, "a": 4}]"#).unwrap();
        assert_eq!(column_names(&table), ["b", "a", "c"]);
        assert_eq!(table.version, 1);
        assert_eq!(table.rows.len(), 2);
    }

    #[test]
    fn infers_column_types() {
        let table = Table::parse(
            r#"[
                {"n": 1, "b": true, "mixed": 1, "nulls": null},
                {"n": 2.5, "b": false, "mixed": "x", "nulls": null},
                {"n": null}
            ]"#,
        )
        .unwrap();
        let kinds: Vec<_> = table.columns.iter().map(|column| column.kind).collect();
        assert_eq!(
            kinds,
            [
                Some(ColumnType::Number),
                Some(ColumnType::Bool),
                Some(ColumnType::String),
                Some(ColumnType::String),
            ]
        );
    }

    #[test]
    fn explicit_type_is_kept() {
        let table =
            Table::parse(r#"{"columns": [{"name": "n", "type": "string"}], "rows": [{"n": 1}]}"#)
                .unwrap();
        assert_eq!(table.columns[0].kind, Some(ColumnType::String));
    }

    #[test]
    fn key_must_be_a_column() {
        let message = error_message(r#"{"key": "id", "rows": [{"name": "a"}]}"#);
        assert!(message.contains("key column \"id\""), "{message}");
    }

    #[test]
    fn duplicate_key_names_both_rows() {
        let message = error_message(
            r#"{"key": "id", "rows": [{"id": 412}, {"id": 1}, {"id": 2}, {"id": 412}]}"#,
        );
        assert!(
            message.contains("duplicate key value \"412\" at rows 0 and 3"),
            "{message}"
        );
    }

    #[test]
    fn missing_key_value_errors() {
        let message = error_message(r#"{"key": "id", "rows": [{"id": 1}, {"id": null}, {}]}"#);
        assert!(message.contains("row 1 has no value"), "{message}");
    }

    fn meta<'a>(title: Option<&'a str>, key: Option<&'a str>) -> Meta<'a> {
        Meta { title, key }
    }

    #[test]
    fn parse_input_sets_a_title_and_key() {
        let rows = r#"[{"id": 1}, {"id": 2}]"#;
        let table = Table::parse_input(rows, meta(Some("T"), Some("id")), Meta::default()).unwrap();
        assert_eq!(
            (table.title.as_deref(), table.key.as_deref()),
            (Some("T"), Some("id"))
        );
        let file = r#"{"title": "old", "key": "x", "rows": [{"id": 1, "x": 1}]}"#;
        let table = Table::parse_input(file, meta(Some("T"), Some("id")), Meta::default()).unwrap();
        assert_eq!(
            (table.title.as_deref(), table.key.as_deref()),
            (Some("T"), Some("id"))
        );
    }

    #[test]
    fn a_set_key_is_validated_like_one_in_the_file() {
        let rows = r#"[{"id": 1}, {"id": 1}]"#;
        let error = Table::parse_input(rows, meta(None, Some("id")), Meta::default()).unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("duplicate key value \"1\" at rows 0 and 1"),
            "{message}"
        );
        assert!(!message.contains("kept"), "{message}");
    }

    #[test]
    fn fill_supplies_only_what_the_text_doesnt_have() {
        let fill = meta(Some("kept"), Some("id"));
        let table = Table::parse_input(r#"[{"id": 1}]"#, Meta::default(), fill).unwrap();
        assert_eq!(
            (table.title.as_deref(), table.key.as_deref()),
            (Some("kept"), Some("id"))
        );
        let own = r#"{"title": "own", "key": "n", "rows": [{"id": 1, "n": 1}]}"#;
        let table = Table::parse_input(own, Meta::default(), fill).unwrap();
        assert_eq!(
            (table.title.as_deref(), table.key.as_deref()),
            (Some("own"), Some("n"))
        );
        // `null` counts as naming the field: it's how a push drops a key.
        let nulls = r#"{"title": null, "key": null, "rows": [{"id": 1}]}"#;
        let table = Table::parse_input(nulls, Meta::default(), fill).unwrap();
        assert_eq!((table.title, table.key), (None, None));
        // What's set wins over `fill`.
        let table = Table::parse_input(r#"[{"id": 1}]"#, meta(Some("set"), None), fill).unwrap();
        assert_eq!(table.title.as_deref(), Some("set"));
    }

    #[test]
    fn a_kept_key_that_fails_says_where_it_came_from() {
        let fill = meta(None, Some("number"));
        for rows in [r#"[{"title": "a"}]"#, r#"[{"number": 1}, {"number": 1}]"#] {
            let error = Table::parse_input(rows, Meta::default(), fill).unwrap_err();
            let message = format!("{error:#}");
            assert!(
                message.starts_with("kept the session's key \"number\""),
                "{message}"
            );
            assert!(message.contains("\"key\": null"), "{message}");
        }
    }

    #[test]
    fn other_errors_dont_blame_the_kept_key() {
        let fill = meta(None, Some("number"));
        for text in ["[1, 2]", "5", r#"{"rows": 5}"#] {
            let error = Table::parse_input(text, Meta::default(), fill).unwrap_err();
            let message = format!("{error:#}");
            assert!(!message.contains("kept"), "{text}: {message}");
        }
    }

    #[test]
    fn from_input_path_applies_meta() {
        let table = Table::from_input_path(
            "examples/prs.json",
            meta(Some("PRs"), None),
            Meta::default(),
        )
        .unwrap();
        assert_eq!(table.title.as_deref(), Some("PRs"));
        assert_eq!(table.key.as_deref(), Some("number"));
        let error =
            Table::from_input_path("no/such.json", Meta::default(), Meta::default()).unwrap_err();
        assert!(format!("{error:#}").starts_with("reading no/such.json"));
    }

    #[test]
    fn parse_reads_only_json_and_parse_input_reads_any_format() {
        let lines = "{\"id\": \"a\"}\n{\"id\": \"b\"}\n";
        assert!(Table::parse(lines).is_err());
        let table = Table::parse_input(lines, meta(None, Some("id")), Meta::default()).unwrap();
        assert_eq!(table.row_id(1), "b");
    }

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

    #[test]
    fn rows_are_required() {
        let message = error_message(r#"{"title": "no rows field"}"#);
        assert!(message.contains("missing field `rows`"), "{message}");
    }

    #[test]
    fn key_column_index_finds_the_key() {
        let table = Table::parse(r#"{"key": "id", "rows": [{"name": "a", "id": 1}]}"#).unwrap();
        assert_eq!(table.key_column_index(), Some(1));
        let no_key = Table::parse(r#"[{"id": 1}]"#).unwrap();
        assert_eq!(no_key.key_column_index(), None);
    }

    #[test]
    fn row_id_uses_the_key_or_the_index() {
        let keyed = Table::parse(r#"{"key": "id", "rows": [{"id": 412}, {"id": "abc"}]}"#).unwrap();
        assert_eq!(keyed.row_id(0), "412");
        assert_eq!(keyed.row_id(1), "abc");
        let unkeyed = Table::parse(r#"[{"a": 1}, {"a": 2}]"#).unwrap();
        assert_eq!(unkeyed.row_id(1), "#1");
    }

    #[test]
    fn row_indices_map_row_ids_to_indices() {
        let keyed = Table::parse(r#"{"key": "id", "rows": [{"id": 412}, {"id": "abc"}]}"#).unwrap();
        let expected = HashMap::from([("412".to_string(), 0), ("abc".to_string(), 1)]);
        assert_eq!(keyed.row_indices(), expected);
        let unkeyed = Table::parse(r#"[{"a": 1}, {"a": 2}]"#).unwrap();
        let expected = HashMap::from([("#0".to_string(), 0), ("#1".to_string(), 1)]);
        assert_eq!(unkeyed.row_indices(), expected);
    }

    #[test]
    fn row_index_finds_one_row_by_id() {
        let keyed = Table::parse(r#"{"key": "id", "rows": [{"id": 412}, {"id": "abc"}]}"#).unwrap();
        assert_eq!(keyed.row_index("412"), Some(0));
        assert_eq!(keyed.row_index("abc"), Some(1));
        assert_eq!(keyed.row_index("#1"), None);
        let unkeyed = Table::parse(r#"[{"a": 1}, {"a": 2}]"#).unwrap();
        assert_eq!(unkeyed.row_index("#1"), Some(1));
        assert_eq!(unkeyed.row_index("#2"), None);
    }

    #[test]
    fn column_index_finds_a_column_by_name() {
        let table = Table::from_path("examples/prs.json").unwrap();
        assert_eq!(table.column_index("number"), Some(0));
        assert_eq!(table.column_index("title"), Some(1));
        assert_eq!(table.column_index("nope"), None);
    }

    #[test]
    fn start_column_is_the_key_column_or_the_first() {
        let keyed_second =
            r#"{"key": "id", "columns": [{"name": "a"}, {"name": "id"}], "rows": []}"#;
        assert_eq!(Table::parse(keyed_second).unwrap().start_column(), 1);
        assert_eq!(
            Table::from_path("examples/prs.json")
                .unwrap()
                .start_column(),
            0
        );
        assert_eq!(
            Table::parse(r#"[{"a": 1, "b": 2}]"#)
                .unwrap()
                .start_column(),
            0
        );
    }

    #[test]
    fn cell_text_renders_each_kind() {
        assert_eq!(cell_text(None), "");
        assert_eq!(cell_text(Some(&Value::Null)), "");
        assert_eq!(cell_text(Some(&json!(412))), "412");
        assert_eq!(cell_text(Some(&json!(2.5))), "2.5");
        assert_eq!(cell_text(Some(&json!(true))), "true");
        assert_eq!(
            cell_text(Some(&json!("two\nlines\tand tab"))),
            "two↵lines and tab"
        );
        assert_eq!(cell_text(Some(&json!({"a": [1, 2]}))), r#"{"a":[1,2]}"#);
    }
}
