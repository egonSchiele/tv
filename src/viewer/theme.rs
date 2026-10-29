//! Styles for each kind of cell, with a monochrome fallback for `NO_COLOR`.

use ratatui::style::{Color, Modifier, Style};

pub struct Theme {
    pub header: Style,
    pub cursor_cell: Style,
    /// The cursor's row and column. They're one highlight tier, so one style.
    pub cursor_line: Style,
    pub null: Style,
    /// Cells the last reload changed, for a few seconds.
    pub changed: Style,
    /// The comment marker `▸`: a thread awaiting a reply, or one the agent
    /// answered. The marker is patched over the cell's style, so each clears
    /// the modifier that would muddle it: a null cell is dim, and in mono the
    /// cursor's row and column are bold.
    pub awaiting: Style,
    pub replied: Style,
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
            // 58 is a dark olive, distinct from the cursor line's gray; 230 a
            // pale yellow that reads on it.
            changed: Style::new().bg(Color::Indexed(58)).fg(Color::Indexed(230)),
            // The spec's colors. Named colors follow the user's palette.
            awaiting: Style::new()
                .fg(Color::Yellow)
                .remove_modifier(Modifier::DIM),
            replied: Style::new().fg(Color::Green).remove_modifier(Modifier::DIM),
        }
    }

    /// No colors: reverse video for the cursor, bold for its row and column,
    /// underlined for changed cells, and a heavy or faint comment marker.
    pub fn mono() -> Self {
        Theme {
            header: Style::new().add_modifier(Modifier::BOLD),
            cursor_cell: Style::new().add_modifier(Modifier::REVERSED),
            cursor_line: Style::new().add_modifier(Modifier::BOLD),
            null: Style::new().add_modifier(Modifier::DIM),
            changed: Style::new().add_modifier(Modifier::UNDERLINED),
            awaiting: Style::new()
                .add_modifier(Modifier::BOLD)
                .remove_modifier(Modifier::DIM),
            replied: Style::new()
                .add_modifier(Modifier::DIM)
                .remove_modifier(Modifier::BOLD),
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
