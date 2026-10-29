//! Drawing. Pure: takes `&App` and draws a frame, never changes state.

use std::iter;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use serde_json::Value;

use super::app::{
    App, Binding, ColonCommand, CommandLine, DETAIL_BINDINGS, GRID_BINDINGS, Marker, Mode,
    RECORD_BINDINGS, key_label,
};
use super::layout::{self, Align, VisibleColumn};
use crate::comments::{Author, Message, Thread};
use crate::table::{ColumnType, cell_text, single_line};

/// Where each part of the screen goes.
pub struct Areas {
    pub header: Rect,
    pub body: Rect,
    /// The full screen width, so the divider can join the side borders.
    pub divider: Rect,
    pub status: Rect,
}

/// The screen layout. `draw` uses it, and the event loop uses it to tell `App`
/// how big the grid is, so the two can't disagree.
pub fn areas(screen: Rect) -> Areas {
    let inner = Block::bordered().inner(screen);
    let [header, body, divider, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let divider = Rect {
        x: screen.x,
        width: screen.width,
        ..divider
    };
    Areas {
        header,
        body,
        divider,
        status,
    }
}

pub fn draw(frame: &mut Frame, app: &App) {
    let screen = frame.area();
    let table = app.table();
    let areas = areas(screen);

    let mut block = Block::bordered();
    if let Some(title) = &table.title {
        let title = single_line(title);
        block = block.title_top(Line::from(format!(" {title} ")).left_aligned());
    }
    if let Some(session_id) = &app.session_id {
        block = block.title_top(Line::from(format!(" {session_id} ")).right_aligned());
    }
    frame.render_widget(block, screen);

    if app.mode == Mode::Record {
        draw_record(frame, app, areas.header.union(areas.body));
    } else {
        let visible = app.visible_columns();
        frame.render_widget(header_line(app, &visible, areas.header.width), areas.header);
        frame.render_widget(
            Paragraph::new(body_lines(app, &visible, areas.body)),
            areas.body,
        );
    }
    frame.render_widget(Line::from(divider(screen.width)), areas.divider);
    draw_status(frame, app, areas.status);

    match app.mode {
        Mode::Help => draw_help(frame, screen),
        // The delete prompt is in the status line, over the popup.
        Mode::Detail | Mode::ConfirmDelete(_) => draw_detail(frame, app, screen),
        _ => {}
    }
}

fn header_line(app: &App, visible: &[VisibleColumn], width: u16) -> Line<'static> {
    let columns = &app.table().columns;
    let header = app.theme.header;
    grid_line(visible, width, Style::new(), |col| GridCell {
        text: columns[col].display_name(),
        align: align(columns[col].kind),
        style: header,
        marker: None,
    })
}

fn body_lines(app: &App, visible: &[VisibleColumn], area: Rect) -> Vec<Line<'static>> {
    let (width, height) = (area.width, area.height);
    let table = app.table();
    if table.rows.is_empty() {
        return vec![Line::from(" No rows")];
    }
    (app.first_row..table.rows.len())
        .take(usize::from(height))
        .map(|row| {
            let row_style = if row == app.cursor.row {
                app.theme.cursor_line
            } else {
                Style::new()
            };
            grid_line(visible, width, row_style, |col| {
                let column = &table.columns[col];
                GridCell {
                    text: cell_text(table.cell(row, &column.name)),
                    align: align(column.kind),
                    style: cell_style(app, row, col),
                    marker: app.marker(row, col).map(|marker| match marker {
                        Marker::Awaiting => app.theme.awaiting,
                        Marker::Replied => app.theme.replied,
                    }),
                }
            })
        })
        .collect()
}

/// The one place that decides how a body cell looks: the cursor cell is
/// brightest, then the cursor's row and column, then everything else. A cell
/// the last reload changed is highlighted over its row and column, but not
/// on the cursor: that one stays reversed, the brightest thing on screen.
/// Comment markers aren't part of this: `grid_line` draws them in the right
/// padding.
fn cell_style(app: &App, row: usize, col: usize) -> Style {
    let theme = &app.theme;
    let cursor = app.cursor;
    let on_cursor = (row, col) == (cursor.row, cursor.col);
    let mut style = if on_cursor {
        theme.cursor_cell
    } else if row == cursor.row || col == cursor.col {
        theme.cursor_line
    } else {
        Style::new()
    };
    if app.is_highlighted(row, col) && !on_cursor {
        style = style.patch(theme.changed);
    }
    let table = app.table();
    if table
        .cell(row, &table.columns[col].name)
        .is_none_or(Value::is_null)
    {
        style = style.patch(theme.null);
    }
    style
}

