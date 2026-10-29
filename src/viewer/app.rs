//! Viewer state. All mutation happens here; render only reads it.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::diff;
use super::layout::{self, VisibleColumn};
use super::theme::Theme;
use crate::comments::{self, Add, Author, Event, Thread};
use crate::files;
use crate::table::Table;

/// Indices into `table.rows` and `table.columns`. Also names a cell in a
/// set of highlighted cells, hence `Hash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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
    Comment,
    NextComment,
    PreviousComment,
    Save,
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

/// Keys in the cell popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailAction {
    Close,
    Comment,
    Delete,
}

pub const DETAIL_BINDINGS: &[Binding<DetailAction>] = &[
    Binding {
        keys: &[
            Key::plain(KeyCode::Esc),
            Key::plain(KeyCode::Enter),
            Key::char('q'),
        ],
        action: DetailAction::Close,
        help: "close",
    },
    Binding {
        keys: &[Key::char('c')],
        action: DetailAction::Comment,
        help: "comment on this cell",
    },
    Binding {
        keys: &[Key::char('d')],
        action: DetailAction::Delete,
        help: "delete this cell's comments",
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
    /// Typing a comment on a cell.
    Comment(CommentInput),
    /// The cell popup, asking whether to delete these threads (their IDs).
    ConfirmDelete(Vec<String>),
}

/// A comment being typed, and the cell it's for: named by row ID and column,
/// so a reload while typing can't move it to another cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentInput {
    pub line: CommandLine,
    row_id: String,
    column: String,
}

/// How a cell's comment marker is colored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// Some thread on the cell is awaiting a reply.
    Awaiting,
    /// The agent replied last on every thread on the cell.
    Replied,
}

/// The `:` line being typed. `cursor` counts characters, not bytes, so typing
/// `é` can't split a UTF-8 sequence; it becomes a byte offset only at the edit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandLine {
    pub text: String,
    pub cursor: usize,
}

impl CommandLine {
    /// A line with `text` already typed and the cursor at the end.
    pub fn new(text: String) -> Self {
        let cursor = text.chars().count();
        CommandLine { text, cursor }
    }

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

    /// Apply an editing key. Both input lines (`:` and `c`) use this, so they
    /// can't start behaving differently. `Enter` and `Esc` are the caller's;
    /// other keys it doesn't know are ignored.
    fn edit(&mut self, key: &KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Backspace => self.backspace(),
            KeyCode::Left => self.left(),
            KeyCode::Right => self.right(),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.end(),
            KeyCode::Char('u') if ctrl => *self = CommandLine::default(),
            KeyCode::Char(character) if !ctrl => self.insert(character),
            _ => {}
        }
    }
}

/// A command typed after `:`. Named so it isn't confused with clap's
/// `Command` in `main.rs`. To add one, add a variant, a `from_str` arm, a
/// `HELP` row, and a `run_command` arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColonCommand {
    Help,
    Quit,
    /// `:w [path]`, or `:w! [path]` to overwrite a file.
    Write {
        path: Option<String>,
        force: bool,
    },
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
        // A trailing `!` (vim's "force") is split off once here, so each
        // command stays one arm. Only `w` accepts it for now.
        let (name, force) = match name.strip_suffix('!') {
            Some(name) => (name, true),
            None => (name, false),
        };
        Ok(match (name, argument, force) {
            ("help" | "h", "", false) => ColonCommand::Help,
            ("q" | "quit", "", false) => ColonCommand::Quit,
            ("w" | "write", path, force) => ColonCommand::Write {
                path: (!path.is_empty()).then(|| path.to_string()),
                force,
            },
            _ => ColonCommand::Unknown(text.to_string()),
        })
    }
}

impl ColonCommand {
    /// Listed in the help overlay. Kept next to `from_str` so they're edited together.
    pub const HELP: &'static [(&'static str, &'static str)] = &[
        (":help", "show this help"),
        (":q", "quit"),
        (":w [path]", "save; default ./<title>.json"),
        (":w! [path]", "save, overwriting a file"),
    ];
}

/// How long a status-line message stays up.
const MESSAGE_DURATION: Duration = Duration::from_secs(3);

/// How long cells stay highlighted after a reload changed them.
const HIGHLIGHT_DURATION: Duration = Duration::from_secs(5);

/// The cells the last reload changed, and when to stop showing that.
struct Highlight {
    cells: HashSet<Cursor>,
    until: Instant,
}

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
        keys: &[Key::char('c')],
        action: Action::Comment,
        help: "comment on this cell",
    },
    Binding {
        keys: &[Key::char(']')],
        action: Action::NextComment,
        help: "next comment",
    },
    Binding {
        keys: &[Key::char('[')],
        action: Action::PreviousComment,
        help: "previous comment",
    },
    Binding {
        keys: &[Key::char('s')],
        action: Action::Save,
        help: "save (opens :w with the default path)",
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
    pub session_id: Option<String>, // shown in the title bar; tests may leave it None
    pub cursor: Cursor,
    pub first_row: usize, // vertical scroll offset
    pub first_col: usize, // index of the first non-pinned column shown
    viewport: Viewport,   // set only through set_viewport
    pub theme: Theme,
    pub mode: Mode,
    pub message: Option<Message>,
    /// Why the last reload of `table.json` failed, until one succeeds. The
    /// status line shows it whenever no message is up.
    pub reload_error: Option<String>,
    highlight: Option<Highlight>, // set only through set_table
    /// The session's comment log as of the last poll, and the threads folded
    /// from it against `table`. `refold` keeps `threads` in step with both.
    events: Vec<Event>,
    threads: Vec<Thread>,
    /// The threads on each cell, as indices into `threads`. Orphaned threads
    /// have no cell, so they aren't here.
    thread_cells: HashMap<Cursor, Vec<usize>>,
    /// Comment events from the keyboard, for `SessionSync` to write.
    pending: Vec<Event>,
    /// The current file in vim's sense: where the last successful save went.
    /// It starts as `None` even after `tv file.json`, so `:w` and `s` default
    /// to `./<title-slug>.json` as the spec says. That's deliberate.
    save_path: Option<PathBuf>,
    pub should_quit: bool,
}

