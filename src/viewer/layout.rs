//! Column geometry: widths, which columns fit on screen, and sizing cell text.
//! Pure functions over numbers and strings, so they're easy to unit test.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::table::{Table, cell_text};

/// Max column width when the table doesn't set `columns[].width`.
pub const DEFAULT_WIDTH_CAP: u16 = 40;
/// Spaces on each side of a cell's text.
pub const CELL_PADDING: u16 = 1;
/// Drawn after every column.
pub const SEPARATOR: &str = "│";
const SEPARATOR_WIDTH: u16 = 1;
const ELLIPSIS: char = '…';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

/// How many terminal cells `text` takes. Wide characters (CJK, most emoji)
/// take two, so this is not the same as `.len()` or `.chars().count()`.
pub fn display_width(text: &str) -> u16 {
    u16::try_from(text.width()).unwrap_or(u16::MAX)
}

/// Width of every column: `min(cap, max(header, widest cell))`, at least 1 so
/// a truncated cell still has room for `…`.
pub fn column_widths(table: &Table) -> Vec<u16> {
    table
        .columns
        .iter()
        .map(|column| {
            let widest_cell = table
                .rows
                .iter()
                .map(|row| display_width(&cell_text(row.get(&column.name))))
                .max()
                .unwrap_or(0);
            let cap = column.width.unwrap_or(DEFAULT_WIDTH_CAP);
            widest_cell.max(display_width(&column.name)).min(cap).max(1)
        })
        .collect()
}

/// Screen width one column takes: padding on both sides plus the separator.
/// `visible_columns` budgets with this and render draws with the same constants,
/// so the two can't disagree.
pub fn column_span(width: u16) -> u16 {
    width
        .saturating_add(2 * CELL_PADDING)
        .saturating_add(SEPARATOR_WIDTH)
}

/// Column indices to draw, left to right: the pinned column (if any), then the
/// non-pinned columns from `first_col` on that fully fit in `total_width`.
///
/// One exception: the first non-pinned column is always included, even if it
/// doesn't fit (render clips it). That way every column can be scrolled into
/// view, however narrow the terminal.
pub fn visible_columns(
    widths: &[u16],
    pinned: Option<usize>,
    first_col: usize,
    total_width: u16,
) -> Vec<usize> {
    let mut visible: Vec<usize> = pinned.into_iter().collect();
    let mut used: u32 = visible
        .iter()
        .map(|&column| u32::from(column_span(widths[column])))
        .sum();
    let scrolled = (first_col..widths.len()).filter(|&column| Some(column) != pinned);
    for (position, column) in scrolled.enumerate() {
        let span = u32::from(column_span(widths[column]));
        if position > 0 && used + span > u32::from(total_width) {
            break;
        }
        used += span;
        visible.push(column);
    }
    visible
}

/// Make `text` exactly `width` display cells: truncate with `…` if it's too
/// long, then pad on the side `align` says. This is the only place cell text
/// is sized.
pub fn fit(text: &str, width: u16, align: Align) -> String {
    let width = usize::from(width);
    let fitted = if text.width() > width {
        truncate(text, width)
    } else {
        text.to_string()
    };
    let padding = " ".repeat(width.saturating_sub(fitted.width()));
    match align {
        Align::Left => fitted + &padding,
        Align::Right => padding + &fitted,
    }
}

