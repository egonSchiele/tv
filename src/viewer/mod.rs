//! The terminal viewer: `app` holds state, `render` draws it, `layout` does
//! the column math, and this module runs the event loop.

mod app;
mod layout;
mod render;

use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};

use crate::table::Table;
use app::App;

/// How long to wait for a key before drawing again. Live reload (ADI-218)
/// will check for file changes on the same tick.
const TICK: Duration = Duration::from_millis(250);

pub fn run(table: Table) -> anyhow::Result<()> {
    let mut app = App::new(table);
    // `ratatui::run` switches the terminal to raw mode and the alternate
    // screen, and puts it back afterwards, also if we return an error or panic.
    ratatui::run(|terminal| event_loop(terminal, &mut app))
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> anyhow::Result<()> {
    while !app.should_quit {
        terminal.draw(|frame| render::draw(frame, app))?;
        // A resize event needs no handling: the next draw uses the new size.
        if event::poll(TICK)?
            && let Event::Key(key) = event::read()?
        {
            app.handle_key(key);
        }
    }
    Ok(())
}
