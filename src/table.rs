//! The table format: types, parsing (including the bare-array shorthand),
//! column and type inference, and validation. See `docs/schema.md`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::files;

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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Value>, // opaque until ADI-223 defines the type
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
    comments: Vec<Value>,
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

impl Table {
    /// Parse a table from JSON text, accepting either the full format or a bare
    /// array of row objects.
    pub fn parse(text: &str) -> anyhow::Result<Table> {
        let mut value: Value = serde_json::from_str(text)?;
        if value.is_array() {
            let mut wrapper = Map::new();
            wrapper.insert("rows".to_string(), value);
            value = Value::Object(wrapper);
        }
        Ok(serde_json::from_value(value)?)
    }

    pub fn from_path(path: impl AsRef<Path>) -> anyhow::Result<Table> {
        let path = path.as_ref();
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Table::parse(&text).with_context(|| format!("parsing {}", path.display()))
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

    pub fn key_column_index(&self) -> Option<usize> {
        let key = self.key.as_deref()?;
        self.columns.iter().position(|column| column.name == key)
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
    let mut kinds = rows
        .iter()
        .filter_map(|row| row.get(column))
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

    #[test]
    fn save_keeps_comments() {
        let table =
            Table::parse(r#"{"rows": [{"a": 1}], "comments": [{"id": "c1", "text": "hi"}]}"#)
                .unwrap();
        let (reloaded, _) = save_and_reload(&table);
        assert_eq!(reloaded.comments, table.comments);
        assert_eq!(reloaded.comments.len(), 1);
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