/// Cut `text` to fit in `width - 1` cells and add `…`. A wide character that
/// would straddle the limit is dropped whole; `fit` pads the gap.
fn truncate(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let budget = width - 1;
    let mut used = 0;
    let mut truncated = String::new();
    for character in text.chars() {
        let character_width = character.width().unwrap_or(0);
        if used + character_width > budget {
            break;
        }
        used += character_width;
        truncated.push(character);
    }
    truncated.push(ELLIPSIS);
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use Align::{Left, Right};

    fn widths_of(json: &str) -> Vec<u16> {
        column_widths(&Table::parse(json).unwrap())
    }

    #[test]
    fn widths_take_the_widest_of_header_and_cells() {
        assert_eq!(
            widths_of(r#"[{"id": 7, "description": "x"}, {"id": 1234567, "description": "y"}]"#),
            [7, 11]
        );
    }

    #[test]
    fn widths_are_capped() {
        let long = "x".repeat(100);
        assert_eq!(
            widths_of(&format!(r#"[{{"a": "{long}"}}]"#)),
            [DEFAULT_WIDTH_CAP]
        );
        assert_eq!(
            widths_of(&format!(
                r#"{{"columns": [{{"name": "a", "width": 10}}], "rows": [{{"a": "{long}"}}]}}"#
            )),
            [10]
        );
    }

    #[test]
    fn widths_are_at_least_one() {
        assert_eq!(
            widths_of(r#"{"columns": [{"name": "a", "width": 0}], "rows": [{"a": "abc"}]}"#),
            [1]
        );
    }

    #[test]
    fn widths_with_no_rows_are_header_widths() {
        assert_eq!(
            widths_of(r#"{"columns": [{"name": "id"}, {"name": "title"}], "rows": []}"#),
            [2, 5]
        );
    }

    #[test]
    fn display_width_counts_wide_characters() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("日本語"), 6);
        assert_eq!(display_width("🦀"), 2);
    }

    #[test]
    fn fit_pads_short_text() {
        assert_eq!(fit("ab", 4, Left), "ab  ");
        assert_eq!(fit("12", 4, Right), "  12");
        assert_eq!(fit("abcd", 4, Left), "abcd");
    }

    #[test]
    fn fit_truncates_long_text() {
        assert_eq!(fit("hello world", 5, Left), "hell…");
        assert_eq!(fit("1234567", 4, Right), "123…");
        assert_eq!(fit("hello", 1, Left), "…");
        assert_eq!(fit("hello", 0, Left), "");
    }

    #[test]
    fn fit_drops_a_wide_character_that_would_straddle() {
        assert_eq!(fit("日本語", 3, Left), "日…");
        assert_eq!(fit("日本語", 4, Left), "日… ");
        assert_eq!(fit("日本語", 4, Right), " 日…");
        assert_eq!(display_width(&fit("🦀🦀🦀", 4, Left)), 4);
    }

    #[test]
    fn column_span_includes_padding_and_separator() {
        assert_eq!(column_span(5), 8);
    }

    #[test]
    fn visible_columns_without_pin_stop_before_a_partial_column() {
        // Spans are 8 each.
        assert_eq!(visible_columns(&[5, 5, 5], None, 0, 20), [0, 1]);
        assert_eq!(visible_columns(&[5, 5, 5], None, 0, 24), [0, 1, 2]);
        assert_eq!(visible_columns(&[5, 5, 5], None, 1, 24), [1, 2]);
    }

    #[test]
    fn visible_columns_put_the_pinned_column_first() {
        // Spans: 9, 13, 13, 13.
        let widths = [6, 10, 10, 10];
        assert_eq!(visible_columns(&widths, Some(0), 2, 40), [0, 2, 3]);
        assert_eq!(visible_columns(&widths, Some(2), 0, 40), [2, 0, 1]);
    }

    #[test]
    fn visible_columns_skip_the_pinned_column_when_scrolled_to_it() {
        let widths = [6, 10, 10, 10];
        assert_eq!(
            visible_columns(&widths, Some(0), 0, 40),
            visible_columns(&widths, Some(0), 1, 40)
        );
    }

    #[test]
    fn visible_columns_always_include_the_first_scrolled_column() {
        // Spans: 9, 43. Only 30 cells, but column 1 must still be reachable.
        assert_eq!(visible_columns(&[6, 40, 5], Some(0), 1, 30), [0, 1]);
        assert_eq!(visible_columns(&[40, 5], None, 0, 10), [0]);
    }
}
