//! Drawing. Pure: takes `&App` and draws a frame, never changes state.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use serde_json::Value;

use super::app::App;
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
    frame.render_widget(status_line(app), areas.status);
}

fn header_line(app: &App, visible: &[VisibleColumn]) -> Line<'static> {
    let columns = &app.table().columns;
    let bold = Style::new().add_modifier(Modifier::BOLD);
    grid_line(visible, |col| {
        (columns[col].display_name(), align(columns[col].kind), bold)
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
            grid_line(visible, |col| {
                let column = &table.columns[col];
                let text = cell_text(table.cell(row, &column.name));
                (text, align(column.kind), cell_style(app, row, col))
            })
        })
        .collect()
}

/// The one place that decides how a body cell looks. The cursor, comment
/// markers, and change highlights get added here.
fn cell_style(app: &App, row: usize, col: usize) -> Style {
    let table = app.table();
    let value = table.cell(row, &table.columns[col].name);
    let mut style = Style::new();
    if (row, col) == (app.cursor.row, app.cursor.col) {
        style = style.add_modifier(Modifier::REVERSED);
    }
    if value.is_none_or(Value::is_null) {
        style = style.add_modifier(Modifier::DIM);
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
fn grid_line(
    visible: &[VisibleColumn],
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
                Span::raw(layout::SEPARATOR),
            ]
        })
        .collect();
    Line::from(spans)
}

fn divider(width: u16) -> String {
    let middle = "─".repeat(usize::from(width.saturating_sub(2)));
    format!("├{middle}┤")
}

fn status_line(app: &App) -> Line<'static> {
    let table = app.table();
    Line::from(format!(
        " rows {}  cols {}  ·  q to quit",
        table.rows.len(),
        table.columns.len()
    ))
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::snapshot::assert_snapshot;
    use crate::table::Table;
    use crate::viewer::sync_viewport;

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

    fn render_text(app: &mut App, width: u16, height: u16) -> String {
        render(app, width, height).backend().to_string()
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
        let mut app = app(r#"[{"a": null, "b": 1}]"#);
        let terminal = render(&mut app, 20, 6);
        let buffer = terminal.backend().buffer();
        // Row 0 is the top border; x 0 is the left border and x 1 is padding.
        assert!(buffer[(2, 1)].modifier.contains(Modifier::BOLD));
        assert!(buffer[(2, 2)].modifier.contains(Modifier::DIM));
        assert!(!buffer[(6, 2)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        for (width, height) in [(0, 0), (1, 1), (3, 2), (10, 3)] {
            render(&mut prs(), width, height);
        }
    }
}
