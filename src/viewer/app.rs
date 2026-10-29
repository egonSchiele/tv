//! Viewer state. All mutation happens here; render only reads it.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::layout::{self, VisibleColumn};
use super::theme::Theme;
use crate::table::Table;

/// Indices into `table.rows` and `table.columns`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
}

/// The grid's size in cells, as of the last frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Viewport {
    pub width: u16,
    pub height: u16,
}

/// A key as the bindings tables name it.
#[derive(Debug, Clone, Copy)]
pub struct Key {
    pub code: KeyCode,
    pub ctrl: bool,
}

impl Key {
    const fn plain(code: KeyCode) -> Self {
        Key { code, ctrl: false }
    }

    const fn char(character: char) -> Self {
        Key::plain(KeyCode::Char(character))
    }

    const fn ctrl(character: char) -> Self {
        Key {
            code: KeyCode::Char(character),
            ctrl: true,
        }
    }

    // Shift is ignored: terminals send `G` and `$` with or without it.
    fn matches(self, event: &KeyEvent) -> bool {
        self.code == event.code && self.ctrl == event.modifiers.contains(KeyModifiers::CONTROL)
    }
}

/// One row of a bindings table: these keys do this action. The help overlay
/// lists `help` next to the keys, so the overlay can't drift from the keys.
pub struct Binding<A> {
    pub keys: &'static [Key],
    pub action: A,
    pub help: &'static str,
}

/// How a key is written in the help overlay: `j`, `↓`, `PgDn`, `Ctrl-f`.
pub fn key_label(key: Key) -> String {
    let name = match key.code {
        KeyCode::Char(character) => character.to_string(),
        KeyCode::Up => "↑".to_string(),
        KeyCode::Down => "↓".to_string(),
        KeyCode::Left => "←".to_string(),
        KeyCode::Right => "→".to_string(),
        KeyCode::PageUp => "PgUp".to_string(),
        KeyCode::PageDown => "PgDn".to_string(),
        other => format!("{other:?}"),
    };
    if key.ctrl {
        format!("Ctrl-{name}")
    } else {
        name
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Down,
    Up,
    Left,
    Right,
    PageDown,
    PageUp,
    FirstRow,
    LastRow,
    FirstCol,
    LastCol,
    Help,
    Quit,
}

/// What the keyboard is driving right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Grid,
    Help,
}

/// Every key the grid responds to. `handle_key` looks keys up here; to add a
/// key, add a row (and an `Action` variant if it does something new).
pub const GRID_BINDINGS: &[Binding<Action>] = &[
    Binding {
        keys: &[Key::plain(KeyCode::Down), Key::char('j')],
        action: Action::Down,
        help: "move down",
    },
    Binding {
        keys: &[Key::plain(KeyCode::Up), Key::char('k')],
        action: Action::Up,
        help: "move up",
    },
    Binding {
        keys: &[Key::plain(KeyCode::Right), Key::char('l')],
        action: Action::Right,
        help: "move right",
    },
    Binding {
        keys: &[Key::plain(KeyCode::Left), Key::char('h')],
        action: Action::Left,
        help: "move left",
    },
    Binding {
        keys: &[Key::plain(KeyCode::PageDown), Key::ctrl('f')],
        action: Action::PageDown,
        help: "page down",
    },
    Binding {
        keys: &[Key::plain(KeyCode::PageUp), Key::ctrl('b')],
        action: Action::PageUp,
        help: "page up",
    },
    Binding {
        keys: &[Key::char('g')],
        action: Action::FirstRow,
        help: "first row",
    },
    Binding {
        keys: &[Key::char('G')],
        action: Action::LastRow,
        help: "last row",
    },
    Binding {
        keys: &[Key::char('0')],
        action: Action::FirstCol,
        help: "first column",
    },
    Binding {
        keys: &[Key::char('$')],
        action: Action::LastCol,
        help: "last column",
    },
    Binding {
        keys: &[Key::char('?')],
        action: Action::Help,
        help: "show this help",
    },
    Binding {
        keys: &[Key::char('q'), Key::ctrl('c')],
        action: Action::Quit,
        help: "quit",
    },
];