impl App {
    pub fn new(table: Table) -> Self {
        let widths = layout::column_widths(&table);
        // Start on the leftmost column on screen, which is the key column if
        // there is one. `tv agent goto` uses the same rule to say where a new
        // viewer's cursor will be.
        let cursor = Cursor {
            row: 0,
            col: table.start_column(),
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
            reload_error: None,
            highlight: None,
            events: Vec::new(),
            threads: Vec::new(),
            thread_cells: HashMap::new(),
            pending: Vec::new(),
            save_path: None,
            should_quit: false,
        }
    }

    pub fn table(&self) -> &Table {
        &self.table
    }

    /// Replace the table after a reload. Recomputes `widths` (the reason
    /// `table` is private), keeps the cursor on the same row ID and column
    /// name, clamps if either is gone, and highlights what changed. The
    /// highlight replaces the previous reload's: its indices were into the
    /// old table and may point at different cells now.
    pub fn set_table(&mut self, table: Table, now: Instant) {
        let cursor = diff::same_cell(&self.table, self.cursor, &table);
        let changed = diff::changed_cells(&self.table, &table);
        self.widths = layout::column_widths(&table);
        self.table = table;
        self.highlight = Some(Highlight {
            cells: changed,
            until: now + HIGHLIGHT_DURATION,
        });
        self.reload_error = None;
        self.refold();
        self.move_to(cursor.row, cursor.col); // clamps and scrolls into view
        // Detail, Record and ConfirmDelete need a cell under the cursor, the
        // same check `open_cell_view` makes before opening them. Comment
        // knows its own cell.
        let needs_cell = matches!(
            self.mode,
            Mode::Detail | Mode::Record | Mode::ConfirmDelete(_)
        );
        if needs_cell && !self.has_cell() {
            self.mode = Mode::Grid;
        }
    }

    /// Replace the comment log after a poll, and refold.
    pub fn set_events(&mut self, events: Vec<Event>) {
        self.events = events;
        self.refold();
    }

    /// The session's comment threads, folded against the current table.
    pub fn threads(&self) -> &[Thread] {
        &self.threads
    }

    /// The comment marker for a cell, if it has threads.
    pub fn marker(&self, row: usize, col: usize) -> Option<Marker> {
        let indices = self.thread_cells.get(&Cursor { row, col })?;
        let awaiting = indices
            .iter()
            .any(|&index| self.threads[index].awaiting_reply);
        Some(if awaiting {
            Marker::Awaiting
        } else {
            Marker::Replied
        })
    }

    /// The cursor cell's threads, in log order, for the cell popup.
    pub fn cell_threads(&self) -> Vec<&Thread> {
        self.thread_cells
            .get(&self.cursor)
            .into_iter()
            .flatten()
            .map(|&index| &self.threads[index])
            .collect()
    }

