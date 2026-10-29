//! Drawing. Pure: takes `&App` and draws a frame, never changes state.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use serde_json::Value;

use super::app::App;
use super::layout::{self, Align, VisibleColumn};
use crate::table::{ColumnType, cell_text};

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let table = app.table();

    let mut block = Block::bordered();
    if let Some(title) = &table.title {
        block = block.title_top(Line::from(format!(" {title} ")).left_aligned());
    }
    if let Some(session_id) = &app.session_id {
        block = block.title_top(Line::from(format!(" {session_id} ")).right_aligned());
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [header_area, body_area, divider_area, status_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    let visible = layout::visible_columns(
        app.widths(),
        table.key_column_index(),
        app.first_col,
        inner.width,
    );
    frame.render_widget(header_line(app, &visible), header_area);
    frame.render_widget(
        Paragraph::new(body_lines(app, &visible, body_area.height)),
        body_area,
    );
    // The divider runs across the side borders too, joining them with ├ and ┤.
    let divider_row = Rect {
        x: area.x,
        width: area.width,
        ..divider_area
    };
    frame.render_widget(Line::from(divider(area.width)), divider_row);
    frame.render_widget(status_line(app), status_area);
}

fn header_line(app: &App, visible: &[VisibleColumn]) -> Line<'static> {
    let columns = &app.table().columns;
    let bold = Style::new().add_modifier(Modifier::BOLD);
    grid_line(visible, |col| {
        (columns[col].name.clone(), align(columns[col].kind), bold)
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

    fn app(json: &str) -> App {
        App::new(Table::parse(json).unwrap())
    }

    fn prs() -> App {
        App::new(Table::from_path("examples/prs.json").unwrap())
    }

    fn render(app: &App, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
    }

    fn render_text(app: &App, width: u16, height: u16) -> String {
        render(app, width, height).backend().to_string()
    }

    #[test]
    fn prs_wide() {
        let mut app = prs();
        app.session_id = Some("k3f9x2".to_string());
        assert_snapshot("prs_wide", &render_text(&app, 100, 20));
    }

    #[test]
    fn prs_narrow() {
        assert_snapshot("prs_narrow", &render_text(&prs(), 50, 12));
    }

    #[test]
    fn pinned_column_survives_scrolling() {
        let mut app = app(r#"{"title": "Pinned", "key": "id", "rows": [
                {"id": "a-fairly-long-identifier", "name": "first", "size": 10, "note": "kept"},
                {"id": "b-another-long-identifier", "name": "second", "size": 200, "note": "also"}
            ]}"#);
        app.first_col = 2;
        assert_snapshot("pinned_scrolled", &render_text(&app, 45, 7));
    }

    #[test]
    fn wide_characters_truncate_cleanly() {
        let app = app(
            r#"{"columns": [{"name": "text", "width": 7}, {"name": "n"}], "rows": [
                {"text": "日本語のテキスト", "n": 1},
                {"text": "🦀 crab 🦀 crab", "n": 2},
                {"text": "ascii", "n": 3}
            ]}"#,
        );
        assert_snapshot("wide_characters", &render_text(&app, 24, 8));
    }

    #[test]
    fn empty_table() {
        let app = app(
            r#"{"title": "Empty", "columns": [{"name": "id"}, {"name": "title"}], "rows": []}"#,
        );
        assert_snapshot("empty", &render_text(&app, 30, 6));
    }

    #[test]
    fn header_is_bold_and_nulls_are_dim() {
        let app = app(r#"[{"a": null, "b": 1}]"#);
        let terminal = render(&app, 20, 6);
        let buffer = terminal.backend().buffer();
        // Row 0 is the top border; x 0 is the left border and x 1 is padding.
        assert!(buffer[(2, 1)].modifier.contains(Modifier::BOLD));
        assert!(buffer[(2, 2)].modifier.contains(Modifier::DIM));
        assert!(!buffer[(6, 2)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        for (width, height) in [(0, 0), (1, 1), (3, 2), (10, 3)] {
            render(&prs(), width, height);
        }
    }
}
