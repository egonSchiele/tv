//! The viewer's half of the session directory: `view.json` out (where the
//! cursor is), `table.json` in (live reload), and `comments.jsonl` both ways.
//! Everything the event loop does to the filesystem goes through here, with
//! the time passed in so tests control it. `tick` is a list of steps; a later
//! poll (`goto.json`) is one more step.

use std::io;
use std::time::{Duration, Instant, SystemTime};

use super::app::App;
use crate::comments::Log;
use crate::session::{COMMENTS_FILE, Fingerprint, Session, TABLE_FILE, VIEW_FILE, View};
use crate::table::Table;
use crate::time;

/// At most one `view.json` write per this much cursor movement.
const VIEW_DEBOUNCE: Duration = Duration::from_millis(100);

/// Not `Sync`: that's the standard library's thread-safety trait, in scope
/// everywhere, and a struct with its name would hide it.
pub struct SessionSync {
    session: Session,
    /// The position last written (or last tried), so a tick with no movement
    /// writes nothing. `start` writes, so there always is one.
    written: Position,
    /// When the cursor first moved away from `written`, if it has.
    moved_at: Option<Instant>,
    /// `table.json` as of the last look, so a tick only parses it when it
    /// changed. `None` before the first look, or if it couldn't be read.
    seen: Option<Fingerprint>,
    /// `comments.jsonl` as of the last look. `None` means no log, which is
    /// also what `App` starts with, so the first tick reads one only if it
    /// exists.
    comments_seen: Option<Fingerprint>,
}

/// What `view.json` records about the cursor: the row's ID and the column's
/// name, which survive a reload where indices don't.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Position {
    row_id: Option<String>,
    column: Option<String>,
}

impl Position {
    fn of(app: &App) -> Position {
        Position {
            row_id: app.row_id(),
            column: app.column_name(),
        }
    }

    /// `view.json`'s contents for this position, stamped now. `start`,
    /// `tick` and `finish` all build their `View` here.
    fn view(&self, pid: Option<u32>) -> View {
        View {
            pid,
            row_id: self.row_id.clone(),
            column: self.column.clone(),
            updated_at: time::rfc3339(SystemTime::now()),
        }
    }
}

impl SessionSync {
    /// Write `view.json` once, so the agent can see the viewer the moment it
    /// opens.
    pub fn start(session: Session, app: &mut App) -> SessionSync {
        let position = Position::of(app);
        let mut sync = SessionSync {
            session,
            written: position.clone(),
            moved_at: None,
            // Not the current fingerprint: `main` loaded the table before
            // this, and a push in between would be recorded as seen but never
            // shown (the wrong order; see `changed_table`). So the first tick
            // parses `table.json` again. Unchanged, it highlights nothing.
            seen: None,
            comments_seen: None,
        };
        sync.write(app, position);
        sync
    }

    /// Called once per event-loop iteration, before drawing. A reload can
    /// move the cursor (its row was deleted), so the polls come before the
    /// `view.json` write. Each step is its own method, so one returning early
    /// can't skip the next.
    pub fn tick(&mut self, app: &mut App, now: Instant) {
        self.write_pending(app);
        self.poll_table(app, now);
        self.poll_comments(app);
        self.write_view_if_moved(app, now);
    }

    /// Write the comment events from the keyboard since the last tick.
    /// `poll_comments` then sees the log grow and reads it back, so a new
    /// comment shows up on the same frame. A failed write loses what was
    /// typed, and says so; there's nowhere to keep it once the session is
    /// gone.
    fn write_pending(&mut self, app: &mut App) {
        if let Err(error) = self.session.append_events(&app.take_pending()) {
            app.set_message(format!("not saved: {}", file_error(COMMENTS_FILE, &error)));
        }
    }

    /// Reload `table.json` into `app` if it changed. A failure keeps the
    /// last table and says why in the status line until a reload succeeds.
    fn poll_table(&mut self, app: &mut App, now: Instant) {
        match self.changed_table() {
            Ok(None) => {}
            Ok(Some(table)) => app.set_table(table, now),
            Err(error) => app.reload_error = Some(file_error(TABLE_FILE, &error)),
        }
    }

