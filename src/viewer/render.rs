//! Drawing. Pure: takes `&App` and draws a frame, never changes state.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use serde_json::Value;

use super::app::{App, ColonCommand, GRID_BINDINGS, Mode, key_label};
use super::layout::{self, Align, VisibleColumn};
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

    let visible = app.visible_columns();
    frame.render_widget(header_line(app, &visible), areas.header);
    frame.render_widget(
        Paragraph::new(body_lines(app, &visible, areas.body.height)),
        areas.body,
    );
    frame.render_widget(Line::from(divider(screen.width)), areas.divider);
    draw_status(frame, app, areas.status);

    if app.mode == Mode::Help {
        draw_help(frame, screen);
    }
}

fn header_line(app: &App, visible: &[VisibleColumn]) -> Line<'static> {
    let columns = &app.table().columns;
    let header = app.theme.header;
    grid_line(visible, Style::new(), |col| {
        (
            columns[col].display_name(),
            align(columns[col].kind),
            header,
        )
    })
}

fn body_lines(app: &App, visible: &[VisibleColumn], height: u16) -> Vec<Line<'static>> {
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
            grid_line(visible, row_style, |col| {
                let column = &table.columns[col];
                let text = cell_text(table.cell(row, &column.name));
                (text, align(column.kind), cell_style(app, row, col))
            })
        })
        .collect()
}

/// The one place that decides how a body cell looks: the cursor cell is
/// brightest, then the cursor's row and column, then everything else. Comment
/// markers and change highlights get added here.
fn cell_style(app: &App, row: usize, col: usize) -> Style {
    let theme = &app.theme;
    let cursor = app.cursor;
    let mut style = if (row, col) == (cursor.row, cursor.col) {
        theme.cursor_cell
    } else if row == cursor.row || col == cursor.col {
        theme.cursor_line
    } else {
        Style::new()
    };
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

/// The only code that lays out a grid row: each visible column's text is
/// sized with `layout::fit`, padded, and followed by the separator. The header
/// and body rows both go through here; `cell` says what goes in each column.
/// Separators take `row_style`, so a highlighted row isn't striped at each `│`.
fn grid_line(
    visible: &[VisibleColumn],
    row_style: Style,
    cell: impl Fn(usize) -> (String, Align, Style),
) -> Line<'static> {
    let padding = " ".repeat(usize::from(layout::CELL_PADDING));
    let spans: Vec<Span> = visible
        .iter()
        .flat_map(|column| {
            let (text, align, style) = cell(column.index);
            let sized = layout::fit(&text, column.width, align);
            [
                Span::styled(format!("{padding}{sized}{padding}"), style),
                Span::styled(layout::SEPARATOR, row_style),
            ]
        })
        .collect();
    Line::from(spans)
}

fn divider(width: u16) -> String {
    let middle = "─".repeat(usize::from(width.saturating_sub(2)));
    format!("├{middle}┤")
}

/// Key hints after the position, unless a message is showing. The full list
/// is in the help overlay.
const STATUS_HINTS: &str = "? help  : command  q quit";

/// The status line, or the `:` line while a command is being typed.
fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    if let Mode::Command(line) = &app.mode {
        frame.render_widget(Line::from(format!(":{}", line.text)), area);
        // Put the terminal's cursor where typing goes: after the `:` and the
        // text before the cursor, measured in cells, not bytes.
        let offset = 1 + layout::display_width(line.before_cursor());
        let x = area
            .x
            .saturating_add(offset)
            .min(area.right().saturating_sub(1));
        frame.set_cursor_position((x, area.y));
        return;
    }
    frame.render_widget(status_line(app), area);
}

fn status_line(app: &App) -> Line<'static> {
    let table = app.table();
    let position = if table.rows.is_empty() {
        "no rows".to_string()
    } else {
        format!(
            "row {}/{}  col {}/{}",
            app.cursor.row + 1,
            table.rows.len(),
            app.cursor.col + 1,
            table.columns.len()
        )
    };
    let after = app
        .message
        .as_ref()
        .map_or(STATUS_HINTS, |message| message.text.as_str());
    Line::from(format!(" {position}  ·  {after}"))
}

/// A centered box listing every grid key and `:` command. It's built from
/// `GRID_BINDINGS` and `ColonCommand::HELP`, so it always matches what works.
fn draw_help(frame: &mut Frame, screen: Rect) {
    let keys: Vec<(String, &str)> = GRID_BINDINGS
        .iter()
        .map(|binding| {
            let labels: Vec<String> = binding.keys.iter().copied().map(key_label).collect();
            (labels.join(" "), binding.help)
        })
        .collect();
    let commands: Vec<(String, &str)> = ColonCommand::HELP
        .iter()
        .map(|&(name, help)| (name.to_string(), help))
        .collect();
    let label_width = keys
        .iter()
        .chain(&commands)
        .map(|(label, _)| layout::display_width(label))
        .max()
        .unwrap_or(0);
    let help_line = |(label, help): &(String, &str)| {
        Line::from(vec![
            Span::styled(
                format!(" {} ", layout::fit(label, label_width, Align::Right)),
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {help} ")),
        ])
    };

    let mut lines: Vec<Line> = keys.iter().map(help_line).collect();
    lines.push(Line::from(""));
    lines.extend(commands.iter().map(help_line));
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
    use crate::snapshot::{assert_snapshot, buffer_view};
    use crate::table::Table;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::viewer::app::Cursor;
    use crate::viewer::sync_viewport;
    use crate::viewer::theme::Theme;

    fn app(json: &str) -> App {
        App::new(Table::parse(json).unwrap())
    }

    fn prs() -> App {
        App::new(Table::from_path("examples/prs.json").unwrap())
    }

    /// Draw `app` the way the event loop does: viewport first, then the frame.
    fn render(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
        sync_viewport(app, Rect::new(0, 0, width, height));
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
        assert_snapshot("help", &render_text(&mut app, 60, 26));
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
    fn tiny_terminal_does_not_panic() {
        for (width, height) in [(0, 0), (1, 1), (3, 2), (10, 3)] {
            render(&mut prs(), width, height);
            let mut help = prs();
            help.mode = Mode::Help;
            render(&mut help, width, height);
        }
    }
}