fn lookup<A: Copy>(bindings: &[Binding<A>], event: &KeyEvent) -> Option<A> {
    bindings
        .iter()
        .find(|binding| binding.keys.iter().any(|key| key.matches(event)))
        .map(|binding| binding.action)
}

pub struct App {
    // Private because `widths` is derived from `table`. When the table can
    // change (live reload), add a `set_table` that recomputes `widths`, so
    // nothing can update one without the other.
    table: Table,
    widths: Vec<u16>,
    pub session_id: Option<String>, // None until sessions exist
    pub cursor: Cursor,
    pub first_row: usize, // vertical scroll offset
    pub first_col: usize, // index of the first non-pinned column shown
    viewport: Viewport,   // set only through set_viewport
    pub theme: Theme,
    pub mode: Mode,
    pub should_quit: bool,
}

impl App {
    pub fn new(table: Table) -> Self {
        let widths = layout::column_widths(&table);
        App {
            table,
            widths,
            session_id: None,
            cursor: Cursor::default(),
            first_row: 0,
            first_col: 0,
            viewport: Viewport::default(),
            theme: Theme::color(),
            mode: Mode::Grid,
            should_quit: false,
        }
    }

    pub fn table(&self) -> &Table {
        &self.table
    }

    /// Record the grid's size and scroll so the cursor stays visible. The
    /// event loop calls this before every draw, so resizes are handled here.
    pub fn set_viewport(&mut self, width: u16, height: u16) {
        self.viewport = Viewport { width, height };
        self.scroll_into_view();
    }

    /// The grid's columns for the current scroll and viewport. Render draws
    /// these and `scroll_into_view` checks them, so both agree on what's shown.
    pub fn visible_columns(&self) -> Vec<VisibleColumn> {
        layout::visible_columns(
            &self.widths,
            self.table.key_column_index(),
            self.first_col,
            self.viewport.width,
        )
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        // Some terminals also report key releases; act on presses and repeats.
        if key.kind == KeyEventKind::Release {
            return;
        }
        // Ctrl-c quits from every mode; raw mode turned off the terminal's own.
        if Key::ctrl('c').matches(&key) {
            self.should_quit = true;
            return;
        }
        match self.mode {
            Mode::Grid => {
                if let Some(action) = lookup(GRID_BINDINGS, &key) {
                    self.perform(action);
                }
            }
            Mode::Help => self.mode = Mode::Grid, // any key closes it
        }
    }

    fn perform(&mut self, action: Action) {
        let Cursor { row, col } = self.cursor;
        let page = self.page_size();
        match action {
            Action::Down => self.move_to(row + 1, col),
            Action::Up => self.move_to(row.saturating_sub(1), col),
            Action::Right => self.move_to(row, col + 1),
            Action::Left => self.move_to(row, col.saturating_sub(1)),
            // Like vim's Ctrl-f and Ctrl-b, the view moves by a whole page too.
            Action::PageDown => {
                self.first_row = (self.first_row + page).min(self.max_first_row());
                self.move_to(row + page, col);
            }
            Action::PageUp => {
                self.first_row = self.first_row.saturating_sub(page);
                self.move_to(row.saturating_sub(page), col);
            }
            Action::FirstRow => self.move_to(0, col),
            Action::LastRow => self.move_to(usize::MAX, col),
            Action::FirstCol => self.move_to(row, 0),
            Action::LastCol => self.move_to(row, usize::MAX),
            Action::Help => self.mode = Mode::Help,
            Action::Quit => self.should_quit = true,
        }
    }

    /// Put the cursor at (`row`, `col`), clamped to the table, and scroll to it.
    fn move_to(&mut self, row: usize, col: usize) {
        self.cursor = Cursor {
            row: row.min(self.table.rows.len().saturating_sub(1)),
            col: col.min(self.table.columns.len().saturating_sub(1)),
        };
        self.scroll_into_view();
    }