fn align(kind: Option<ColumnType>) -> Align {
    if kind == Some(ColumnType::Number) {
        Align::Right
    } else {
        Align::Left
    }
}

/// Marks a cell with comment threads, in the cell's right padding.
const MARKER: &str = "▸";

/// What `grid_line` draws in one column.
struct GridCell {
    text: String,
    align: Align,
    style: Style,
    /// The comment marker's style, if the cell has threads.
    marker: Option<Style>,
}

/// The only code that lays out a grid row: each visible column's text is
/// sized with `layout::fit`, padded, and followed by the separator. The header
/// and body rows both go through here; `cell` says what goes in each column.
/// Separators take `row_style`, so a highlighted row isn't striped at each `│`.
///
/// When the row fills `width` exactly, the last separator would sit against
/// the right border and look doubled (`││`), so it's dropped.
fn grid_line(
    visible: &[VisibleColumn],
    width: u16,
    row_style: Style,
    cell: impl Fn(usize) -> GridCell,
) -> Line<'static> {
    let padding = " ".repeat(usize::from(layout::CELL_PADDING));
    let spans: Vec<Span> = visible
        .iter()
        .flat_map(|column| {
            let GridCell {
                text,
                align,
                style,
                marker,
            } = cell(column.index);
            let sized = layout::fit(&text, column.width, align);
            // The marker takes the right padding, so it never hides text or
            // changes a column's width. With `CELL_PADDING` at 1, that's all
            // of the padding.
            let right = match marker {
                Some(marker) => Span::styled(MARKER, style.patch(marker)),
                None => Span::styled(padding.clone(), style),
            };
            [
                Span::styled(format!("{padding}{sized}"), style),
                right,
                Span::styled(layout::SEPARATOR, row_style),
            ]
        })
        .collect();
    let mut line = Line::from(spans);
    if line.width() == usize::from(width) {
        line.spans.pop();
    }
    line
}

fn divider(width: u16) -> String {
    let middle = "─".repeat(usize::from(width.saturating_sub(2)));
    format!("├{middle}┤")
}

/// Key hints after the position, unless a message or a reload error is
/// showing. The full list is in the help overlay.
const STATUS_HINTS: &str = "? help  : command  q quit";

/// Asked in the status line before `y` deletes a cell's threads.
const DELETE_PROMPT: &str = " delete this cell's comments? y/n";

/// What the status row shows in each mode: an input line while one is being
/// typed, the delete prompt, or the status line.
fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    match &app.mode {
        Mode::Command(line) => draw_input(frame, area, ":", line),
        Mode::Comment(input) => draw_input(frame, area, "comment: ", &input.line),
        Mode::ConfirmDelete(_) => frame.render_widget(Line::from(DELETE_PROMPT), area),
        _ => frame.render_widget(status_line(app), area),
    }
}

/// An input line: `prefix`, then the text, with the terminal's cursor where
/// typing goes, measured in cells, not bytes. When the text before the cursor
/// doesn't fit, characters are dropped from the front until it does, so what
/// you're typing stays on screen.
fn draw_input(frame: &mut Frame, area: Rect, prefix: &str, line: &CommandLine) {
    let prefix_width = layout::display_width(prefix);
    // Room for the text before the cursor, leaving a cell for the cursor.
    let room = area.width.saturating_sub(prefix_width.saturating_add(1));
    let before = line.before_cursor();
    let start = before
        .char_indices()
        .map(|(index, _)| index)
        .chain([before.len()])
        .find(|&index| layout::display_width(&before[index..]) <= room)
        .unwrap_or(before.len());
    frame.render_widget(Line::from(format!("{prefix}{}", &line.text[start..])), area);
    let offset = prefix_width.saturating_add(layout::display_width(&before[start..]));
    let x = area
        .x
        .saturating_add(offset)
        .min(area.right().saturating_sub(1));
    frame.set_cursor_position((x, area.y));
}

/// Status-line hints in the record view.
const RECORD_HINTS: &str = "j/k field  n/p row  Esc back";