    /// Load `comments.jsonl` into `app` if it changed, saying so if lines were
    /// skipped or it couldn't be read. A persistent error sets the message
    /// again every tick, so it stays up while the problem lasts. It isn't
    /// `reload_error`: that one belongs to `table.json`.
    fn poll_comments(&mut self, app: &mut App) {
        match self.changed_comments() {
            Ok(None) => {}
            Ok(Some(log)) => {
                if let Some(warning) = log.warning() {
                    app.set_message(format!("{COMMENTS_FILE}: {warning}"));
                }
                app.set_events(log.events);
            }
            Err(error) => app.set_message(file_error(COMMENTS_FILE, &error)),
        }
    }

    /// Write `view.json` if the cursor moved and the debounce has passed.
    fn write_view_if_moved(&mut self, app: &mut App, now: Instant) {
        let position = Position::of(app);
        if position == self.written {
            self.moved_at = None;
            return;
        }
        let moved_at = *self.moved_at.get_or_insert(now);
        if now.duration_since(moved_at) >= VIEW_DEBOUNCE {
            self.write(app, position);
            self.moved_at = None;
        }
    }

    /// How long the event loop may wait for a key before `tick` has something
    /// to do: `None` means "no deadline, use the normal tick".
    pub fn next_due(&self, now: Instant) -> Option<Duration> {
        // `saturating_sub`: subtracting a longer `Duration` panics.
        self.moved_at
            .map(|moved_at| VIEW_DEBOUNCE.saturating_sub(now.duration_since(moved_at)))
    }

    /// The final write on exit, without `pid`. A directory that's gone
    /// (`tv rm --force` under the viewer) is fine: there's nothing left to
    /// record. Any other error is returned.
    pub fn finish(&self, app: &App) -> anyhow::Result<()> {
        let result = self.session.write_view(&Position::of(app).view(None));
        if let Err(error) = &result
            && is_not_found(error)
        {
            return Ok(());
        }
        result
    }

    /// The table if `table.json` changed since the last look, `None` if not.
    /// A file that fails to parse is still recorded as seen, so it's parsed
    /// once, not every 250ms. A missing one (`tv rm --force`) is an error
    /// like any other: the viewer keeps the last table and keeps running.
    fn changed_table(&mut self) -> anyhow::Result<Option<Table>> {
        // Fingerprint first, then read. A write landing between the two costs
        // one extra reload next tick; the other order could record the new
        // fingerprint with the old contents and miss the change.
        // Forget the last fingerprint when the file can't be read, so whatever
        // comes back counts as new, even the same file with the same mtime
        // and size (`mv` away and back, `cp -p`): that clears the error.
        let fingerprint = self
            .session
            .table_fingerprint()
            .inspect_err(|_| self.seen = None)?;
        if Some(fingerprint) == self.seen {
            return Ok(None);
        }
        self.seen = Some(fingerprint);
        self.session.load_table().map(Some)
    }

    /// The log if `comments.jsonl` changed since the last look, `None` if
    /// not. No log is an empty one. A failed read isn't recorded as seen, so
    /// the next tick tries again: nothing in the log can fail to parse (bad
    /// lines are skipped), so an error here is IO, and it may clear up.
    ///
    /// The fingerprint is still read first. An append landing between the two
    /// is in the log but not in `comments_seen`, so the next tick reads the
    /// log again, which is harmless.
    fn changed_comments(&mut self) -> anyhow::Result<Option<Log>> {
        let fingerprint = self.session.comments_fingerprint()?;
        if fingerprint == self.comments_seen {
            return Ok(None);
        }
        let log = self.session.read_log()?;
        self.comments_seen = fingerprint;
        Ok(Some(log))
    }

    /// Write `view.json` for `position`. An error goes to the status line and
    /// isn't retried until the cursor moves again: the position is advisory,
    /// and a full disk shouldn't put up a message every 100ms.
    fn write(&mut self, app: &mut App, position: Position) {
        let view = position.view(Some(std::process::id()));
        if let Err(error) = self.session.write_view(&view) {
            app.set_message(file_error(VIEW_FILE, &error));
        }
        self.written = position;
    }
}

