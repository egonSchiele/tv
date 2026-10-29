//! Column geometry: widths, which columns fit on screen, and sizing cell text.
//! Pure functions over numbers and strings, so they're easy to unit test.

use unicode_width::UnicodeWidthStr;

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
            widest_cell
                .max(display_width(&column.display_name()))
                .min(cap)
                .max(1)
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

/// Column indices in the order they're drawn: the pinned column first, then
/// the rest in table order. Cursor movement, the record view, and
/// `visible_columns` all use this, so they agree on what's left and right.
pub fn screen_order(count: usize, pinned: Option<usize>) -> Vec<usize> {
    pinned
        .into_iter()
        .chain((0..count).filter(|&index| Some(index) != pinned))
        .collect()
}

/// A column to draw and the width to draw it at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisibleColumn {
    pub index: usize,
    pub width: u16,
}

/// Columns to draw, left to right: the pinned column (if any), then the
/// non-pinned columns from `first_col` on that fully fit in `total_width`.
///
/// One exception: the pinned column and the first non-pinned column are always
/// included. If one doesn't fit, it is narrowed to the space left, so its text
/// is truncated with `…` (or clipped, if no space is left). That way every
/// column can be scrolled into view, however narrow the terminal.
///
/// The last column's separator may land just past `total_width`, where the
/// grid's right border is. Render clips it and the border draws the same line.
pub fn visible_columns(
    widths: &[u16],
    pinned: Option<usize>,
    first_col: usize,
    total_width: u16,
) -> Vec<VisibleColumn> {
    let budget = u32::from(total_width) + u32::from(SEPARATOR_WIDTH);
    let always_shown = 1 + usize::from(pinned.is_some());
    let shown = screen_order(widths.len(), pinned)
        .into_iter()
        .filter(|&index| Some(index) == pinned || index >= first_col);
    let mut visible = Vec::new();
    let mut used: u32 = 0;
    for (position, index) in shown.enumerate() {
        let remaining = budget.saturating_sub(used);
        let fits = u32::from(column_span(widths[index])) <= remaining;
        if !fits && position >= always_shown {
            break;
        }
        let width = if fits {
            widths[index]
        } else {
            width_within(remaining)
        };
        used += u32::from(column_span(width));
        visible.push(VisibleColumn { index, width });
    }
    visible
}

/// The widest column whose span fits in `space`, but at least 1.
fn width_within(space: u32) -> u16 {
    let overhead = u32::from(column_span(0));
    u16::try_from(space.saturating_sub(overhead))
        .unwrap_or(u16::MAX)
        .max(1)
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
///
/// Measures the whole string as it grows rather than adding up character
/// widths: they disagree for sequences like `❤️` (U+2764 + U+FE0F), which is 2
/// cells as a string but 1 + 0 as characters.
fn truncate(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let budget = width - 1;
    let mut truncated = String::new();
    for character in text.chars() {
        truncated.push(character);
        if truncated.width() > budget {
            truncated.pop();
            break;
        }
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
    fn widths_measure_the_displayed_header() {
        // `a\nb` displays as `a↵b`, 3 cells.
        assert_eq!(
            widths_of(r#"{"columns": [{"name": "a\nb"}], "rows": []}"#),
            [3]
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
        // A variation selector makes ❤️ 2 cells wide, though its characters are 1 + 0.
        assert_eq!(display_width(&fit("❤️❤️❤️❤️", 4, Left)), 4);
        assert_eq!(display_width(&fit("❤️❤️❤️❤️", 5, Left)), 5);
    }

    #[test]
    fn screen_order_puts_the_pinned_column_first() {
        assert_eq!(screen_order(4, None), [0, 1, 2, 3]);
        assert_eq!(screen_order(4, Some(2)), [2, 0, 1, 3]);
        assert_eq!(screen_order(0, None), Vec::<usize>::new());
    }

    #[test]
    fn column_span_includes_padding_and_separator() {
        assert_eq!(column_span(5), 8);
    }

    /// Shorthand for expected results: `(index, width)` pairs.
    fn columns(pairs: &[(usize, u16)]) -> Vec<VisibleColumn> {
        pairs
            .iter()
            .map(|&(index, width)| VisibleColumn { index, width })
            .collect()
    }

    #[test]
    fn visible_columns_without_pin_stop_before_a_partial_column() {
        // Spans are 8 each. The budget is the width plus 1, because the last
        // separator can sit on the border.
        let widths = [5, 5, 5];
        assert_eq!(
            visible_columns(&widths, None, 0, 20),
            columns(&[(0, 5), (1, 5)])
        );
        assert_eq!(
            visible_columns(&widths, None, 0, 24),
            columns(&[(0, 5), (1, 5), (2, 5)])
        );
        assert_eq!(
            visible_columns(&widths, None, 1, 24),
            columns(&[(1, 5), (2, 5)])
        );
    }

    #[test]
    fn visible_columns_put_the_pinned_column_first() {
        // Spans: 9, 13, 13, 13.
        let widths = [6, 10, 10, 10];
        assert_eq!(
            visible_columns(&widths, Some(0), 2, 40),
            columns(&[(0, 6), (2, 10), (3, 10)])
        );
        assert_eq!(
            visible_columns(&widths, Some(2), 0, 40),
            columns(&[(2, 10), (0, 6), (1, 10)])
        );
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
    fn visible_columns_narrow_the_first_scrolled_column_to_fit() {
        // Budget 31; pinned span 9 leaves 22; minus padding and separator, 19.
        assert_eq!(
            visible_columns(&[6, 40, 5], Some(0), 1, 30),
            columns(&[(0, 6), (1, 19)])
        );
        assert_eq!(visible_columns(&[40, 5], None, 0, 10), columns(&[(0, 8)]));
    }

    #[test]
    fn visible_columns_keep_both_shown_columns_with_no_space_left() {
        // The pinned column is narrowed to fill the screen; the first scrolled
        // column still comes back (at width 1, off screen) so scrolling can
        // find it.
        assert_eq!(
            visible_columns(&[40, 5], Some(0), 1, 10),
            columns(&[(0, 8), (1, 1)])
        );
    }
}