    fn scroll_into_view(&mut self) {
        let height = self.page_size();
        let Cursor { row, col } = self.cursor;
        if row < self.first_row {
            self.first_row = row;
        } else if row >= self.first_row + height {
            self.first_row = row + 1 - height;
        }

        if Some(col) == self.table.key_column_index() {
            return; // the pinned column is always on screen
        }
        if col < self.first_col {
            self.first_col = col;
        }
        // Ends by `first_col == col` at the latest: `visible_columns` always
        // includes the first non-pinned column.
        while self.first_col < col && !self.visible_columns().iter().any(|c| c.index == col) {
            self.first_col += 1;
        }
    }

    /// Rows in a page: the grid's height, but at least 1 before the first frame.
    fn page_size(&self) -> usize {
        usize::from(self.viewport.height.max(1))
    }

    fn max_first_row(&self) -> usize {
        self.table.rows.len().saturating_sub(self.page_size())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 50 rows by 10 columns; every cell is like `r12c3`, so columns are 5 wide.
    fn grid_app(key: Option<&str>) -> App {
        let rows: Vec<String> = (0..50)
            .map(|row| {
                let cells: Vec<String> = (0..10)
                    .map(|col| format!(r#""c{col}": "r{row}c{col}""#))
                    .collect();
                format!("{{{}}}", cells.join(", "))
            })
            .collect();
        let key = key
            .map(|key| format!(r#""key": "{key}", "#))
            .unwrap_or_default();
        let json = format!(r#"{{{key}"rows": [{}]}}"#, rows.join(", "));
        let mut app = App::new(Table::parse(&json).unwrap());
        // 60 cells fit seven 5-wide columns (8 cells each with padding and separator).
        app.set_viewport(60, 10);
        app
    }

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        app.handle_key(KeyEvent::new(code, modifiers));
    }

    fn press_char(app: &mut App, character: char) {
        press(app, KeyCode::Char(character), KeyModifiers::NONE);
    }

    fn on_screen(app: &App, col: usize) -> bool {
        app.visible_columns()
            .iter()
            .any(|column| column.index == col)
    }

    #[test]
    fn q_and_ctrl_c_quit() {
        let mut app = grid_app(None);
        press_char(&mut app, 'q');
        assert!(app.should_quit);

        let mut app = grid_app(None);
        press(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.should_quit);
    }

    #[test]
    fn unbound_keys_and_releases_do_nothing() {
        let mut app = grid_app(None);
        press_char(&mut app, 'c');
        press_char(&mut app, 'x');
        let mut release = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        app.handle_key(release);
        assert!(!app.should_quit);
        assert_eq!(app.cursor, Cursor::default());
    }

    #[test]
    fn arrows_and_vim_keys_move() {
        let mut app = grid_app(None);
        press_char(&mut app, 'j');
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        press_char(&mut app, 'l');
        assert_eq!(app.cursor, Cursor { row: 2, col: 1 });
        press_char(&mut app, 'k');
        press(&mut app, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(app.cursor, Cursor { row: 1, col: 0 });
    }

    #[test]
    fn page_down_moves_cursor_and_view_by_a_page() {
        let mut app = grid_app(None);
        press(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        assert_eq!((app.cursor.row, app.first_row), (10, 10));
        press(&mut app, KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert_eq!((app.cursor.row, app.first_row), (20, 20));
        press(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
        assert_eq!((app.cursor.row, app.first_row), (10, 10));
    }

    #[test]
    fn page_down_stops_at_the_end() {
        let mut app = grid_app(None);
        for _ in 0..10 {
            press(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        }
        assert_eq!((app.cursor.row, app.first_row), (49, 40));
    }

    #[test]
    fn g_and_shift_g_jump_to_first_and_last_row() {
        let mut app = grid_app(None);
        press_char(&mut app, 'G');
        assert_eq!((app.cursor.row, app.first_row), (49, 40));
        press_char(&mut app, 'g');
        assert_eq!((app.cursor.row, app.first_row), (0, 0));
        // Some terminals send Shift along with the capital letter.
        press(&mut app, KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert_eq!(app.cursor.row, 49);
    }

    #[test]
    fn dollar_scrolls_the_last_column_into_view() {
        let mut app = grid_app(None);
        assert!(!on_screen(&app, 9));
        press_char(&mut app, '$');
        assert_eq!(app.cursor.col, 9);
        assert!(app.first_col > 0);
        assert!(on_screen(&app, 9));
        press_char(&mut app, '0');
        assert_eq!((app.cursor.col, app.first_col), (0, 0));
    }

    #[test]
    fn moving_left_past_the_view_scrolls_back() {
        let mut app = grid_app(None);
        press_char(&mut app, '$');
        let first_col = app.first_col;
        for _ in 0..=(9 - first_col) {
            press_char(&mut app, 'h');
        }
        assert_eq!(app.cursor.col, first_col - 1);
        assert_eq!(app.first_col, first_col - 1);
    }

    #[test]
    fn edges_clamp() {
        let mut app = grid_app(None);
        press_char(&mut app, 'h');
        press_char(&mut app, 'k');
        assert_eq!(app.cursor, Cursor::default());
        press_char(&mut app, 'G');
        press_char(&mut app, '$');
        press_char(&mut app, 'j');
        press_char(&mut app, 'l');
        assert_eq!(app.cursor, Cursor { row: 49, col: 9 });
    }

    #[test]
    fn the_pinned_column_never_scrolls() {
        let mut app = grid_app(Some("c0"));
        press_char(&mut app, '$');
        let first_col = app.first_col;
        assert!(first_col > 0);
        press_char(&mut app, '0');
        assert_eq!(app.cursor.col, 0);
        assert_eq!(app.first_col, first_col);
        assert!(on_screen(&app, 0));
    }

    #[test]
    fn shrinking_the_viewport_keeps_the_cursor_visible() {
        let mut app = grid_app(None);
        press_char(&mut app, 'G');
        press_char(&mut app, '$');
        app.set_viewport(20, 3);
        assert_eq!(app.first_row, 47);
        assert!(on_screen(&app, 9));
    }

    #[test]
    fn question_mark_opens_help_and_any_key_closes_it() {
        let mut app = grid_app(None);
        press_char(&mut app, '?');
        assert_eq!(app.mode, Mode::Help);
        // The key that closes help does nothing else.
        press_char(&mut app, 'j');
        assert_eq!(app.mode, Mode::Grid);
        assert_eq!(app.cursor, Cursor::default());
    }

    #[test]
    fn ctrl_c_quits_from_help() {
        let mut app = grid_app(None);
        press_char(&mut app, '?');
        press(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.should_quit);
    }

    #[test]
    fn key_labels() {
        assert_eq!(key_label(Key::char('j')), "j");
        assert_eq!(key_label(Key::plain(KeyCode::Down)), "↓");
        assert_eq!(key_label(Key::plain(KeyCode::PageDown)), "PgDn");
        assert_eq!(key_label(Key::ctrl('f')), "Ctrl-f");
    }

    #[test]
    fn every_grid_binding_has_help_text() {
        for binding in GRID_BINDINGS {
            assert_ne!(binding.help, "");
            assert_ne!(binding.keys.len(), 0);
        }
    }

    #[test]
    fn empty_tables_ignore_moves() {
        let empty_rows = Table::parse(r#"{"columns": [{"name": "a"}], "rows": []}"#).unwrap();
        let no_columns = Table::parse(r"[{}, {}]").unwrap();
        for table in [empty_rows, no_columns] {
            let mut app = App::new(table);
            app.set_viewport(40, 10);
            for character in ['j', 'l', 'G', '$', 'g', '0'] {
                press_char(&mut app, character);
            }
            press(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
            assert_eq!(app.cursor.col, 0);
            assert_eq!(app.first_col, 0);
        }
    }
}