fn status_line(app: &App) -> Line<'static> {
    let table = app.table();
    let (rows, cols) = (table.rows.len(), table.columns.len());
    // Positions count from 1 and go by screen order, as the user sees them.
    let (row, col) = (app.cursor.row + 1, app.cursor_position() + 1);
    let (position, hints) = if rows == 0 {
        ("no rows".to_string(), STATUS_HINTS)
    } else if app.mode == Mode::Record {
        (
            format!("row {row}/{rows}  field {col}/{cols}"),
            RECORD_HINTS,
        )
    } else {
        (format!("row {row}/{rows}  col {col}/{cols}"), STATUS_HINTS)
    };
    // A message (a save result, say) wins while it's up; a reload error
    // stays until a reload succeeds, so it comes back when the message goes.
    let after = match (&app.message, &app.reload_error) {
        (Some(message), _) => message.text.clone(),
        (None, Some(error)) => format!("reload failed: {error}"),
        (None, None) => hints.to_string(),
    };
    let mut parts = vec![position];
    parts.extend(comment_counts(app));
    parts.push(after);
    Line::from(format!(" {}", parts.join("  ·  ")))
}

/// `1 awaiting reply` and `1 orphaned` for the status line, leaving out a
/// zero.
fn comment_counts(app: &App) -> Vec<String> {
    let threads = app.threads();
    let awaiting = threads
        .iter()
        .filter(|thread| thread.awaiting_reply)
        .count();
    let orphaned = threads.iter().filter(|thread| thread.orphaned).count();
    [(awaiting, "awaiting reply"), (orphaned, "orphaned")]
        .into_iter()
        .filter(|&(count, _)| count > 0)
        .map(|(count, what)| format!("{count} {what}"))
        .collect()
}

/// A centered box listing every key and `:` command. It's built from the
/// bindings tables and `ColonCommand::HELP`, so it always matches what works.
fn draw_help(frame: &mut Frame, screen: Rect) {
    let sections: [(&str, Vec<(String, &str)>); 4] = [
        ("Grid", binding_rows(GRID_BINDINGS)),
        ("Cell detail", binding_rows(DETAIL_BINDINGS)),
        ("Record view", binding_rows(RECORD_BINDINGS)),
        (
            "Commands",
            ColonCommand::HELP
                .iter()
                .map(|&(name, help)| (name.to_string(), help))
                .collect(),
        ),
    ];
    let label_width = sections
        .iter()
        .flat_map(|(_, rows)| rows)
        .map(|(label, _)| layout::display_width(label))
        .max()
        .unwrap_or(0);
    let bold = Style::new().add_modifier(Modifier::BOLD);

    let mut lines: Vec<Line> = Vec::new();
    for (heading, rows) in &sections {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(Line::styled(format!(" {heading}"), bold));
        for (label, help) in rows {
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {} ", layout::fit(label, label_width, Align::Right)),
                    bold,
                ),
                Span::raw(format!(" {help} ")),
            ]));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(" Press any key to close").dim());

    let width = lines.iter().map(Line::width).max().unwrap_or(0);
    let popup = popup_area(screen, width, lines.len());
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Keys ")),
        popup,
    );
}

/// `(keys, help)` for each binding, with keys as the overlay writes them.
fn binding_rows<A>(bindings: &[Binding<A>]) -> Vec<(String, &'static str)> {
    bindings
        .iter()
        .map(|binding| {
            let labels: Vec<String> = binding.keys.iter().copied().map(key_label).collect();
            (labels.join(" "), binding.help)
        })
        .collect()
}

