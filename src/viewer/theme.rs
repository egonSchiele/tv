//! Styles for each kind of cell, with a monochrome fallback for `NO_COLOR`.

use ratatui::style::{Color, Modifier, Style};

pub struct Theme {
    pub header: Style,
    pub cursor_cell: Style,
    /// The cursor's row and column. They're one highlight tier, so one style.
    pub cursor_line: Style,
    pub null: Style,
}

impl Theme {
    pub fn color() -> Self {
        Theme {
            header: Style::new().add_modifier(Modifier::BOLD),
            cursor_cell: Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD),
            // 256-color palette entries: 236 is a dark gray just lighter than a
            // black background, 252 a light gray. Setting the foreground too
            // keeps the row readable on light terminal themes, whose default
            // text is near-black.
            cursor_line: Style::new().bg(Color::Indexed(236)).fg(Color::Indexed(252)),
            null: Style::new().add_modifier(Modifier::DIM),
        }
    }

    /// No colors: reverse video for the cursor, bold for its row and column.
    pub fn mono() -> Self {
        Theme {
            header: Style::new().add_modifier(Modifier::BOLD),
            cursor_cell: Style::new().add_modifier(Modifier::REVERSED),
            cursor_line: Style::new().add_modifier(Modifier::BOLD),
            null: Style::new().add_modifier(Modifier::DIM),
        }
    }

    /// `mono` when `NO_COLOR` is set to a non-empty value (see no-color.org).
    pub fn detect() -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
        if no_color {
            Theme::mono()
        } else {
            Theme::color()
        }
    }
}
