//! The terminal viewer: `app` holds state, `render` draws it, `layout` does
//! the column math, and this module runs the event loop.

mod app;
mod layout;
mod render;
mod theme;

use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};
use ratatui::layout::Rect;

use crate::table::Table;
use app::App;
use theme::Theme;

/// How long to wait for a key before drawing again. Live reload (ADI-218)
/// will check for file changes on the same tick.
const TICK: Duration = Duration::from_millis(250);

pub fn run(table: Table) -> anyhow::Result<()> {
    let mut app = App::new(table);
    app.theme = Theme::detect();
    // `ratatui::run` switches the terminal to raw mode and the alternate
    // screen, and puts it back afterwards, also if we return an error or panic.
    ratatui::run(|terminal| event_loop(terminal, &mut app))
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> anyhow::Result<()> {
    while !app.should_quit {
        let size = terminal.size()?;
        sync_viewport(app, Rect::new(0, 0, size.width, size.height));
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

/// Tell `app` how big the grid is on a screen of this size. Called before
/// every draw, so a resize is picked up on the next frame.
fn sync_viewport(app: &mut App, screen: Rect) {
    let grid = render::areas(screen).body;
    app.set_viewport(grid.width, grid.height);
}