/// The cursor cell's full value in a popup: strings as written (newlines
/// kept), nested JSON pretty-printed, null dimmed. Under it, the cell's
/// comment threads.
fn draw_detail(frame: &mut Frame, app: &App, screen: Rect) {
    let table = app.table();
    let (row, column) = (app.cursor.row, &table.columns[app.cursor.col]);
    let value: Text = match table.cell(row, &column.name) {
        None | Some(Value::Null) => Text::styled("null", app.theme.null),
        // Tabs would be dropped by the terminal; show them as spaces.
        Some(Value::String(text)) => Text::raw(text.replace('\t', "    ")),
        Some(nested @ (Value::Array(_) | Value::Object(_))) => {
            // Pretty-printing a `Value` can't fail.
            Text::raw(serde_json::to_string_pretty(nested).unwrap_or_default())
        }
        Some(other) => Text::raw(other.to_string()),
    };
    // Name the row the way the grid shows it: by its key value, or by the same
    // 1-based number the status line uses. (Unkeyed row IDs like `#0` are
    // for agents, and would read as an off-by-one next to `row 1/1`.)
    let row_name = match &table.key {
        Some(key) => format!("{} {}", single_line(key), table.row_id(row)),
        None => format!("row {}", row + 1),
    };
    let title = format!(" {} · {row_name} ", column.display_name());

    let popup = screen.centered(Constraint::Percentage(80), Constraint::Percentage(60));
    let block = Block::bordered().title(title);
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    // Without threads the comments take two lines. With them, the value and
    // the comments share the popup in proportion to the rows each wraps to,
    // so a short thread leaves a long value room. The counts assume wrapping
    // at any character; wrapping at words can need a row more, which matters
    // only when the popup is nearly full.
    let threads = app.cell_threads();
    let comments = comment_lines(&threads);
    let width = usize::from(inner.width.max(1));
    let share = |lines: &[Line]| {
        let rows: usize = lines
            .iter()
            .map(|line| line.width().div_ceil(width).max(1))
            .sum();
        Constraint::Fill(u16::try_from(rows).unwrap_or(u16::MAX))
    };
    let comments_height = if threads.is_empty() {
        Constraint::Length(2)
    } else {
        share(&comments)
    };
    let [value_area, comments_area] =
        Layout::vertical([share(&value.lines), comments_height]).areas(inner);
    frame.render_widget(Paragraph::new(value).wrap(Wrap { trim: false }), value_area);
    frame.render_widget(
        Paragraph::new(comments).wrap(Wrap { trim: false }),
        comments_area,
    );
}

/// The popup's comments: a heading, then every message of every thread, with
/// a blank line between threads. `none yet` without any.
fn comment_lines(threads: &[&Thread]) -> Vec<Line<'static>> {
    let bold = Style::new().add_modifier(Modifier::BOLD);
    if threads.is_empty() {
        return vec![Line::styled("Comments", bold), Line::from("none yet").dim()];
    }
    let heading = Line::from(vec![
        Span::styled("Comments", bold),
        Span::raw("  ·  c reply  ·  d delete"),
    ]);
    let mut lines = vec![heading];
    for (index, thread) in threads.iter().enumerate() {
        if index > 0 {
            lines.push(Line::from(""));
        }
        lines.extend(thread.messages.iter().flat_map(message_lines));
    }
    lines
}

/// One message: the author in bold, two spaces, then the text. ratatui drops
/// `\n` inside a `Line` (it drops every control character, so comment text
/// can't send escape sequences to the terminal), so a multi-line message is
/// split here: its first line goes after the author, the rest underneath.
/// Tabs become spaces, as in the value.
fn message_lines(message: &Message) -> Vec<Line<'static>> {
    let author = match message.author {
        Author::User => "user",
        Author::Agent => "agent",
    };
    let text = message.text.replace('\t', "    ");
    let mut text_lines = text.split('\n');
    let first = Line::from(vec![
        Span::styled(
            format!("{author}  "),
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Span::raw(text_lines.next().unwrap_or_default().to_string()),
    ]);
    iter::once(first)
        .chain(text_lines.map(|rest| Line::from(rest.to_string())))
        .collect()
}