/// A status-line error about one of the session's files: `table.json: ` and
/// the underlying error, without the context chain. The chain starts with
/// the session's full path, which would push the reason off a narrow status
/// line, and the file name already says which file it was.
fn file_error(file: &str, error: &anyhow::Error) -> String {
    format!("{file}: {}", error.root_cause())
}

/// Whether `error` is a missing file or directory. `downcast_ref` looks
/// through anyhow's context to the `io::Error` underneath. For a write, it
/// means `write_atomic` couldn't create its temp file: the directory is gone.
fn is_not_found(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<io::Error>()
        .is_some_and(|error| error.kind() == io::ErrorKind::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comments::{self, Event};
    use crate::session::Sessions;
    use crate::table::Table;
    use crate::viewer::app::Cursor;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::fs::{self, File};
    use std::path::PathBuf;

    const MS_30: Duration = Duration::from_millis(30);
    const MS_150: Duration = Duration::from_millis(150);

    fn table() -> Table {
        Table::parse(r#"{"key": "id", "rows": [{"id": "a", "n": 1}, {"id": "b", "n": 2}]}"#)
            .unwrap()
    }

    /// A session in a temp dir and a viewer started on it. Keep the
    /// `TempDir`: dropping it deletes the directory.
    struct Fixture {
        dir: tempfile::TempDir,
        sessions: Sessions,
        id: String,
        view_path: PathBuf,
        app: App,
        sync: SessionSync,
    }

    fn start(table: Table) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sessions");
        let sessions = Sessions::new(root.clone());
        let session = sessions.create(&table).unwrap();
        let id = session.id().to_string();
        let view_path = root.join(&id).join(VIEW_FILE);
        let mut app = App::new(table);
        let sync = SessionSync::start(session, &mut app);
        Fixture {
            dir,
            sessions,
            id,
            view_path,
            app,
            sync,
        }
    }

    impl Fixture {
        fn view(&self) -> View {
            self.sync.session.read_view().unwrap().unwrap()
        }

        /// Backdate `view.json`, so `was_rewritten` can tell whether a later
        /// write replaced it. Comparing bytes can't: `updated_at` has whole
        /// seconds, so a rewrite within one second is byte-for-byte the same.
        fn backdate_view(&self) {
            File::options()
                .write(true)
                .open(&self.view_path)
                .unwrap()
                .set_modified(SystemTime::UNIX_EPOCH)
                .unwrap();
        }

        /// Whether `view.json` was replaced since `backdate_view`. Writes go
        /// through `write_atomic`, which renames a new file into place, so
        /// they get a fresh mtime.
        fn was_rewritten(&self) -> bool {
            fs::metadata(&self.view_path).unwrap().modified().unwrap() != SystemTime::UNIX_EPOCH
        }

        fn move_to_row(&mut self, row: usize) {
            self.app.cursor = Cursor { row, col: 0 };
        }
    }

    #[test]
    fn start_writes_the_pid_and_the_cursor_cell() {
        let fixture = start(table());
        let view = fixture.view();
        assert_eq!(view.pid, Some(std::process::id()));
        assert_eq!(view.row_id.as_deref(), Some("a"));
        assert_eq!(view.column.as_deref(), Some("id"));
    }

    #[test]
    fn a_move_is_written_once_the_debounce_has_passed() {
        let mut fixture = start(table());
        let now = Instant::now();
        fixture.backdate_view();
        fixture.move_to_row(1);

        fixture.sync.tick(&mut fixture.app, now);
        assert!(!fixture.was_rewritten());

        fixture.sync.tick(&mut fixture.app, now + VIEW_DEBOUNCE);
        assert!(fixture.was_rewritten());
        assert_eq!(fixture.view().row_id.as_deref(), Some("b"));

        fixture.backdate_view();
        fixture
            .sync
            .tick(&mut fixture.app, now + VIEW_DEBOUNCE + MS_30);
        assert!(!fixture.was_rewritten(), "no movement, no write");
    }

    #[test]
    fn moves_within_the_debounce_write_the_last_position() {
        let mut fixture = start(table());
        let now = Instant::now();
        fixture.move_to_row(1);
        fixture.sync.tick(&mut fixture.app, now);
        fixture.app.cursor = Cursor { row: 1, col: 1 };
        fixture.sync.tick(&mut fixture.app, now + MS_30);
        fixture.sync.tick(&mut fixture.app, now + VIEW_DEBOUNCE);
        let view = fixture.view();
        assert_eq!(view.row_id.as_deref(), Some("b"));
        assert_eq!(view.column.as_deref(), Some("n"));
    }

    #[test]
    fn moving_back_before_the_debounce_writes_nothing() {
        let mut fixture = start(table());
        let now = Instant::now();
        fixture.backdate_view();
        fixture.move_to_row(1);
        fixture.sync.tick(&mut fixture.app, now);
        fixture.move_to_row(0);
        fixture.sync.tick(&mut fixture.app, now + MS_30);
        fixture.sync.tick(&mut fixture.app, now + MS_150);
        assert!(!fixture.was_rewritten());
        assert_eq!(fixture.sync.next_due(now + MS_150), None);
    }

    #[test]
    fn next_due_counts_down_the_debounce() {
        let mut fixture = start(table());
        let now = Instant::now();
        assert_eq!(fixture.sync.next_due(now), None);
        fixture.move_to_row(1);
        fixture.sync.tick(&mut fixture.app, now);
        assert_eq!(fixture.sync.next_due(now), Some(VIEW_DEBOUNCE));
        let after_60ms = now + Duration::from_millis(60);
        assert_eq!(
            fixture.sync.next_due(after_60ms),
            Some(Duration::from_millis(40))
        );
        // Past the deadline without a tick: zero, not a panic.
        assert_eq!(fixture.sync.next_due(now + MS_150), Some(Duration::ZERO));
        fixture.sync.tick(&mut fixture.app, now + MS_150);
        assert_eq!(fixture.sync.next_due(now + MS_150), None);
    }

    #[test]
    fn an_empty_table_writes_no_cell() {
        let fixture = start(Table::parse("[]").unwrap());
        let view = fixture.view();
        assert_eq!((view.row_id, view.column), (None, None));
        assert_eq!(view.pid, Some(std::process::id()));
    }

    #[test]
    fn finish_keeps_the_position_and_drops_the_pid() {
        let mut fixture = start(table());
        fixture.move_to_row(1);
        fixture.sync.finish(&fixture.app).unwrap();
        let view = fixture.view();
        assert_eq!(view.pid, None);
        assert_eq!(view.row_id.as_deref(), Some("b"));
        assert!(!fixture.sync.session.viewer_open());
    }

    #[test]
    fn a_push_just_before_start_is_still_shown() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = Sessions::new(dir.path().join("sessions"));
        let session = sessions.create(&table()).unwrap();
        let mut app = App::new(session.load_table().unwrap()); // what `open` and `run` do
        let pushed =
            Table::parse(r#"{"key": "id", "rows": [{"id": "a", "n": 10}, {"id": "b", "n": 2}]}"#)
                .unwrap();
        session.write_table(&pushed).unwrap(); // the agent's push
        let mut sync = SessionSync::start(session, &mut app);
        sync.tick(&mut app, Instant::now());
        assert_eq!(app.table().rows, pushed.rows);
    }

    #[test]
    fn a_tick_right_after_start_highlights_nothing() {
        let mut fixture = start(table());
        fixture.sync.tick(&mut fixture.app, Instant::now());
        assert!(!fixture.app.is_highlighted(0, 0));
        assert_eq!(fixture.app.reload_error, None);
    }

    #[test]
    fn a_new_table_json_is_reloaded_and_highlighted() {
        let mut fixture = start(table());
        let changed =
            Table::parse(r#"{"key": "id", "rows": [{"id": "a", "n": 10}, {"id": "b", "n": 2}]}"#)
                .unwrap();
        fixture.sync.session.write_table(&changed).unwrap();
        fixture.sync.tick(&mut fixture.app, Instant::now());
        assert_eq!(fixture.app.table().rows, changed.rows);
        assert!(fixture.app.is_highlighted(0, 1));
        assert!(!fixture.app.is_highlighted(1, 1));
    }

    #[test]
    fn a_broken_table_json_keeps_the_table_and_is_parsed_once() {
        let mut fixture = start(table());
        let table_path = fixture.view_path.with_file_name(TABLE_FILE);
        fs::write(&table_path, "not json").unwrap();
        fixture.sync.tick(&mut fixture.app, Instant::now());
        assert_eq!(fixture.app.table().rows, table().rows);
        assert_eq!(
            fixture.app.reload_error.as_deref(),
            Some("table.json: expected ident at line 1 column 2")
        );
        // Recorded as seen, which is what stops a re-parse every tick.
        let current = fixture.sync.session.table_fingerprint().unwrap();
        assert_eq!(fixture.sync.seen, Some(current));

        fixture.sync.session.write_table(&table()).unwrap();
        fixture.sync.tick(&mut fixture.app, Instant::now());
        assert_eq!(fixture.app.reload_error, None);
    }

    #[test]
    fn the_same_file_coming_back_clears_the_error() {
        let mut fixture = start(table());
        fixture.sync.tick(&mut fixture.app, Instant::now());
        // Away and back with `rename`, which keeps the mtime and size, so
        // the fingerprint is the one already seen.
        let table_path = fixture.view_path.with_file_name(TABLE_FILE);
        let moved = fixture.dir.path().join("moved.json");
        fs::rename(&table_path, &moved).unwrap();
        fixture.sync.tick(&mut fixture.app, Instant::now());
        assert!(fixture.app.reload_error.is_some());
        fs::rename(&moved, &table_path).unwrap();
        fixture.sync.tick(&mut fixture.app, Instant::now());
        assert_eq!(fixture.app.reload_error, None);
    }

    #[test]
    fn two_writes_in_one_second_are_both_noticed() {
        let mut fixture = start(table());
        for rows in [3, 4] {
            let json: Vec<String> = (0..rows)
                .map(|row| format!(r#"{{"id": "r{row}", "n": {row}}}"#))
                .collect();
            let table = format!(r#"{{"key": "id", "rows": [{}]}}"#, json.join(", "));
            fixture
                .sync
                .session
                .write_table(&Table::parse(&table).unwrap())
                .unwrap();
            fixture.sync.tick(&mut fixture.app, Instant::now());
            assert_eq!(fixture.app.table().rows.len(), rows);
        }
    }

    #[test]
    fn a_removed_session_keeps_the_table_and_says_so() {
        let mut fixture = start(table());
        let other = fixture.sessions.open(&fixture.id).unwrap();
        fixture.sessions.remove(other).unwrap();
        fixture.sync.tick(&mut fixture.app, Instant::now());
        assert_eq!(fixture.app.table().rows, table().rows);
        let error = fixture.app.reload_error.as_deref().unwrap();
        assert!(error.starts_with("table.json: No such file"), "{error}");
    }

    #[test]
    fn finish_after_the_session_is_removed_is_fine() {
        let mut fixture = start(table());
        // Remove it the way `tv rm --force` does under an open viewer: a
        // second handle on the same directory, through the one delete path.
        let other = fixture.sessions.open(&fixture.id).unwrap();
        fixture.sessions.remove(other).unwrap();
        fixture.sync.finish(&fixture.app).unwrap();

        // A move then fails to write, and says so in the status line.
        fixture.move_to_row(1);
        let now = Instant::now();
        fixture.sync.tick(&mut fixture.app, now);
        fixture.sync.tick(&mut fixture.app, now + VIEW_DEBOUNCE);
        let message = fixture.app.message.as_ref().unwrap();
        assert!(message.text.starts_with("view.json: "), "{}", message.text);
        assert!(!message.text.contains('/'), "no path: {}", message.text);
    }

    /// One comment on row `a`'s `n`, the way the agent would append it.
    fn agent_comment(id: &str) -> Event {
        comments::parse_log(&format!(
            r#"{{"op":"add","id":"{id}","author":"agent","row_id":"a","column":"n","text":"look","ts":"2026-10-28T19:03:00Z"}}"#
        ))
        .events
        .remove(0)
    }

    impl Fixture {
        /// The session through a second handle: the agent's side.
        fn other_side(&self) -> Session {
            self.sessions.open(&self.id).unwrap()
        }

        fn comments_path(&self) -> PathBuf {
            self.view_path.with_file_name(COMMENTS_FILE)
        }

        fn message(&self) -> Option<&str> {
            self.app
                .message
                .as_ref()
                .map(|message| message.text.as_str())
        }
    }

    #[test]
    fn with_no_log_a_tick_reads_nothing() {
        let mut fixture = start(table());
        fixture.sync.tick(&mut fixture.app, Instant::now());
        assert!(
            fixture.app.threads().is_empty(),
            "{:?}",
            fixture.app.threads()
        );
        assert_eq!(fixture.message(), None);
    }

    #[test]
    fn the_agents_comments_show_up_on_the_next_tick() {
        let mut fixture = start(table());
        let now = Instant::now();
        // No cursor movement: `write_view_if_moved` returns early, and the
        // poll must run anyway.
        fixture.sync.tick(&mut fixture.app, now);
        fixture
            .other_side()
            .append_events(&[agent_comment("c1")])
            .unwrap();
        fixture.sync.tick(&mut fixture.app, now + MS_30);
        assert_eq!(fixture.app.threads().len(), 1);
        assert_eq!(fixture.app.threads()[0].id, "c1");

        fixture
            .other_side()
            .append_events(&[agent_comment("c2")])
            .unwrap();
        fixture.sync.tick(&mut fixture.app, now + MS_150);
        assert_eq!(fixture.app.threads().len(), 2);
    }

    #[test]
    fn skipped_log_lines_are_reported() {
        let mut fixture = start(table());
        let good = comments::log_text(&[agent_comment("c1"), agent_comment("c2")]).unwrap();
        let (first, second) = good.split_once('\n').unwrap();
        fs::write(
            fixture.comments_path(),
            format!("{first}\nnot json\n{second}"),
        )
        .unwrap();
        fixture.sync.tick(&mut fixture.app, Instant::now());
        assert_eq!(fixture.app.threads().len(), 2);
        assert_eq!(
            fixture.message(),
            Some("comments.jsonl: skipped lines that aren't events: 2")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_log_that_cant_be_read_is_retried_every_tick() {
        let mut fixture = start(table());
        // A directory stats fine, but reading it fails with `EISDIR`.
        fs::create_dir(fixture.comments_path()).unwrap();
        let expected = Some("comments.jsonl: Is a directory (os error 21)");
        fixture.sync.tick(&mut fixture.app, Instant::now());
        assert_eq!(fixture.message(), expected);
        fixture.app.message = None;
        fixture.sync.tick(&mut fixture.app, Instant::now());
        assert_eq!(fixture.message(), expected, "retried");

        // Moved out of the session (not deleted), and a real log in its place.
        fs::rename(fixture.comments_path(), fixture.dir.path().join("moved")).unwrap();
        fixture
            .other_side()
            .append_events(&[agent_comment("c1")])
            .unwrap();
        fixture.app.message = None;
        fixture.sync.tick(&mut fixture.app, Instant::now());
        assert_eq!(fixture.app.threads().len(), 1);
        assert_eq!(fixture.message(), None);
    }

    /// `c`, `hi`, Enter: a comment on the cursor cell (row `a`, column `id`).
    fn type_comment(app: &mut App) {
        for code in [
            KeyCode::Char('c'),
            KeyCode::Char('h'),
            KeyCode::Char('i'),
            KeyCode::Enter,
        ] {
            app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
        }
    }

    #[test]
    fn a_typed_comment_is_written_and_read_back_on_one_tick() {
        let mut fixture = start(table());
        type_comment(&mut fixture.app);
        fixture.sync.tick(&mut fixture.app, Instant::now());
        let log = fixture.other_side().read_log().unwrap();
        assert_eq!(log.events.len(), 1);
        assert_eq!(fixture.app.threads().len(), 1);
        assert_eq!(fixture.app.threads()[0].messages[0].text, "hi");
        assert!(fixture.app.marker(0, 0).is_some());
    }

    #[test]
    fn a_comment_on_a_removed_session_says_it_wasnt_saved() {
        let mut fixture = start(table());
        let dir = fixture.view_path.parent().unwrap().to_path_buf();
        fixture.sessions.remove(fixture.other_side()).unwrap();
        type_comment(&mut fixture.app);
        fixture.sync.tick(&mut fixture.app, Instant::now());
        let message = fixture.message().unwrap();
        assert!(
            message.starts_with("not saved: comments.jsonl: No such file"),
            "{message}"
        );
        assert!(!dir.exists());
    }
}