    /// The comment events queued since the last call, for `SessionSync`.
    pub(super) fn take_pending(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.pending)
    }

    /// Fold `events` against `table`, and index the threads by cell.
    /// `set_events` and `set_table` both call it: a reload can orphan a
    /// thread, move its cell, or change its `value`. Where a thread's cell is
    /// comes from `comments::threads` (`Thread.cell`), so the markers and the
    /// `orphaned` flag can't disagree.
    fn refold(&mut self) {
        self.threads = comments::threads(&self.events, &self.table);
        self.thread_cells.clear();
        for (index, thread) in self.threads.iter().enumerate() {
            if let Some((row, col)) = thread.cell {
                self.thread_cells
                    .entry(Cursor { row, col })
                    .or_default()
                    .push(index);
            }
        }
    }

    /// Whether `tv agent goto` may move the cursor now: only in the grid. In
    /// a popup, the record view, an input line or the help overlay, moving it
    /// would change what the human is reading or typing about.
    pub fn ready_for_goto(&self) -> bool {
        self.mode == Mode::Grid
    }

    /// Move the cursor where `tv agent goto` asked: row `row_id`, and column
    /// `column` or the current one. The status line says the agent moved it,
    /// since a cursor jumping on its own is puzzling. A row or column the
    /// table doesn't have (a push since the request) moves nothing, and the
    /// status line says why.
    pub fn go_to(&mut self, row_id: &str, column: Option<&str>) {
        let Some(row) = self.table.row_index(row_id) else {
            self.set_message(format!("goto: no row \"{row_id}\""));
            return;
        };
        let col = match column {
            None => self.cursor.col,
            Some(name) => {
                let Some(col) = self.table.column_index(name) else {
                    self.set_message(format!("goto: no column \"{name}\""));
                    return;
                };
                col
            }
        };
        self.move_to(row, col);
        self.set_message("the agent moved the cursor here".to_string());
    }

    /// Whether the last reload changed this cell, for the next few seconds.
    pub fn is_highlighted(&self, row: usize, col: usize) -> bool {
        self.highlight
            .as_ref()
            .is_some_and(|highlight| highlight.cells.contains(&Cursor { row, col }))
    }

    /// Whether there's a cell under the cursor: the table has at least one
    /// row and one column. Detail and Record modes need one; `draw_detail`
    /// indexes the cursor's column.
    pub fn has_cell(&self) -> bool {
        !self.table.rows.is_empty() && !self.table.columns.is_empty()
    }

    /// The cursor row's ID (`Table::row_id`), or `None` without a cell.
    pub fn row_id(&self) -> Option<String> {
        self.has_cell().then(|| self.table.row_id(self.cursor.row))
    }

    /// The cursor column's name, or `None` without a cell.
    pub fn column_name(&self) -> Option<String> {
        self.has_cell()
            .then(|| self.table.columns[self.cursor.col].name.clone())
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
            Mode::Command(line) => match key.code {
                KeyCode::Esc => self.mode = Mode::Grid,
                KeyCode::Enter => {
                    let text = line.text.trim().to_string();
                    self.mode = Mode::Grid;
                    if !text.is_empty() {
                        let Ok(command) = text.parse();
                        self.run_command(command);
                    }
                }
                _ => line.edit(&key),
            },
            Mode::Comment(input) => match key.code {
                KeyCode::Esc => self.mode = Mode::Grid,
                KeyCode::Enter => {
                    let input = input.clone();
                    self.mode = Mode::Grid;
                    self.send_comment(input);
                }
                _ => input.line.edit(&key),
            },
            Mode::Detail => {
                if let Some(action) = lookup(DETAIL_BINDINGS, &key) {
                    self.perform_in_detail(action);
                }
            }
            // Only `y` deletes; any other key keeps the threads.
            Mode::ConfirmDelete(ids) => {
                if Key::char('y').matches(&key) {
                    let deletes = std::mem::take(ids).into_iter().map(Event::delete);
                    self.pending.extend(deletes);
                }
                self.mode = Mode::Detail;
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

    fn perform_in_detail(&mut self, action: DetailAction) {
        match action {
            DetailAction::Close => self.mode = Mode::Grid,
            DetailAction::Comment => self.open_comment(),
            DetailAction::Delete => {
                let ids: Vec<String> = self
                    .cell_threads()
                    .iter()
                    .map(|thread| thread.id.clone())
                    .collect();
                if ids.is_empty() {
                    self.set_message("no comments on this cell".to_string());
                } else {
                    self.mode = Mode::ConfirmDelete(ids);
                }
            }
        }
    }

    /// Open a view of the cursor's cell or row, if there is one.
    fn open_cell_view(&mut self, mode: Mode) {
        if self.has_cell() {
            self.mode = mode;
        } else {
            self.set_message(self.no_cell_message().to_string());
        }
    }

    /// Why there's no cell to open or comment on.
    fn no_cell_message(&self) -> &'static str {
        if self.table.rows.is_empty() {
            "no rows"
        } else {
            "no columns"
        }
    }

    /// Start typing a comment on the cursor cell, if there is one.
    fn open_comment(&mut self) {
        match (self.row_id(), self.column_name()) {
            (Some(row_id), Some(column)) => {
                self.mode = Mode::Comment(CommentInput {
                    line: CommandLine::default(),
                    row_id,
                    column,
                });
            }
            _ => self.set_message(self.no_cell_message().to_string()),
        }
    }

    /// Queue the comment typed in `input` for `SessionSync` to write: a reply
    /// to the newest thread on its cell, or a new thread. That's decided now,
    /// not when `c` was pressed, so a thread the agent started meanwhile is
    /// joined. Empty text sends nothing.
    fn send_comment(&mut self, input: CommentInput) {
        let text = input.line.text.trim().to_string();
        if text.is_empty() {
            return;
        }
        // The queue too, so this doesn't depend on `pending` being written
        // before the next key.
        let id = comments::new_id(self.events.iter().chain(&self.pending));
        let thread = self
            .threads
            .iter()
            .rev()
            .find(|thread| thread.row_id == input.row_id && thread.column == input.column);
        let add = match thread {
            Some(thread) => Add::reply(id, Author::User, thread.id.clone(), text),
            None => Add::comment(id, Author::User, input.row_id, input.column, text),
        };
        self.pending.push(Event::Add(add));
    }

    /// Move to the next (or previous) cell with a thread, in reading order:
    /// by row, then by screen position, so the pinned key column comes first.
    /// Wraps around at either end.
    fn jump_to_comment(&mut self, forward: bool) {
        let order = self.column_order();
        let screen_position = |col: usize| {
            order
                .iter()
                .position(|&candidate| candidate == col)
                .unwrap_or(0)
        };
        let mut cells: Vec<(usize, usize, usize)> = self
            .thread_cells
            .keys()
            .map(|cell| (cell.row, screen_position(cell.col), cell.col))
            .collect();
        cells.sort_unstable();
        let here = (self.cursor.row, self.cursor_position());
        let target = if forward {
            cells
                .iter()
                .find(|&&(row, position, _)| (row, position) > here)
                .or(cells.first())
        } else {
            cells
                .iter()
                .rev()
                .find(|&&(row, position, _)| (row, position) < here)
                .or(cells.last())
        };
        match target {
            Some(&(row, _, col)) => self.move_to(row, col),
            None => self.set_message("no comments".to_string()),
        }
    }

    /// Clear the status-line message once it's `MESSAGE_DURATION` old, and
    /// the reload highlight once it's over. Takes the time as an argument so
    /// tests don't have to sleep.
    pub fn tick(&mut self, now: Instant) {
        let expired = self
            .message
            .as_ref()
            .is_some_and(|message| now.duration_since(message.shown_at) >= MESSAGE_DURATION);
        if expired {
            self.message = None;
        }
        let highlight_over = self
            .highlight
            .as_ref()
            .is_some_and(|highlight| now >= highlight.until);
        if highlight_over {
            self.highlight = None;
        }
    }

    pub(super) fn set_message(&mut self, text: String) {
        self.message = Some(Message {
            text,
            shown_at: Instant::now(),
        });
    }

    fn run_command(&mut self, command: ColonCommand) {
        match command {
            ColonCommand::Help => self.mode = Mode::Help,
            ColonCommand::Quit => self.should_quit = true,
            ColonCommand::Write { path, force } => self.save(path, force),
            ColonCommand::Unknown(text) => self.set_message(format!("unknown command: {text}")),
        }
    }

    /// Where `:w` and `s` save by default: the last save's path, or
    /// `./<title-slug>.json` (`./table.json` if the title gives no slug).
    fn default_save_path(&self) -> PathBuf {
        if let Some(path) = &self.save_path {
            return path.clone();
        }
        let slug = files::slug(self.table.title.as_deref().unwrap_or(""));
        let name = if slug.is_empty() { "table" } else { &slug };
        PathBuf::from(format!("./{name}.json"))
    }

    /// Save the table and its comment threads, and say how it went in the
    /// status line. Saving over an
    /// existing file is refused, unless it's the current file or `force` is
    /// set (vim's rule). The writing itself is `Table::save`.
    fn save(&mut self, path: Option<String>, force: bool) {
        let target = match path {
            Some(path) => files::expand_tilde(&path),
            None => self.default_save_path(),
        };
        if target.exists() && !force && !self.is_save_path(&target) {
            self.set_message(format!("{} exists; use :w! to overwrite", target.display()));
            return;
        }
        // The threads go in the file: a session keeps them in its log, not in
        // `table`.
        let saved = Table {
            comments: self.threads.clone(),
            ..self.table.clone()
        };
        match saved.save(&target) {
            Ok(()) => {
                self.set_message(format!(
                    "wrote {} rows to {}",
                    self.table.rows.len(),
                    target.display()
                ));
                self.save_path = Some(target);
            }
            Err(error) => self.set_message(format!("save failed: {error:#}")),
        }
    }

    /// Whether `target`, which exists, is the current file. Comparing
    /// canonical paths makes `./a.json` and `a.json` match; `canonicalize`
    /// fails on a missing path, which is why this only runs for existing ones.
    fn is_save_path(&self, target: &Path) -> bool {
        let Some(saved) = &self.save_path else {
            return false;
        };
        match (fs::canonicalize(saved), fs::canonicalize(target)) {
            (Ok(saved), Ok(target)) => saved == target,
            _ => false,
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
            Action::Comment => self.open_comment(),
            Action::NextComment => self.jump_to_comment(true),
            Action::PreviousComment => self.jump_to_comment(false),
            // The spec's "prompt for a path": the command line, prefilled.
            Action::Save => {
                let text = format!("w {}", self.default_save_path().display());
                self.mode = Mode::Command(CommandLine::new(text));
            }
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

        // A reload can leave `first_col` past the last column; then start
        // from the left again. This comes before the pinned-column return,
        // which would otherwise skip it and leave only the key column on
        // screen.
        if self.first_col >= self.table.columns.len() {
            self.first_col = 0;
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

    fn write(path: Option<&str>, force: bool) -> ColonCommand {
        ColonCommand::Write {
            path: path.map(str::to_string),
            force,
        }
    }

    #[test]
    fn colon_commands_parse() {
        let cases = [
            ("help", ColonCommand::Help),
            ("h", ColonCommand::Help),
            (" q ", ColonCommand::Quit),
            ("quit", ColonCommand::Quit),
            ("w", write(None, false)),
            ("w  out file.json ", write(Some("out file.json"), false)),
            ("write a.json", write(Some("a.json"), false)),
            ("w!", write(None, true)),
            ("write! ~/a.json", write(Some("~/a.json"), true)),
            ("q!", ColonCommand::Unknown("q!".to_string())),
            ("help!", ColonCommand::Unknown("help!".to_string())),
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
    fn unknown_commands_show_a_message() {
        let mut app = grid_app(None);
        type_text(&mut app, ":frob");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(message(&app), Some("unknown command: frob"));
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
    fn the_cursor_cell_is_named_by_row_id_and_column() {
        let mut app = grid_app(Some("c0"));
        app.cursor = Cursor { row: 3, col: 2 };
        assert_eq!(app.row_id().as_deref(), Some("r3c0"));
        assert_eq!(app.column_name().as_deref(), Some("c2"));

        for json in [r#"{"columns": [{"name": "a"}], "rows": []}"#, "[{}]"] {
            let app = App::new(Table::parse(json).unwrap());
            assert_eq!((app.row_id(), app.column_name()), (None, None), "{json}");
        }
    }

    #[test]
    fn every_record_binding_has_help_text() {
        for binding in RECORD_BINDINGS {
            assert_ne!(binding.help, "");
            assert_ne!(binding.keys.len(), 0);
        }
    }

    #[test]
    fn every_detail_binding_has_help_text() {
        for binding in DETAIL_BINDINGS {
            assert_ne!(binding.help, "");
            assert_ne!(binding.keys.len(), 0);
        }
    }

    fn prs_app() -> App {
        App::new(Table::from_path("examples/prs.json").unwrap())
    }

    /// Type `:{command}` and press Enter.
    fn run_colon(app: &mut App, command: &str) {
        type_text(app, &format!(":{command}"));
        press(app, KeyCode::Enter, KeyModifiers::NONE);
    }

    fn command_line_text(app: &App) -> &str {
        match &app.mode {
            Mode::Command(line) => &line.text,
            other => panic!("expected the command line, got {other:?}"),
        }
    }

    // Every save test writes inside a temp dir, never the repo.

    #[test]
    fn colon_w_saves_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.json");
        let mut app = prs_app();
        run_colon(&mut app, &format!("w {}", path.display()));
        assert_eq!(
            message(&app),
            Some(format!("wrote 40 rows to {}", path.display()).as_str())
        );
        let saved = Table::from_path(&path).unwrap();
        assert_eq!(saved.rows, app.table().rows);
    }

    #[test]
    fn plain_colon_w_rewrites_the_current_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.json");
        let mut app = prs_app();
        run_colon(&mut app, &format!("w {}", path.display()));
        fs::write(&path, "stale").unwrap();
        run_colon(&mut app, "w");
        assert!(Table::from_path(&path).is_ok());
    }

    #[test]
    fn colon_w_refuses_another_existing_file_and_bang_overwrites_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.json");
        fs::write(&path, "keep me").unwrap();
        let mut app = prs_app();

        run_colon(&mut app, &format!("w {}", path.display()));
        assert_eq!(
            message(&app),
            Some(format!("{} exists; use :w! to overwrite", path.display()).as_str())
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "keep me");

        run_colon(&mut app, &format!("w! {}", path.display()));
        assert!(Table::from_path(&path).is_ok());
    }

    #[test]
    fn the_current_file_matches_however_its_path_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = prs_app();
        run_colon(
            &mut app,
            &format!("w {}", dir.path().join("a.json").display()),
        );
        // The same file by another path is still the current file.
        let other_spelling = dir.path().join(".").join("a.json");
        run_colon(&mut app, &format!("w {}", other_spelling.display()));
        assert!(message(&app).unwrap().starts_with("wrote 40 rows"));
    }

    #[test]
    fn a_failed_save_says_why_and_keeps_the_viewer_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("a.json");
        let mut app = prs_app();
        run_colon(&mut app, &format!("w {}", path.display()));
        let expected = format!("save failed: writing {}: ", path.display());
        assert!(
            message(&app).unwrap().starts_with(&expected),
            "{:?}",
            message(&app)
        );
        assert!(!app.should_quit);
        assert_eq!(app.mode, Mode::Grid);
    }

    #[test]
    fn s_prefills_the_default_path() {
        let mut app = prs_app();
        press_char(&mut app, 's');
        assert_eq!(command_line_text(&app), "w ./open-pull-requests.json");
        // Don't press Enter here: the `./` default would write into the repo.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.json");
        run_colon(&mut app, &format!("w {}", path.display()));
        press_char(&mut app, 's');
        assert_eq!(command_line_text(&app), format!("w {}", path.display()));
        // The cursor is at the end, ready for Enter.
        let Mode::Command(line) = &app.mode else {
            unreachable!()
        };
        assert_eq!(line.cursor, line.text.chars().count());
    }

    #[test]
    fn default_save_path_falls_back_to_table() {
        for json in [r#"[{"a": 1}]"#, r#"{"title": "!!! ???", "rows": []}"#] {
            let app = App::new(Table::parse(json).unwrap());
            assert_eq!(app.default_save_path(), PathBuf::from("./table.json"));
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

    /// `rows` rows by `cols` columns named `c0`…; every cell is like `r2c1`.
    fn grid_table(rows: usize, cols: usize, key: Option<&str>) -> Table {
        let rows: Vec<String> = (0..rows)
            .map(|row| {
                let cells: Vec<String> = (0..cols)
                    .map(|col| format!(r#""c{col}": "r{row}c{col}""#))
                    .collect();
                format!("{{{}}}", cells.join(", "))
            })
            .collect();
        let key = key
            .map(|key| format!(r#""key": "{key}", "#))
            .unwrap_or_default();
        Table::parse(&format!(r#"{{{key}"rows": [{}]}}"#, rows.join(", "))).unwrap()
    }

    #[test]
    fn set_table_recomputes_widths() {
        let mut app = grid_app(None);
        let before = app.visible_columns().len();
        let wide = Table::parse(&format!(
            r#"[{{"c0": "{}", "c1": "x", "c2": "y"}}]"#,
            "w".repeat(30)
        ))
        .unwrap();
        app.set_table(wide, Instant::now());
        assert_eq!(app.visible_columns()[0].width, 30);
        assert!(app.visible_columns().len() < before);
    }

    #[test]
    fn set_table_clamps_a_cursor_past_the_end() {
        let mut app = grid_app(None);
        press_char(&mut app, 'G');
        assert_eq!((app.cursor.row, app.first_row), (49, 40));
        app.set_table(grid_table(5, 10, None), Instant::now());
        assert_eq!((app.cursor.row, app.first_row), (4, 0));
    }

    #[test]
    fn set_table_from_an_empty_table_highlights_everything() {
        let mut app = App::new(Table::parse("[]").unwrap());
        app.set_viewport(60, 10);
        app.set_table(grid_table(50, 10, None), Instant::now());
        assert_eq!(app.cursor, Cursor { row: 0, col: 0 });
        assert!(app.is_highlighted(0, 0));
        assert!(app.is_highlighted(49, 9));
    }

    #[test]
    fn set_table_follows_the_row_id() {
        let mut app = grid_app(Some("c0"));
        app.move_to(3, 2);
        // The same rows from r1 on: row r3 is now at index 2.
        let rows: Vec<String> = (1..50)
            .map(|row| format!(r#"{{"c0": "r{row}c0", "c2": "r{row}c2"}}"#))
            .collect();
        let shifted = Table::parse(&format!(
            r#"{{"key": "c0", "rows": [{}]}}"#,
            rows.join(", ")
        ))
        .unwrap();
        app.set_table(shifted, Instant::now());
        assert_eq!(app.cursor, Cursor { row: 2, col: 1 });
        assert!(!app.is_highlighted(2, 1));
    }

    #[test]
    fn a_reload_with_fewer_columns_scrolls_back_from_the_pinned_column() {
        let mut app = grid_app(Some("c0"));
        press_char(&mut app, '$');
        press_char(&mut app, '0'); // back on the pinned column; first_col stays
        assert!(app.first_col > 2, "{}", app.first_col);
        app.set_table(grid_table(50, 3, Some("c0")), Instant::now());
        assert_eq!(app.first_col, 0);
        assert_eq!(app.visible_columns().len(), 3);
    }

    #[test]
    fn a_reload_without_cells_closes_the_cell_views() {
        let empty = || Table::parse(r#"{"columns": [{"name": "c0"}], "rows": []}"#).unwrap();
        for key in [KeyCode::Enter, KeyCode::Char('r')] {
            let mut app = grid_app(None);
            press(&mut app, key, KeyModifiers::NONE);
            assert_ne!(app.mode, Mode::Grid);
            app.set_table(empty(), Instant::now());
            assert_eq!(app.mode, Mode::Grid);
        }

        let mut app = grid_app(None);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        app.set_table(grid_table(3, 3, None), Instant::now());
        assert_eq!(app.mode, Mode::Detail);
    }

    #[test]
    fn highlights_last_five_seconds() {
        let mut app = grid_app(None);
        let now = Instant::now();
        app.set_table(grid_table(51, 10, None), now);
        assert!(app.is_highlighted(50, 0));
        assert!(!app.is_highlighted(0, 0));
        app.tick(now + Duration::from_secs(4));
        assert!(app.is_highlighted(50, 0));
        app.tick(now + HIGHLIGHT_DURATION);
        assert!(!app.is_highlighted(50, 0));
    }

    #[test]
    fn set_table_clears_the_reload_error() {
        let mut app = grid_app(None);
        app.reload_error = Some("table.json: broken".to_string());
        app.set_table(grid_table(3, 3, None), Instant::now());
        assert_eq!(app.reload_error, None);
    }

    /// Log lines for a comment by `author` on `prs.json`'s PR `row_id`.
    fn comment_on(id: &str, author: &str, row_id: &str, column: &str) -> String {
        format!(
            r#"{{"op":"add","id":"{id}","author":"{author}","row_id":"{row_id}","column":"{column}","text":"{id} text","ts":"2026-10-28T19:03:00Z"}}"#
        )
    }

    fn log(lines: &[String]) -> Vec<Event> {
        comments::parse_log(&lines.join("\n")).events
    }

    #[test]
    fn set_events_folds_the_log() {
        let mut app = prs_app();
        assert!(app.threads.is_empty(), "{:?}", app.threads);
        app.set_events(log(&[comment_on("c1", "user", "404", "title")]));
        assert_eq!(app.threads.len(), 1);
        assert_eq!(
            app.threads[0].value,
            serde_json::json!("Bump serde to 1.0.229")
        );
        assert!(app.threads[0].awaiting_reply);
    }

    #[test]
    fn a_reload_without_the_commented_row_orphans_its_thread() {
        let mut app = prs_app();
        app.set_events(log(&[comment_on("c1", "user", "404", "title")]));
        assert!(!app.threads[0].orphaned);
        let mut table = app.table().clone();
        table.rows.retain(|row| row["number"] != 404);
        app.set_table(table, Instant::now());
        assert!(app.threads[0].orphaned);
    }

    #[test]
    fn colon_w_saves_the_threads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.json");
        let mut app = prs_app();
        app.set_events(log(&[comment_on("c1", "user", "404", "title")]));
        run_colon(&mut app, &format!("w {}", path.display()));
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            saved["comments"],
            serde_json::to_value(&app.threads).unwrap()
        );
        assert_eq!(saved["comments"][0]["awaiting_reply"], true);
    }

    fn reply_to(id: &str, author: &str, thread: &str) -> String {
        format!(
            r#"{{"op":"add","id":"{id}","author":"{author}","reply_to":"{thread}","text":"{id} text","ts":"2026-10-28T19:04:00Z"}}"#
        )
    }

    /// The one pending event, as an `Add`.
    fn pending_add(app: &mut App) -> comments::Add {
        let mut pending = app.take_pending();
        assert_eq!(pending.len(), 1, "{pending:?}");
        let Event::Add(add) = pending.remove(0) else {
            panic!("expected an add");
        };
        add
    }

    /// `c`, then `text`, then Enter.
    fn comment(app: &mut App, text: &str) {
        press_char(app, 'c');
        type_text(app, text);
        press(app, KeyCode::Enter, KeyModifiers::NONE);
    }

    #[test]
    fn c_comments_on_the_cursor_cell() {
        let mut app = prs_app();
        press_char(&mut app, 'j'); // PR 404
        press_char(&mut app, 'l'); // title
        press_char(&mut app, 'c');
        assert!(matches!(app.mode, Mode::Comment(_)), "{:?}", app.mode);
        type_text(&mut app, "hi");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.mode, Mode::Grid);
        let add = pending_add(&mut app);
        assert_eq!(add.author, Author::User);
        assert_eq!(add.text, "hi");
        assert_eq!(
            (add.row_id.as_deref(), add.column.as_deref()),
            (Some("404"), Some("title"))
        );
        assert_eq!(add.reply_to, None);
    }

    #[test]
    fn c_on_a_cell_with_threads_replies_to_the_newest() {
        let mut app = prs_app();
        app.set_events(log(&[
            comment_on("c1", "user", "409", "number"),
            comment_on("c2", "agent", "409", "number"),
        ]));
        comment(&mut app, "hi");
        let add = pending_add(&mut app);
        assert_eq!(add.reply_to.as_deref(), Some("c2"));
        assert_eq!((add.row_id, add.column), (None, None));
    }

    #[test]
    fn esc_or_nothing_typed_sends_nothing() {
        let mut app = prs_app();
        press_char(&mut app, 'c');
        type_text(&mut app, "hi");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.mode, Mode::Grid);
        comment(&mut app, "   ");
        assert_eq!(app.take_pending(), []);
    }

    #[test]
    fn c_without_a_cell_says_why() {
        let mut app = App::new(Table::parse("[]").unwrap());
        press_char(&mut app, 'c');
        assert_eq!(app.mode, Mode::Grid);
        assert_eq!(message(&app), Some("no rows"));
    }

    #[test]
    fn a_reload_while_typing_keeps_the_comments_cell() {
        let mut app = prs_app();
        press_char(&mut app, 'j'); // PR 404, at index 1
        press_char(&mut app, 'c');
        type_text(&mut app, "hi");
        let mut table = app.table().clone();
        table.rows.remove(0); // now 404 is at index 0
        app.set_table(table, Instant::now());
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(pending_add(&mut app).row_id.as_deref(), Some("404"));
    }

    #[test]
    fn the_input_lines_share_their_editing_keys() {
        let mut line = CommandLine::default();
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        for character in "abc".chars() {
            line.edit(&key(KeyCode::Char(character)));
        }
        line.edit(&key(KeyCode::Backspace));
        line.edit(&key(KeyCode::Home));
        line.edit(&key(KeyCode::Char('x')));
        assert_eq!((line.text.as_str(), line.cursor), ("xab", 1));
        line.edit(&key(KeyCode::End));
        line.edit(&key(KeyCode::Left));
        line.edit(&key(KeyCode::Char('y')));
        assert_eq!(line.text, "xayb");
        line.edit(&KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(line, CommandLine::default());
    }

    #[test]
    fn c_in_the_cell_popup_comments_too() {
        let mut app = prs_app();
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.mode, Mode::Detail);
        comment(&mut app, "hi");
        assert_eq!(app.mode, Mode::Grid);
        assert_eq!(pending_add(&mut app).row_id.as_deref(), Some("409"));
    }

    #[test]
    fn d_in_the_cell_popup_asks_then_deletes_every_thread_on_the_cell() {
        let mut app = prs_app();
        app.set_events(log(&[
            comment_on("c1", "user", "409", "number"),
            comment_on("c2", "agent", "409", "number"),
            comment_on("c3", "user", "404", "number"),
        ]));
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        press_char(&mut app, 'd');
        let ids = vec!["c1".to_string(), "c2".to_string()];
        assert_eq!(app.mode, Mode::ConfirmDelete(ids.clone()));
        press_char(&mut app, 'n');
        assert_eq!(app.mode, Mode::Detail);
        assert_eq!(app.take_pending(), []);

        press_char(&mut app, 'd');
        press_char(&mut app, 'y');
        assert_eq!(app.mode, Mode::Detail);
        let deleted: Vec<String> = app
            .take_pending()
            .iter()
            .map(|event| match event {
                Event::Delete { id, .. } => id.clone(),
                Event::Add(add) => panic!("expected deletes, got {add:?}"),
            })
            .collect();
        assert_eq!(deleted, ids);
    }

    #[test]
    fn d_without_threads_says_so() {
        let mut app = prs_app();
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        press_char(&mut app, 'd');
        assert_eq!(app.mode, Mode::Detail);
        assert_eq!(message(&app), Some("no comments on this cell"));
    }

    #[test]
    fn a_reload_without_cells_closes_the_delete_prompt() {
        let mut app = prs_app();
        app.mode = Mode::ConfirmDelete(vec!["c1".to_string()]);
        app.set_table(Table::parse("[{}]").unwrap(), Instant::now());
        assert_eq!(app.mode, Mode::Grid);
    }

    #[test]
    fn brackets_jump_between_threads_in_reading_order() {
        // Keyed on `c3`, so the key column is drawn first but isn't index 0.
        let mut app = grid_app(Some("c3"));
        app.set_events(log(&[
            comment_on("t1", "user", "r3c3", "c1"),
            comment_on("t2", "user", "r0c3", "c5"),
            comment_on("t3", "user", "r0c3", "c3"),
            comment_on("t4", "user", "r99c3", "c0"), // orphaned
        ]));
        app.cursor = Cursor { row: 1, col: 3 };
        let mut visited = Vec::new();
        for _ in 0..4 {
            press_char(&mut app, ']');
            visited.push((app.cursor.row, app.cursor.col));
        }
        // Row 3, then wrap to row 0: the key column (drawn first), then `c5`.
        assert_eq!(visited, [(3, 1), (0, 3), (0, 5), (3, 1)]);
        let mut visited = Vec::new();
        for _ in 0..3 {
            press_char(&mut app, '[');
            visited.push((app.cursor.row, app.cursor.col));
        }
        assert_eq!(visited, [(0, 5), (0, 3), (3, 1)]);
    }

    #[test]
    fn brackets_without_threads_say_so() {
        let mut app = prs_app();
        press_char(&mut app, ']');
        assert_eq!(message(&app), Some("no comments"));
        assert_eq!(app.cursor, Cursor { row: 0, col: 0 });
    }

    #[test]
    fn markers_say_whether_a_thread_awaits_a_reply() {
        let mut app = prs_app();
        app.set_events(log(&[
            comment_on("c1", "user", "409", "title"),
            reply_to("c2", "agent", "c1"),
            comment_on("c3", "user", "404", "title"),
            comment_on("c4", "user", "409", "author"),
            reply_to("c5", "agent", "c4"),
            comment_on("c6", "user", "409", "author"),
        ]));
        assert_eq!(app.marker(0, 0), None);
        assert_eq!(app.marker(0, 1), Some(Marker::Replied));
        assert_eq!(app.marker(1, 1), Some(Marker::Awaiting));
        // Two threads on 409's author; one is awaiting.
        assert_eq!(app.marker(0, 2), Some(Marker::Awaiting));
    }

    #[test]
    fn take_pending_empties_the_queue() {
        let mut app = prs_app();
        comment(&mut app, "hi");
        assert_eq!(app.take_pending().len(), 1);
        assert_eq!(app.take_pending(), []);
    }

    #[test]
    fn ready_for_goto_only_in_the_grid() {
        let mut app = prs_app();
        assert!(app.ready_for_goto());
        for mode in [
            Mode::Help,
            Mode::Detail,
            Mode::Record,
            Mode::Command(CommandLine::default()),
            Mode::ConfirmDelete(vec!["c1".to_string()]),
        ] {
            app.mode = mode;
            assert!(!app.ready_for_goto(), "{:?}", app.mode);
        }
        let mut app = prs_app();
        press_char(&mut app, 'c');
        assert!(!app.ready_for_goto(), "{:?}", app.mode);
    }

    #[test]
    fn go_to_moves_to_the_cell_and_says_so() {
        let mut app = prs_app();
        app.go_to("404", Some("title"));
        assert_eq!(app.cursor, Cursor { row: 1, col: 1 });
        assert_eq!(message(&app), Some("the agent moved the cursor here"));
        app.go_to("409", None);
        assert_eq!(app.cursor, Cursor { row: 0, col: 1 }, "keeps the column");
    }

    #[test]
    fn go_to_a_cell_thats_gone_moves_nothing() {
        let mut app = prs_app();
        app.go_to("404", Some("title"));
        app.go_to("412", Some("title"));
        assert_eq!(app.cursor, Cursor { row: 1, col: 1 });
        assert_eq!(message(&app), Some("goto: no row \"412\""));
        app.go_to("409", Some("nope"));
        assert_eq!(app.cursor, Cursor { row: 1, col: 1 });
        assert_eq!(message(&app), Some("goto: no column \"nope\""));
    }

    #[test]
    fn go_to_scrolls_the_cell_into_view() {
        let mut app = grid_app(None);
        app.go_to("#30", Some("c8"));
        assert_eq!(app.cursor, Cursor { row: 30, col: 8 });
        assert!(
            app.first_row <= 30 && 30 < app.first_row + 10,
            "{}",
            app.first_row
        );
        let visible = app.visible_columns();
        assert!(
            visible.iter().any(|column| column.index == 8),
            "{visible:?}"
        );
    }
}
