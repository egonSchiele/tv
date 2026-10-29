//! Viewer state. All mutation happens here; render only reads it.

use std::convert::Infallible;
use std::str::FromStr;
use std::time::{Duration, Instant};

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
    CommandLine,
    Detail,
    Record,
    Quit,
}

/// Keys in the record view, which reuses the grid cursor: the cursor's column
/// is the current field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordAction {
    NextField,
    PreviousField,
    NextRow,
    PreviousRow,
    Back,
}

pub const RECORD_BINDINGS: &[Binding<RecordAction>] = &[
    Binding {
        keys: &[Key::plain(KeyCode::Down), Key::char('j')],
        action: RecordAction::NextField,
        help: "next field",
    },
    Binding {
        keys: &[Key::plain(KeyCode::Up), Key::char('k')],
        action: RecordAction::PreviousField,
        help: "previous field",
    },
    Binding {
        keys: &[Key::char('n')],
        action: RecordAction::NextRow,
        help: "next row",
    },
    Binding {
        keys: &[Key::char('p')],
        action: RecordAction::PreviousRow,
        help: "previous row",
    },
    Binding {
        keys: &[Key::plain(KeyCode::Esc), Key::char('r'), Key::char('q')],
        action: RecordAction::Back,
        help: "back to the grid",
    },
];

/// What the keyboard is driving right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Grid,
    Help,
    Command(CommandLine),
    /// A popup with the cursor cell's full value.
    Detail,
    /// The cursor's row as a list of fields.
    Record,
}

/// The `:` line being typed. `cursor` counts characters, not bytes, so typing
/// `é` can't split a UTF-8 sequence; it becomes a byte offset only at the edit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandLine {
    pub text: String,
    pub cursor: usize,
}

impl CommandLine {
    /// The text left of the cursor, for placing the terminal cursor.
    pub fn before_cursor(&self) -> &str {
        &self.text[..self.byte_index(self.cursor)]
    }

    fn byte_index(&self, chars: usize) -> usize {
        self.text
            .char_indices()
            .nth(chars)
            .map_or(self.text.len(), |(index, _)| index)
    }

    fn insert(&mut self, character: char) {
        let index = self.byte_index(self.cursor);
        self.text.insert(index, character);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        let index = self.byte_index(self.cursor);
        self.text.remove(index);
    }

    fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.text.chars().count());
    }

    fn end(&mut self) {
        self.cursor = self.text.chars().count();
    }
}

/// A command typed after `:`. Named so it isn't confused with clap's
/// `Command` in `main.rs`. To add one, add a variant, a `from_str` arm, a
/// `HELP` row, and a `run_command` arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColonCommand {
    Help,
    Quit,
    Write(Option<String>),
    Unknown(String),
}

impl FromStr for ColonCommand {
    // Parsing never fails: anything unrecognised is `Unknown`.
    type Err = Infallible;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        let (name, argument) = match text.split_once(char::is_whitespace) {
            Some((name, argument)) => (name, argument.trim()),
            None => (text, ""),
        };
        Ok(match (name, argument) {
            ("help" | "h", "") => ColonCommand::Help,
            ("q" | "quit", "") => ColonCommand::Quit,
            ("w" | "write", "") => ColonCommand::Write(None),
            ("w" | "write", path) => ColonCommand::Write(Some(path.to_string())),
            _ => ColonCommand::Unknown(text.to_string()),
        })
    }
}

impl ColonCommand {
    /// Listed in the help overlay. Kept next to `from_str` so they're edited together.
    pub const HELP: &'static [(&'static str, &'static str)] = &[
        (":help", "show this help"),
        (":q", "quit"),
        (":w [path]", "save (not implemented yet)"),
    ];
}

/// How long a status-line message stays up.
const MESSAGE_DURATION: Duration = Duration::from_secs(3);

pub struct Message {
    pub text: String,
    shown_at: Instant,
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
        keys: &[Key::plain(KeyCode::Enter)],
        action: Action::Detail,
        help: "cell detail",
    },
    Binding {
        keys: &[Key::char('r')],
        action: Action::Record,
        help: "record view",
    },
    Binding {
        keys: &[Key::char(':')],
        action: Action::CommandLine,
        help: "command line",
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
    pub message: Option<Message>,
    pub should_quit: bool,
}

