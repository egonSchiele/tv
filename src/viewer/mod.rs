//! The terminal viewer: `app` holds state, `render` draws it, `layout` does
//! the column math, `sync` does the session's file IO, `diff` compares a
//! reloaded table with the old one, and this module runs the event loop.

mod app;
mod diff;
pub mod layout;
mod render;
mod sync;
mod theme;

use std::time::{Duration, Instant};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};
use ratatui::layout::Rect;

use crate::session::Session;
use crate::table::Table;
use app::App;
use sync::SessionSync;
use theme::Theme;

/// How long to wait for a key before drawing again. Each wake-up also checks
/// whether `table.json` changed (`SessionSync::tick`), so a reload shows up
/// within this long.
const TICK: Duration = Duration::from_millis(250);

pub fn run(table: Table, session: Session) -> anyhow::Result<()> {
    let mut app = App::new(table);
    app.session_id = Some(session.id().to_string());
    app.theme = Theme::detect();
    let mut sync = SessionSync::start(session, &mut app);
    // `ratatui::run` switches the terminal to raw mode and the alternate
    // screen, and puts it back afterwards, also if we return an error or panic.
    let result = ratatui::run(|terminal| event_loop(terminal, &mut app, &mut sync));
    // Also after an error, so `view.json` says the viewer is gone. The loop's
    // error matters more, so it's the one returned if both fail.
    let finished = sync.finish(&app);
    result.and(finished)
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    sync: &mut SessionSync,
) -> anyhow::Result<()> {
    while !app.should_quit {
        let now = Instant::now();
        let size = terminal.size()?;
        fit_viewport(app, Rect::new(0, 0, size.width, size.height));
        app.tick(now);
        sync.tick(app, now);
        terminal.draw(|frame| render::draw(frame, app))?;
        // Wake up early when a `view.json` write is due. A resize event needs
        // no handling: the next draw uses the new size.
        let timeout = sync.next_due(now).map_or(TICK, |due| due.min(TICK));
        if event::poll(timeout)?
            && let Event::Key(key) = event::read()?
        {
            app.handle_key(key);
        }
    }
    Ok(())
}

/// Tell `app` how big the grid is on a screen of this size. Called before
/// every draw, so a resize is picked up on the next frame.
fn fit_viewport(app: &mut App, screen: Rect) {
    let grid = render::areas(screen).body;
    app.set_viewport(grid.width, grid.height);
}
