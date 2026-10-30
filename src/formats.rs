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