impl App {
    pub fn new(table: Table) -> Self {
        let widths = layout::column_widths(&table);
        // Start on the leftmost column on screen, which is the key column if
        // there is one.
        let cursor = Cursor {
            row: 0,
            col: table.key_column_index().unwrap_or(0),
        };
        App {
            table,
            widths,
            session_id: None,
            cursor,
            first_row: 0,
            first_col: 0,
            viewport: Viewport::default(),
            theme: Theme::color(),
            mode: Mode::Grid,
            message: None,
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

    /// Column indices in screen order: the pinned key column, then the rest.
    pub fn column_order(&self) -> Vec<usize> {
        layout::screen_order(self.table.columns.len(), self.table.key_column_index())
    }

    /// Where the cursor's column is on screen, counting from 0 at the left.
    pub fn cursor_position(&self) -> usize {
        self.column_order()
            .iter()
            .position(|&col| col == self.cursor.col)
            .unwrap_or(0)
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
        match &mut self.mode {
            Mode::Grid => {
                if let Some(action) = lookup(GRID_BINDINGS, &key) {
                    self.perform(action);
                }
            }
            Mode::Help => self.mode = Mode::Grid, // any key closes it
            Mode::Command(line) => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    KeyCode::Esc => self.mode = Mode::Grid,
                    KeyCode::Enter => {
                        let text = line.text.trim().to_string();
                        self.mode = Mode::Grid;
                        if !text.is_empty() {
                            let Ok(command) = text.parse();
                            self.run_command(command);
                        }
                    }
                    KeyCode::Backspace => line.backspace(),
                    KeyCode::Left => line.left(),
                    KeyCode::Right => line.right(),
                    KeyCode::Home => line.cursor = 0,
                    KeyCode::End => line.end(),
                    KeyCode::Char('u') if ctrl => *line = CommandLine::default(),
                    KeyCode::Char(character) if !ctrl => line.insert(character),
                    _ => {}
                }
            }
            Mode::Detail => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                    self.mode = Mode::Grid;
                }
            }
            Mode::Record => {
                if let Some(action) = lookup(RECORD_BINDINGS, &key) {
                    self.perform_in_record(action);
                }
            }
        }
    }

    /// Moves go through `move_to`, so the grid is already scrolled to the
    /// cursor when the record view closes.
    fn perform_in_record(&mut self, action: RecordAction) {
        let Cursor { row, col } = self.cursor;
        let position = self.cursor_position();
        match action {
            // Fields go in screen order, so they match the grid's columns.
            RecordAction::NextField => self.move_to_position(row, position + 1),
            RecordAction::PreviousField => {
                self.move_to_position(row, position.saturating_sub(1));
            }
            RecordAction::NextRow => self.move_to(row + 1, col),
            RecordAction::PreviousRow => self.move_to(row.saturating_sub(1), col),
            RecordAction::Back => self.mode = Mode::Grid,
        }
    }

    /// Open a view of the cursor's cell or row, if there is one.
    fn open_cell_view(&mut self, mode: Mode) {
        if self.table.rows.is_empty() {
            self.set_message("no rows".to_string());
        } else if self.table.columns.is_empty() {
            self.set_message("no columns".to_string());
        } else {
            self.mode = mode;
        }
    }

    /// Clear the status-line message once it's `MESSAGE_DURATION` old. Takes
    /// the time as an argument so tests don't have to sleep.
    pub fn tick(&mut self, now: Instant) {
        let expired = self
            .message
            .as_ref()
            .is_some_and(|message| now.duration_since(message.shown_at) >= MESSAGE_DURATION);
        if expired {
            self.message = None;
        }
    }

    fn set_message(&mut self, text: String) {
        self.message = Some(Message {
            text,
            shown_at: Instant::now(),
        });
    }

    fn run_command(&mut self, command: ColonCommand) {
        match command {
            ColonCommand::Help => self.mode = Mode::Help,
            ColonCommand::Quit => self.should_quit = true,
            // ADI-218 implements saving.
            ColonCommand::Write(path) => {
                let target = path.map(|path| format!(" (would write {path})"));
                self.set_message(format!(
                    "save isn't implemented yet{}",
                    target.unwrap_or_default()
                ));
            }
            ColonCommand::Unknown(text) => self.set_message(format!("unknown command: {text}")),
        }
    }

    fn perform(&mut self, action: Action) {
        let Cursor { row, col } = self.cursor;
        let position = self.cursor_position();
        let page = self.page_size();
        match action {
            Action::Down => self.move_to(row + 1, col),
            Action::Up => self.move_to(row.saturating_sub(1), col),
            // Left and right go by screen position, since the pinned key
            // column is drawn first whatever its index in the table.
            Action::Right => self.move_to_position(row, position + 1),
            Action::Left => self.move_to_position(row, position.saturating_sub(1)),
            // Like vim's Ctrl-f and Ctrl-b, the view moves by a whole page too.
            // `scroll_into_view` clamps it at the end.
            Action::PageDown => {
                self.first_row += page;
                self.move_to(row + page, col);
            }
            Action::PageUp => {
                self.first_row = self.first_row.saturating_sub(page);
                self.move_to(row.saturating_sub(page), col);
            }
            Action::FirstRow => self.move_to(0, col),
            Action::LastRow => self.move_to(usize::MAX, col),
            Action::FirstCol => self.move_to_position(row, 0),
            Action::LastCol => self.move_to_position(row, usize::MAX),
            Action::Help => self.mode = Mode::Help,
            Action::CommandLine => self.mode = Mode::Command(CommandLine::default()),
            Action::Detail => self.open_cell_view(Mode::Detail),
            Action::Record => self.open_cell_view(Mode::Record),
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

    /// Like `move_to`, but `position` counts columns on screen from the left
    /// (clamped), not table indices.
    fn move_to_position(&mut self, row: usize, position: usize) {
        let order = self.column_order();
        let last = order.len().saturating_sub(1);
        let col = order.get(position.min(last)).copied().unwrap_or(0);
        self.move_to(row, col);
    }

    fn scroll_into_view(&mut self) {
        let height = self.page_size();
        let Cursor { row, col } = self.cursor;
        if row < self.first_row {
            self.first_row = row;
        } else if row >= self.first_row + height {
            self.first_row = row + 1 - height;
        }
        // Don't leave empty space below the last row, e.g. after the
        // terminal gets taller.
        self.first_row = self.first_row.min(self.max_first_row());

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

    /// The key is the middle column, so it's drawn first: `id | name | size`.
    fn key_in_the_middle() -> App {
        let json = r#"{"key": "id", "rows": [{"name": "a", "id": 1, "size": 2}]}"#;
        let mut app = App::new(Table::parse(json).unwrap());
        app.set_viewport(60, 10);
        app
    }

    #[test]
    fn columns_move_in_screen_order_when_the_key_is_not_first() {
        let mut app = key_in_the_middle();
        assert_eq!((app.cursor.col, app.cursor_position()), (1, 0)); // starts on `id`
        press_char(&mut app, 'h');
        assert_eq!(app.cursor.col, 1);
        press_char(&mut app, 'l');
        assert_eq!(app.cursor.col, 0); // `name`, to the right of `id`
        press_char(&mut app, 'l');
        assert_eq!(app.cursor.col, 2); // `size`
        press_char(&mut app, 'l');
        assert_eq!(app.cursor.col, 2);
        press_char(&mut app, '0');
        assert_eq!(app.cursor.col, 1);
        press_char(&mut app, '$');
        assert_eq!((app.cursor.col, app.cursor_position()), (2, 2));
    }

    #[test]
    fn record_fields_go_in_screen_order() {
        let mut app = key_in_the_middle();
        press_char(&mut app, 'r');
        press_char(&mut app, 'j');
        assert_eq!(app.cursor.col, 0);
        press_char(&mut app, 'j');
        assert_eq!(app.cursor.col, 2);
        press_char(&mut app, 'k');
        press_char(&mut app, 'k');
        assert_eq!(app.cursor.col, 1);
    }

    #[test]
    fn growing_the_viewport_fills_it() {
        let mut app = grid_app(None);
        press_char(&mut app, 'G');
        assert_eq!(app.first_row, 40);
        app.set_viewport(60, 20);
        assert_eq!(app.first_row, 30);
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

    fn type_text(app: &mut App, text: &str) {
        for character in text.chars() {
            press_char(app, character);
        }
    }

    fn message(app: &App) -> Option<&str> {
        app.message.as_ref().map(|message| message.text.as_str())
    }

    #[test]
    fn colon_commands_parse() {
        let cases = [
            ("help", ColonCommand::Help),
            ("h", ColonCommand::Help),
            (" q ", ColonCommand::Quit),
            ("quit", ColonCommand::Quit),
            ("w", ColonCommand::Write(None)),
            (
                "w  out file.json ",
                ColonCommand::Write(Some("out file.json".to_string())),
            ),
            (
                "write a.json",
                ColonCommand::Write(Some("a.json".to_string())),
            ),
            ("q now", ColonCommand::Unknown("q now".to_string())),
            (
                "frobnicate",
                ColonCommand::Unknown("frobnicate".to_string()),
            ),
        ];
        for (text, expected) in cases {
            let Ok(command) = text.parse::<ColonCommand>();
            assert_eq!(command, expected, "{text:?}");
        }
    }

    #[test]
    fn colon_q_quits() {
        let mut app = grid_app(None);
        type_text(&mut app, ":q");
        assert!(!app.should_quit);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.should_quit);
    }

    #[test]
    fn esc_cancels_the_command_line() {
        let mut app = grid_app(None);
        type_text(&mut app, ":qj");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.mode, Mode::Grid);
        assert!(!app.should_quit);
        assert_eq!(app.cursor, Cursor::default());
        assert_eq!(message(&app), None);
    }

    #[test]
    fn blank_command_just_closes() {
        let mut app = grid_app(None);
        type_text(&mut app, ":  ");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.mode, Mode::Grid);
        assert_eq!(message(&app), None);
    }

    #[test]
    fn colon_help_opens_help() {
        let mut app = grid_app(None);
        type_text(&mut app, ":help");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.mode, Mode::Help);
    }

    #[test]
    fn unknown_and_write_commands_show_a_message() {
        let mut app = grid_app(None);
        type_text(&mut app, ":frob");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(message(&app), Some("unknown command: frob"));
        type_text(&mut app, ":w out.json");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            message(&app),
            Some("save isn't implemented yet (would write out.json)")
        );
    }

    #[test]
    fn messages_clear_after_three_seconds() {
        let mut app = grid_app(None);
        type_text(&mut app, ":frob");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let now = Instant::now();
        app.tick(now + Duration::from_secs(1));
        assert!(app.message.is_some());
        app.tick(now + Duration::from_secs(4));
        assert!(app.message.is_none());
    }

    #[test]
    fn command_line_edits_multi_byte_text() {
        let mut line = CommandLine::default();
        for character in "é日本".chars() {
            line.insert(character);
        }
        line.left();
        line.backspace(); // removes 日
        assert_eq!((line.text.as_str(), line.cursor), ("é本", 1));
        line.insert('x');
        assert_eq!(line.text, "éx本");
        assert_eq!(line.before_cursor(), "éx");
        line.cursor = 0;
        line.backspace(); // nothing before the cursor
        line.end();
        line.right(); // already at the end
        assert_eq!((line.text.as_str(), line.cursor), ("éx本", 3));
    }

    #[test]
    fn command_line_keys() {
        let mut app = grid_app(None);
        type_text(&mut app, ":abc");
        press(&mut app, KeyCode::Home, KeyModifiers::NONE);
        press_char(&mut app, 'w');
        press(&mut app, KeyCode::End, KeyModifiers::NONE);
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(
            app.mode,
            Mode::Command(CommandLine {
                text: "wab".to_string(),
                cursor: 3
            })
        );
        press(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(app.mode, Mode::Command(CommandLine::default()));
    }

    #[test]
    fn enter_opens_cell_detail_and_esc_enter_or_q_close_it() {
        for close in [KeyCode::Esc, KeyCode::Enter, KeyCode::Char('q')] {
            let mut app = grid_app(None);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(app.mode, Mode::Detail);
            press_char(&mut app, 'j'); // ignored in the popup
            press(&mut app, close, KeyModifiers::NONE);
            assert_eq!(app.mode, Mode::Grid);
            assert!(!app.should_quit);
            assert_eq!(app.cursor, Cursor::default());
        }
    }

    #[test]
    fn record_view_moves_the_grid_cursor() {
        let mut app = grid_app(None);
        press_char(&mut app, 'r');
        assert_eq!(app.mode, Mode::Record);
        press_char(&mut app, 'n');
        press_char(&mut app, 'n');
        press_char(&mut app, 'j');
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        press_char(&mut app, 'k');
        press_char(&mut app, 'p');
        // `q` goes back to the grid instead of quitting.
        press_char(&mut app, 'q');
        assert_eq!(app.mode, Mode::Grid);
        assert!(!app.should_quit);
        assert_eq!(app.cursor, Cursor { row: 1, col: 1 });
    }

    #[test]
    fn record_view_scrolls_the_grid_to_the_cursor() {
        let mut app = grid_app(None);
        press_char(&mut app, 'r');
        for _ in 0..15 {
            press_char(&mut app, 'n');
        }
        for _ in 0..9 {
            press_char(&mut app, 'j');
        }
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.cursor, Cursor { row: 15, col: 9 });
        assert_eq!(app.first_row, 6);
        assert!(on_screen(&app, 9));
    }

    #[test]
    fn cell_views_need_cells() {
        let mut app =
            App::new(Table::parse(r#"{"columns": [{"name": "a"}], "rows": []}"#).unwrap());
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.mode, Mode::Grid);
        assert_eq!(message(&app), Some("no rows"));

        let mut app = App::new(Table::parse("[{}]").unwrap());
        press_char(&mut app, 'r');
        assert_eq!(app.mode, Mode::Grid);
        assert_eq!(message(&app), Some("no columns"));
    }

    #[test]
    fn every_record_binding_has_help_text() {
        for binding in RECORD_BINDINGS {
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
