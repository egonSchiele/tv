//! Comparing a table with its reloaded version: where the cursor goes, and
//! which cells to highlight. Pure functions over two tables, so they're
//! tested without a terminal.

use std::collections::HashSet;

use super::app::Cursor;
use crate::table::Table;

/// Where `cursor` in `old` is in `new`: the row with the same row ID and the
/// column with the same name. A row or column that's gone keeps its index,
/// which the caller clamps. Unkeyed tables have `#index` row IDs, so their
/// cursor stays by position. A cursor that isn't on a cell of `old` (an
/// empty table's `(0, 0)`) is returned as it is.
pub fn same_cell(old: &Table, cursor: Cursor, new: &Table) -> Cursor {
    // Check before reading the ID: on a keyed table, `row_id` of a missing
    // row is `""`, which could match a new row whose key is empty.
    let Some(column) = old.columns.get(cursor.col) else {
        return cursor;
    };
    if cursor.row >= old.rows.len() {
        return cursor;
    }
    let row_id = old.row_id(cursor.row);
    let row = (0..new.rows.len())
        .find(|&index| new.row_id(index) == row_id)
        .unwrap_or(cursor.row);
    let col = new
        .columns
        .iter()
        .position(|candidate| candidate.name == column.name)
        .unwrap_or(cursor.col);
    Cursor { row, col }
}

/// The cells to highlight after a reload: every cell of a row whose ID isn't
/// in `old` (a new row), and every cell whose value differs from the cell with
/// the same row ID and column name in `old`. A column `old` doesn't have
/// counts as changed in every row. Indices are into `new`. Deleted rows and
/// columns highlight nothing: there's no cell left to color.
///
/// Without a key, rows match by index, so a row inserted at the top marks
/// every row below it as changed. That's expected, not a bug.
pub fn changed_cells(old: &Table, new: &Table) -> HashSet<Cursor> {
    let old_rows = old.row_indices();
    let old_columns: HashSet<&str> = old
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect();
    let mut changed = HashSet::new();
    for new_row in 0..new.rows.len() {
        let old_row = old_rows.get(&new.row_id(new_row)).copied();
        for (col, column) in new.columns.iter().enumerate() {
            let name = column.name.as_str();
            // Missing and `null` compare unequal here. A cell going from
            // absent to `null` is a change nobody will notice either way.
            let unchanged = old_row.is_some_and(|old_row| {
                old_columns.contains(name) && old.cell(old_row, name) == new.cell(new_row, name)
            });
            if !unchanged {
                changed.insert(Cursor { row: new_row, col });
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(json: &str) -> Table {
        Table::parse(json).unwrap()
    }

    fn cell(row: usize, col: usize) -> Cursor {
        Cursor { row, col }
    }

    fn cells(list: &[(usize, usize)]) -> HashSet<Cursor> {
        list.iter().map(|&(row, col)| cell(row, col)).collect()
    }

    const KEYED: &str = r#"{"key": "id", "rows": [
        {"id": "a", "n": 1, "s": "x"},
        {"id": "b", "n": 2, "s": "y"},
        {"id": "c", "n": 3, "s": "z"}
    ]}"#;

    #[test]
    fn the_cursor_follows_its_row_id() {
        let old = table(KEYED);
        let reordered = table(
            r#"{"key": "id", "rows": [{"id": "c", "n": 3, "s": "z"}, {"id": "a", "n": 1, "s": "x"}, {"id": "b", "n": 2, "s": "y"}]}"#,
        );
        assert_eq!(same_cell(&old, cell(1, 2), &reordered), cell(2, 2));
    }

    #[test]
    fn a_deleted_row_keeps_its_index() {
        let old = table(KEYED);
        let without_b = table(
            r#"{"key": "id", "rows": [{"id": "a", "n": 1, "s": "x"}, {"id": "c", "n": 3, "s": "z"}]}"#,
        );
        assert_eq!(same_cell(&old, cell(1, 1), &without_b), cell(1, 1));
    }

    #[test]
    fn the_cursor_follows_its_column_name() {
        let old = table(KEYED);
        let columns_reordered = table(
            r#"{"key": "id", "columns": [{"name": "s"}, {"name": "id"}, {"name": "n"}], "rows": [{"id": "a", "n": 1, "s": "x"}]}"#,
        );
        assert_eq!(same_cell(&old, cell(0, 1), &columns_reordered), cell(0, 2));
        let without_n = table(r#"{"key": "id", "rows": [{"id": "a", "s": "x"}]}"#);
        assert_eq!(same_cell(&old, cell(0, 1), &without_n), cell(0, 1));
    }

    #[test]
    fn an_unkeyed_cursor_stays_by_position() {
        let old = table(r#"[{"n": 1}, {"n": 2}, {"n": 3}]"#);
        let new = table(r#"[{"n": 0}, {"n": 1}, {"n": 2}, {"n": 3}]"#);
        assert_eq!(same_cell(&old, cell(2, 0), &new), cell(2, 0));
    }

    #[test]
    fn a_cursor_from_an_empty_table_is_unchanged() {
        let full = table(KEYED);
        for empty in [r#"{"columns": [{"name": "id"}], "rows": []}"#, "[]", "[{}]"] {
            assert_eq!(
                same_cell(&table(empty), cell(0, 0), &full),
                cell(0, 0),
                "{empty}"
            );
        }
    }

    #[test]
    fn one_changed_cell() {
        let new = table(&KEYED.replace(r#""n": 2"#, r#""n": 20"#));
        assert_eq!(changed_cells(&table(KEYED), &new), cells(&[(1, 1)]));
    }

    #[test]
    fn a_new_row_is_changed_everywhere_and_a_deleted_row_nowhere() {
        let old = table(KEYED);
        let added = table(&KEYED.replace(
            r#"{"id": "c", "n": 3, "s": "z"}"#,
            r#"{"id": "c", "n": 3, "s": "z"}, {"id": "d", "n": 4, "s": "w"}"#,
        ));
        assert_eq!(
            changed_cells(&old, &added),
            cells(&[(3, 0), (3, 1), (3, 2)])
        );
        let deleted = table(
            r#"{"key": "id", "rows": [{"id": "a", "n": 1, "s": "x"}, {"id": "c", "n": 3, "s": "z"}]}"#,
        );
        assert_eq!(changed_cells(&old, &deleted), HashSet::new());
    }

    #[test]
    fn a_renamed_column_is_changed_in_every_row() {
        let renamed = table(&KEYED.replace(r#""s":"#, r#""t":"#));
        assert_eq!(
            changed_cells(&table(KEYED), &renamed),
            cells(&[(0, 2), (1, 2), (2, 2)])
        );
    }

    #[test]
    fn identical_tables_change_nothing() {
        assert_eq!(changed_cells(&table(KEYED), &table(KEYED)), HashSet::new());
    }

    #[test]
    fn everything_is_new_after_an_empty_table() {
        let new = table(r#"[{"a": 1, "b": 2}, {"a": 3, "b": 4}]"#);
        let all = cells(&[(0, 0), (0, 1), (1, 0), (1, 1)]);
        assert_eq!(changed_cells(&table("[]"), &new), all);
    }

    #[test]
    fn a_number_becoming_a_string_is_a_change() {
        let new = table(&KEYED.replace(r#""n": 1"#, r#""n": "1""#));
        assert_eq!(changed_cells(&table(KEYED), &new), cells(&[(0, 1)]));
    }
}