/// The cursor's row as one line per field: name, `│`, value. The cursor's
/// column is the current field, so this needs no state of its own.
fn draw_record(frame: &mut Frame, app: &App, area: Rect) {
    let table = app.table();
    let theme = &app.theme;
    let cursor = app.cursor;
    let name_width = table
        .columns
        .iter()
        .map(|column| layout::display_width(&column.display_name()))
        .max()
        .unwrap_or(0)
        .min(layout::DEFAULT_WIDTH_CAP);
    // The name takes a column span (padding and separator); the value gets
    // padding on both sides.
    let value_width = area
        .width
        .saturating_sub(layout::column_span(name_width) + 2 * layout::CELL_PADDING);
    let padding = " ".repeat(usize::from(layout::CELL_PADDING));

    // Scroll so the current field is on screen; it sits on the last line once
    // there are more fields than lines.
    let height = usize::from(area.height);
    let first = app
        .cursor_position()
        .saturating_sub(height.saturating_sub(1));
    // Fields in screen order, so they match the grid's columns.
    let lines: Vec<Line> = app
        .column_order()
        .into_iter()
        .skip(first)
        .take(height)
        .map(|col| {
            let column = &table.columns[col];
            let value = table.cell(cursor.row, &column.name);
            let current = col == cursor.col;
            let line_style = if current {
                theme.cursor_line
            } else {
                Style::new()
            };
            let mut value_style = if current {
                theme.cursor_cell
            } else {
                Style::new()
            };
            if value.is_none_or(Value::is_null) {
                value_style = value_style.patch(theme.null);
            }
            let name = layout::fit(&column.display_name(), name_width, Align::Right);
            let text = layout::fit(&cell_text(value), value_width, Align::Left);
            Line::from(vec![
                Span::styled(
                    format!("{padding}{name}{padding}"),
                    theme.header.patch(line_style),
                ),
                Span::styled(layout::SEPARATOR, line_style),
                Span::styled(format!("{padding}{text}{padding}"), value_style),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// A centered box big enough for `width` × `height` of content plus its
/// border, shrunk to fit the screen.
fn popup_area(screen: Rect, width: usize, height: usize) -> Rect {
    let border = 2;
    let width = u16::try_from(width + border).unwrap_or(u16::MAX);
    let height = u16::try_from(height + border).unwrap_or(u16::MAX);
    screen.centered(Constraint::Length(width), Constraint::Length(height))
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::comments;
    use crate::snapshot::{assert_snapshot, buffer_view};
    use crate::table::Table;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use serde_json::json;
    use std::time::Instant;

    use crate::viewer::app::Cursor;
    use crate::viewer::fit_viewport;
    use crate::viewer::theme::Theme;

    fn app(json: &str) -> App {
        App::new(Table::parse(json).unwrap())
    }

    fn prs() -> App {
        App::new(Table::from_path("examples/prs.json").unwrap())
    }

    /// Draw `app` the way the event loop does: viewport first, then the frame.
    fn render(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
        fit_viewport(app, Rect::new(0, 0, width, height));
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
    }

    /// The screen's text plus its style runs, for snapshots.
    fn render_text(app: &mut App, width: u16, height: u16) -> String {
        buffer_view(render(app, width, height).backend().buffer())
    }

    #[test]
    fn prs_wide() {
        let mut app = prs();
        app.session_id = Some("k3f9x2".to_string());
        assert_snapshot("prs_wide", &render_text(&mut app, 100, 20));
    }

    #[test]
    fn prs_narrow() {
        assert_snapshot("prs_narrow", &render_text(&mut prs(), 50, 12));
    }

    #[test]
    fn pinned_column_survives_scrolling() {
        let mut app = app(r#"{"title": "Pinned", "key": "id", "rows": [
                {"id": "a-fairly-long-identifier", "name": "first", "size": 10, "note": "kept"},
                {"id": "b-another-long-identifier", "name": "second", "size": 200, "note": "also"}
            ]}"#);
        app.first_col = 2;
        assert_snapshot("pinned_scrolled", &render_text(&mut app, 45, 7));
    }

    #[test]
    fn wide_characters_truncate_cleanly() {
        let mut app = app(
            r#"{"columns": [{"name": "text", "width": 7}, {"name": "n"}], "rows": [
                {"text": "日本語のテキスト", "n": 1},
                {"text": "🦀 crab 🦀 crab", "n": 2},
                {"text": "ascii", "n": 3}
            ]}"#,
        );
        assert_snapshot("wide_characters", &render_text(&mut app, 24, 8));
    }

    #[test]
    fn empty_table() {
        let mut app = app(
            r#"{"title": "Empty", "columns": [{"name": "id"}, {"name": "title"}], "rows": []}"#,
        );
        assert_snapshot("empty", &render_text(&mut app, 30, 6));
    }

    #[test]
    fn title_and_headers_show_newlines_like_cells() {
        let mut app = app(
            r#"{"title": "two\nlines", "columns": [{"name": "a\nb"}], "rows": [{"a\nb": "c\nd"}]}"#,
        );
        let text = render_text(&mut app, 30, 6);
        assert!(text.contains(" two↵lines "), "{text}");
        assert!(text.contains("│ a↵b │"), "{text}");
        assert!(text.contains("│ c↵d │"), "{text}");
    }

    #[test]
    fn header_is_bold_and_nulls_are_dim() {
        let mut app = app(r#"[{"a": null, "b": 1}, {"a": 2, "b": null}, {"a": 3, "b": 4}]"#);
        app.cursor = Cursor { row: 2, col: 1 };
        assert_snapshot("nulls_and_header", &render_text(&mut app, 20, 7));
    }

    #[test]
    fn cursor_bottom_right() {
        let mut app = prs();
        app.cursor = Cursor { row: 39, col: 8 };
        assert_snapshot("cursor_bottom_right", &render_text(&mut app, 70, 12));
    }

    #[test]
    fn cursor_mid_table() {
        let mut app = prs();
        app.cursor = Cursor { row: 3, col: 2 };
        assert_snapshot("cursor_mid_table", &render_text(&mut app, 70, 12));
    }

    #[test]
    fn cursor_mid_table_mono() {
        let mut app = prs();
        app.theme = Theme::mono();
        app.cursor = Cursor { row: 3, col: 2 };
        assert_snapshot("cursor_mid_table_mono", &render_text(&mut app, 70, 12));
    }

    #[test]
    fn empty_table_status() {
        let mut app = app(r#"{"columns": [{"name": "a"}], "rows": []}"#);
        let text = render_text(&mut app, 40, 6);
        assert!(text.contains(" no rows  ·  ? help"), "{text}");
    }

    #[test]
    fn help_overlay() {
        let mut app = prs();
        app.mode = Mode::Help;
        // Tall enough for every row: `popup_area` shrinks the box to the
        // screen, so a short one would cut off the end without failing.
        assert_snapshot("help", &render_text(&mut app, 60, 44));
    }

    #[test]
    fn command_line_mid_edit() {
        let mut app = prs();
        for code in [':', 'w', ' ', 'o', 'u', 't', '.', 'j', 's', 'o', 'n']
            .map(KeyCode::Char)
            .into_iter()
            .chain([KeyCode::Left, KeyCode::Left])
        {
            app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
        }
        let mut terminal = render(&mut app, 50, 8);
        assert_snapshot("command_line", &buffer_view(terminal.backend().buffer()));
        // After `:w out.js`, so at column 1 (border) + 9; on the status row.
        terminal.backend_mut().assert_cursor_position((10, 6));
    }

    #[test]
    fn status_message() {
        let mut app = prs();
        for code in [':', 'f', 'r', 'o', 'b']
            .map(KeyCode::Char)
            .into_iter()
            .chain([KeyCode::Enter])
        {
            app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
        }
        assert_snapshot("status_message", &render_text(&mut app, 70, 8));
    }

    #[test]
    fn detail_long_string() {
        let mut app = app(
            r#"{"key": "id", "rows": [{"id": 7, "note": "First line of a long note that keeps going well past the width of the popup so it has to wrap.\nSecond line.\tTabbed."}]}"#,
        );
        app.cursor = Cursor { row: 0, col: 1 };
        app.mode = Mode::Detail;
        assert_snapshot("detail_long_string", &render_text(&mut app, 50, 16));
    }

    #[test]
    fn detail_nested_and_null() {
        let mut app = app(r#"[{"meta": {"labels": ["bug", "ci"], "draft": false}, "gone": null}]"#);
        app.mode = Mode::Detail;
        // Tall enough for the whole value: the popup is 60% of the height
        // and doesn't scroll yet.
        assert_snapshot("detail_nested", &render_text(&mut app, 50, 20));
        app.cursor = Cursor { row: 0, col: 1 };
        assert_snapshot("detail_null", &render_text(&mut app, 50, 12));
    }

    #[test]
    fn record_view() {
        let mut app = prs();
        app.cursor = Cursor { row: 7, col: 5 };
        app.mode = Mode::Record;
        assert_snapshot("record_view", &render_text(&mut app, 60, 14));
    }

    #[test]
    fn record_view_with_the_key_in_the_middle() {
        let mut app = app(r#"{"key": "id", "rows": [{"name": "a", "id": 1, "size": 2}]}"#);
        app.mode = Mode::Record;
        assert_snapshot("record_view_key_in_middle", &render_text(&mut app, 30, 8));
    }

    #[test]
    fn record_view_scrolls_to_the_current_field() {
        let mut app = prs();
        app.cursor = Cursor { row: 0, col: 8 };
        app.mode = Mode::Record;
        assert_snapshot("record_view_scrolled", &render_text(&mut app, 60, 9));
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        for (width, height) in [(0, 0), (1, 1), (3, 2), (10, 3)] {
            render(&mut prs(), width, height);
            for mode in [
                Mode::Help,
                Mode::Detail,
                Mode::Record,
                Mode::ConfirmDelete(vec!["c1".to_string()]),
            ] {
                let mut app = prs();
                app.mode = mode;
                render(&mut app, width, height);
            }
            // `CommentInput`'s fields are private, so type `c` instead.
            let mut app = prs();
            app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
            assert!(matches!(app.mode, Mode::Comment(_)));
            render(&mut app, width, height);
        }
    }

    #[test]
    fn reload_highlights() {
        let mut app = prs();
        let mut table = app.table().clone();
        table.rows[1].insert("title".to_string(), json!("Bump serde to 1.0.230"));
        let mut added = table.rows[0].clone();
        added.insert("number".to_string(), json!(999));
        table.rows.insert(2, added);
        app.set_table(table, Instant::now());
        // The cursor is on row 0: the changed cell is on row 1, the new row
        // is row 2, and neither touches the cursor cell.
        assert_snapshot("reload_highlights", &render_text(&mut app, 100, 8));
    }

    #[test]
    fn reload_error_status() {
        let mut app = prs();
        app.reload_error = Some("table.json: expected value at line 3 column 1".to_string());
        assert_snapshot("reload_error_status", &render_text(&mut app, 80, 6));
    }

    #[test]
    fn a_reload_to_an_empty_table_in_the_cell_popup_does_not_panic() {
        let mut app = prs();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Detail);
        app.set_table(Table::parse("[{}]").unwrap(), Instant::now());
        render(&mut app, 60, 10);
        assert_eq!(app.mode, Mode::Grid);
    }

    fn press_keys(app: &mut App, codes: impl IntoIterator<Item = KeyCode>) {
        for code in codes {
            app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
        }
    }

    /// `prs()` with three threads: one awaiting a reply on 404's title, one
    /// the agent answered on 409's author, and an orphaned one the agent had
    /// the last word on, so only one is awaiting.
    fn prs_with_comments() -> App {
        let mut app = prs();
        let log = [
            r#"{"op":"add","id":"c1","author":"user","row_id":"404","column":"title","text":"Why is this still open?","ts":"2026-10-29T10:00:00Z"}"#,
            r#"{"op":"add","id":"c2","author":"user","row_id":"409","column":"author","text":"Who reviews this?","ts":"2026-10-29T10:01:00Z"}"#,
            r#"{"op":"add","id":"c3","author":"agent","reply_to":"c2","text":"Sam, tomorrow.","ts":"2026-10-29T10:02:00Z"}"#,
            r#"{"op":"add","id":"c4","author":"agent","row_id":"1","column":"title","text":"Row 1 is gone","ts":"2026-10-29T10:03:00Z"}"#,
        ];
        app.set_events(comments::parse_log(&log.join("\n")).events);
        app
    }

    /// Where the `▸` markers are on screen.
    fn markers(terminal: &Terminal<TestBackend>) -> Vec<(u16, u16)> {
        let buffer = terminal.backend().buffer();
        let area = buffer.area;
        (area.top()..area.bottom())
            .flat_map(|y| (area.left()..area.right()).map(move |x| (x, y)))
            .filter(|&(x, y)| buffer[(x, y)].symbol() == "▸")
            .collect()
    }

    #[test]
    fn comment_markers() {
        let mut app = prs_with_comments();
        assert_snapshot("comment_markers", &render_text(&mut app, 100, 8));
        let status = render_text(&mut app, 100, 8);
        assert!(
            status.contains("1 awaiting reply  ·  1 orphaned"),
            "{status}"
        );
    }

    #[test]
    fn comment_markers_mono() {
        let mut app = prs_with_comments();
        app.theme = Theme::mono();
        assert_snapshot("comment_markers_mono", &render_text(&mut app, 100, 8));
        // The replied marker on 409's author sits on the bold cursor line,
        // and must still read as faint.
        let terminal = render(&mut app, 100, 8);
        let replied = markers(&terminal)
            .into_iter()
            .find(|&(_, y)| y == 2)
            .unwrap();
        let modifier = terminal.backend().buffer()[replied].modifier;
        assert!(modifier.contains(Modifier::DIM), "{modifier:?}");
        assert!(!modifier.contains(Modifier::BOLD), "{modifier:?}");
    }

    #[test]
    fn a_marker_on_a_null_cell_isnt_dimmed() {
        for theme in [Theme::color(), Theme::mono()] {
            let mut app = app(r#"{"key": "id", "rows": [{"id": "a", "note": null}, {"id": "b"}]}"#);
            app.theme = theme;
            let log = r#"{"op":"add","id":"c1","author":"user","row_id":"a","column":"note","text":"why blank?","ts":"t"}"#;
            app.set_events(comments::parse_log(log).events);
            let terminal = render(&mut app, 40, 8);
            let found = markers(&terminal);
            assert_eq!(found.len(), 1, "{found:?}");
            let modifier = terminal.backend().buffer()[found[0]].modifier;
            assert!(!modifier.contains(Modifier::DIM), "{modifier:?}");
        }
    }

    #[test]
    fn comment_detail() {
        let mut app = prs_with_comments();
        let log = [
            r#"{"op":"add","id":"c1","author":"user","row_id":"404","column":"title","text":"Why is this still open?","ts":"2026-10-29T10:00:00Z"}"#,
            r#"{"op":"add","id":"c5","author":"agent","reply_to":"c1","text":"Waiting on CI.\nShould be green by noon.","ts":"2026-10-29T10:05:00Z"}"#,
        ];
        app.set_events(comments::parse_log(&log.join("\n")).events);
        app.cursor = Cursor { row: 1, col: 1 };
        press_keys(&mut app, [KeyCode::Enter]);
        assert_eq!(app.mode, Mode::Detail);
        assert_snapshot("comment_detail", &render_text(&mut app, 70, 20));
    }

    #[test]
    fn comment_input() {
        let mut app = prs();
        press_keys(&mut app, ['c', 'h', 'i'].map(KeyCode::Char));
        let mut terminal = render(&mut app, 50, 8);
        assert_snapshot("comment_input", &buffer_view(terminal.backend().buffer()));
        // After `comment: hi`: column 1 (border) + 11, on the status row.
        terminal.backend_mut().assert_cursor_position((12, 6));
    }

    #[test]
    fn a_long_comment_scrolls_to_the_end() {
        let mut app = prs();
        press_keys(&mut app, [KeyCode::Char('c')]);
        let text: String = (0..100)
            .map(|index| char::from(b'a' + index % 26))
            .collect();
        press_keys(&mut app, text.chars().map(KeyCode::Char));
        let mut terminal = render(&mut app, 50, 8);
        let status = buffer_view(terminal.backend().buffer());
        let status_row = status.lines().nth(6).unwrap();
        assert!(status_row.contains(&text[90..]), "{status_row}");
        assert!(status_row.contains("comment: "), "{status_row}");
        // The cursor is just after the last character, inside the border.
        let cursor = terminal.get_cursor_position().unwrap();
        assert_eq!(cursor.y, 6);
        assert!(cursor.x < 49, "{cursor:?}");
    }

    #[test]
    fn comment_delete_prompt() {
        let mut app = prs_with_comments();
        app.cursor = Cursor { row: 1, col: 1 };
        press_keys(&mut app, [KeyCode::Enter, KeyCode::Char('d')]);
        assert!(matches!(app.mode, Mode::ConfirmDelete(_)), "{:?}", app.mode);
        assert_snapshot("comment_delete_prompt", &render_text(&mut app, 70, 12));
    }

    #[test]
    fn a_reload_to_an_empty_table_in_the_delete_prompt_does_not_panic() {
        let mut app = prs_with_comments();
        app.cursor = Cursor { row: 1, col: 1 };
        press_keys(&mut app, [KeyCode::Enter, KeyCode::Char('d')]);
        app.set_table(Table::parse("[{}]").unwrap(), Instant::now());
        render(&mut app, 60, 10);
        assert_eq!(app.mode, Mode::Grid);
    }

    #[test]
    fn a_short_thread_leaves_the_value_room() {
        let body: Vec<String> = (1..=12).map(|n| format!("line {n}")).collect();
        let json = serde_json::json!({"key": "id", "rows": [{"id": "a", "body": body.join("\n")}]});
        let mut app = app(&json.to_string());
        let log = r#"{"op":"add","id":"c1","author":"user","row_id":"a","column":"body","text":"why?","ts":"t"}"#;
        app.set_events(comments::parse_log(log).events);
        app.cursor = Cursor { row: 0, col: 1 };
        press_keys(&mut app, [KeyCode::Enter]);
        let screen = render_text(&mut app, 80, 24);
        assert!(screen.contains("user  why?"), "{screen}");
        assert!(screen.contains("line 10"), "{screen}");
    }
}
