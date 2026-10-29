//! Viewer state. All mutation happens here; render only reads it.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::layout;
use crate::table::Table;

pub struct App {
    // Private because `widths` is derived from `table`. When the table can
    // change (live reload), add a `set_table` that recomputes `widths`, so
    // nothing can update one without the other.
    table: Table,
    widths: Vec<u16>,
    pub session_id: Option<String>, // None until sessions exist
    pub first_col: usize,           // horizontal scroll, fixed at 0 for now
    pub first_row: usize,           // vertical scroll, fixed at 0 for now
    pub should_quit: bool,
}

impl App {
    pub fn new(table: Table) -> Self {
        let widths = layout::column_widths(&table);
        App {
            table,
            widths,
            session_id: None,
            first_col: 0,
            first_row: 0,
            should_quit: false,
        }
    }

    pub fn table(&self) -> &Table {
        &self.table
    }

    pub fn widths(&self) -> &[u16] {
        &self.widths
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        // Some terminals also report key releases; act on presses only.
        if key.kind != KeyEventKind::Press {
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            // Raw mode turns off the terminal's own Ctrl-C handling, so we quit ourselves.
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new(Table::parse(r#"[{"a": 1}]"#).unwrap())
    }

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        app.handle_key(KeyEvent::new(code, modifiers));
    }

    #[test]
    fn q_quits() {
        let mut app = app();
        press(&mut app, KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_c_quits() {
        let mut app = app();
        press(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.should_quit);
    }

    #[test]
    fn other_keys_do_nothing() {
        let mut app = app();
        press(&mut app, KeyCode::Char('c'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(!app.should_quit);
    }

    #[test]
    fn key_releases_are_ignored() {
        let mut app = app();
        let mut release = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        app.handle_key(release);
        assert!(!app.should_quit);
    }
}
