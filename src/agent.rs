//! `tv agent`: the commands an agent runs. Each one prints one line of JSON
//! on stdout, except `help`. Errors go to stderr through `main`, with exit
//! status 1.

use std::io::{self, IsTerminal};
use std::path::Path;

use anyhow::{Context, bail};
use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::Value;

use crate::comments::{self, Add, Author, Event, Log, Message, Thread};
use crate::session::{COMMENTS_FILE, Goto, Session, Sessions};
use crate::skills;
use crate::table::{Meta, Row, Table};
use crate::time;

#[derive(Subcommand)]
pub enum AgentCommand {
    /// Create a session from FILE, or from JSON on stdin.
    New(TableInput),
    /// Replace a session's table. Same input as `new`.
    Push {
        id: String,
        #[command(flatten)]
        input: TableInput,
    },
    /// Print a session's table, without comments.
    Get { id: String },
    /// What the user's cursor is on.
    Focus { id: String },
    /// Comment threads awaiting your reply. `--all` for every thread.
    Comments {
        id: String,
        #[arg(long)]
        all: bool,
    },
    /// Reply to a thread.
    Reply {
        id: String,
        thread: String,
        #[arg(allow_hyphen_values = true)]
        text: String,
    },
    /// Start a thread on a cell, to point the user at it.
    Comment {
        id: String,
        #[arg(long)]
        row: String,
        #[arg(long)]
        col: String,
        #[arg(allow_hyphen_values = true)]
        text: String,
    },
    /// List sessions, most recently modified first.
    Sessions,
    /// Move the user's cursor to a cell.
    Goto {
        id: String,
        #[arg(long)]
        row: String,
        /// Without it, the cursor keeps its column.
        #[arg(long)]
        col: Option<String>,
    },
    /// Print the guide: when to use tv, and how. Plain text, not JSON.
    Help,
}

/// The table `new` and `push` read, and what to set on it.
#[derive(Args)]
pub struct TableInput {
    /// A JSON file: the table format, or a bare array of rows. Stdin without
    /// one, or with `-`.
    file: Option<String>,
    /// Set the title.
    #[arg(long)]
    title: Option<String>,
    /// Set the key column. Rows need unique, non-null values in it.
    #[arg(long)]
    key: Option<String>,
}

impl TableInput {
    /// The table from the file or stdin, with `--title` and `--key` applied
    /// before it's validated, and `fill`'s title and key where the input
    /// names neither.
    fn read(&self, fill: Meta) -> anyhow::Result<Table> {
        let set = Meta {
            title: self.title.as_deref(),
            key: self.key.as_deref(),
        };
        match self.file.as_deref() {
            None | Some("-") => {
                Table::parse_with(&read_stdin()?, set, fill).context("parsing stdin")
            }
            Some(path) => Table::from_path_with(path, set, fill),
        }
    }
}

/// Stdin, unless it's a terminal: an agent that forgot the pipe would
/// otherwise wait forever for input nobody types.
fn read_stdin() -> anyhow::Result<String> {
    let stdin = io::stdin();
    if stdin.is_terminal() {
        bail!("no table: give a FILE, or pipe JSON to stdin");
    }
    io::read_to_string(stdin).context("reading stdin")
}

/// Run `command` and print its result.
pub fn run(sessions: &Sessions, command: &AgentCommand) -> anyhow::Result<()> {
    // Each arm serializes its own result: `Serialize` can't be a trait
    // object, so the arms can't return one shared type.
    let json = match command {
        // The one command that isn't JSON: the guide is for reading.
        AgentCommand::Help => {
            print!("{}", skills::guide());
            return Ok(());
        }
        AgentCommand::New(input) => serde_json::to_string(&new(sessions, input)?)?,
        AgentCommand::Push { id, input } => serde_json::to_string(&push(sessions, id, input)?)?,
        AgentCommand::Get { id } => serde_json::to_string(&sessions.open(id)?.load_table()?)?,
        AgentCommand::Focus { id } => serde_json::to_string(&focus(sessions, id)?)?,
        AgentCommand::Comments { id, all } => {
            serde_json::to_string(&list_comments(sessions, id, *all)?)?
        }
        AgentCommand::Reply { id, thread, text } => {
            serde_json::to_string(&reply(sessions, id, thread, text)?)?
        }
        AgentCommand::Comment { id, row, col, text } => {
            serde_json::to_string(&comment(sessions, id, row, col, text)?)?
        }
        AgentCommand::Sessions => serde_json::to_string(&list_sessions(sessions)?)?,
        AgentCommand::Goto { id, row, col } => {
            serde_json::to_string(&goto(sessions, id, row, col.as_deref())?)?
        }
    };
    println!("{json}");
    Ok(())
}

/// What `new` and `push` print.
#[derive(Debug, Serialize)]
pub struct Created {
    session: String,
    open_command: String,
    rows: usize,
    columns: usize,
}

impl Created {
    fn of(sessions: &Sessions, session: &Session, table: &Table) -> Created {
        Created {
            session: session.id().to_string(),
            open_command: open_command(session.id(), sessions.tv_home()),
            rows: table.rows.len(),
            columns: table.columns.len(),
        }
    }
}

/// The command that opens session `id`, for the agent to give the user.
/// With `TV_HOME` set, the command sets it too: the user's shell may not
/// have it, and `tv open` would then look in `~/.tv`.
fn open_command(id: &str, tv_home: Option<&Path>) -> String {
    match tv_home {
        None => format!("tv open {id}"),
        Some(home) => format!(
            "TV_HOME={} tv open {id}",
            shell_quote(&home.to_string_lossy())
        ),
    }
}

/// `text` in single quotes, safe to paste into a POSIX shell. A `'` inside
/// ends the quote, adds an escaped `'`, and starts a new quote.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn new(sessions: &Sessions, input: &TableInput) -> anyhow::Result<Created> {
    let table = input.read(Meta::default())?;
    let session = sessions.create(&table)?;
    Ok(Created::of(sessions, &session, &table))
}

fn push(sessions: &Sessions, id: &str, input: &TableInput) -> anyhow::Result<Created> {
    let session = sessions.open(id)?;
    // Input that names no title or key keeps the session's: dropping the key
    // would orphan every comment. A `table.json` that can't be read has
    // nothing to keep, and replacing it is one reason to push.
    let current = session.load_table().ok();
    let keep = current.as_ref().map_or_else(Meta::default, |table| Meta {
        title: table.title.as_deref(),
        key: table.key.as_deref(),
    });
    let table = input.read(keep)?;
    if !table.comments.is_empty() {
        bail!(
            "push doesn't change comments: take `comments` out of the table, \
             or start a new session from it with `tv agent new`"
        );
    }
    session.write_table(&table)?;
    Ok(Created::of(sessions, &session, &table))
}

/// One session in `tv agent sessions`. What comes from `table.json` is `null`
/// when it can't be read, where `tv ls` shows `?`.
#[derive(Debug, Serialize)]
pub struct Listed {
    session: String,
    title: Option<String>,
    rows: Option<usize>,
    columns: Option<usize>,
    viewer_open: bool,
    awaiting_reply: Option<usize>,
    modified_at: Option<String>,
}

fn list_sessions(sessions: &Sessions) -> anyhow::Result<Vec<Listed>> {
    let listed = sessions
        .list()?
        .iter()
        .map(|session| {
            let summary = session.summary().ok();
            Listed {
                session: session.id().to_string(),
                title: summary.as_ref().and_then(|summary| summary.title.clone()),
                rows: summary.as_ref().map(|summary| summary.rows),
                columns: summary.as_ref().map(|summary| summary.columns),
                viewer_open: session.viewer_open(),
                awaiting_reply: summary.as_ref().map(|summary| summary.awaiting_reply),
                modified_at: session.modified_at().ok().map(time::rfc3339),
            }
        })
        .collect();
    Ok(listed)
}

/// What `focus` and `goto` print: the cell the user's cursor is on, or for
/// `goto` is being sent to, with its value and its row.
#[derive(Debug, PartialEq, Serialize)]
pub struct Focus {
    viewer_open: bool,
    row_id: Option<String>,
    column: Option<String>,
    /// `null` for an empty cell, or when the row or column isn't in the table.
    value: Value,
    /// `null` when the row isn't in the table: the viewer was last on a row
    /// that a push has since removed.
    row: Option<Row>,
}

impl Focus {
    fn at(
        table: &Table,
        viewer_open: bool,
        row_id: Option<String>,
        column: Option<String>,
    ) -> Focus {
        let index = row_id.as_deref().and_then(|row_id| table.row_index(row_id));
        let value = index
            .zip(column.as_deref())
            .and_then(|(index, column)| table.cell(index, column))
            .cloned()
            .unwrap_or_default();
        Focus {
            viewer_open,
            row: index.map(|index| table.rows[index].clone()),
            row_id,
            column,
            value,
        }
    }
}

fn focus(sessions: &Sessions, id: &str) -> anyhow::Result<Focus> {
    let session = sessions.open(id)?;
    let table = session.load_table()?;
    let (row_id, column) = session
        .read_view()?
        .map_or((None, None), |view| (view.row_id, view.column));
    Ok(Focus::at(&table, session.viewer_open(), row_id, column))
}

/// The session's comment log. Lines that were skipped are reported on
/// stderr, since stdout is the JSON.
fn read_log_and_warn(session: &Session) -> anyhow::Result<Log> {
    let log = session.read_log()?;
    if let Some(warning) = log.warning() {
        eprintln!("{COMMENTS_FILE}: {warning}");
    }
    Ok(log)
}

fn list_comments(sessions: &Sessions, id: &str, all: bool) -> anyhow::Result<Vec<Thread>> {
    let session = sessions.open(id)?;
    let table = session.load_table()?;
    let log = read_log_and_warn(&session)?;
    let threads = comments::threads(&log.events, &table);
    Ok(threads
        .into_iter()
        .filter(|thread| all || thread.awaiting_reply)
        .collect())
}

/// `text` without surrounding whitespace, or an error if that leaves nothing.
fn comment_text(text: &str) -> anyhow::Result<String> {
    let text = text.trim();
    if text.is_empty() {
        bail!("the comment is empty");
    }
    Ok(text.to_string())
}

fn reply(sessions: &Sessions, id: &str, thread: &str, text: &str) -> anyhow::Result<Message> {
    let text = comment_text(text)?;
    let session = sessions.open(id)?;
    let table = session.load_table()?;
    let log = read_log_and_warn(&session)?;
    // `threads` would drop a reply to anything else without a word.
    if !comments::threads(&log.events, &table)
        .iter()
        .any(|candidate| candidate.id == thread)
    {
        bail!("no thread \"{thread}\": `tv agent comments {id} --all` lists them");
    }
    let add = Add::reply(
        comments::new_id(&log.events),
        Author::Agent,
        thread.to_string(),
        text,
    );
    write_comment(&session, add)
}

fn comment(
    sessions: &Sessions,
    id: &str,
    row_id: &str,
    column: &str,
    text: &str,
) -> anyhow::Result<Message> {
    let text = comment_text(text)?;
    let session = sessions.open(id)?;
    let table = session.load_table()?;
    check_cell(&table, row_id, Some(column))?;
    let log = read_log_and_warn(&session)?;
    let add = Add::comment(
        comments::new_id(&log.events),
        Author::Agent,
        row_id.to_string(),
        column.to_string(),
        text,
    );
    write_comment(&session, add)
}

/// Append `add` to the session's log, and return it as the message the user
/// will see.
fn write_comment(session: &Session, add: Add) -> anyhow::Result<Message> {
    let message = Message::from(&add);
    session.append_events(&[Event::Add(add)])?;
    Ok(message)
}

/// An error unless `table` has row `row_id` and, if one is given, column
/// `column`. `comment` and `goto` check before they write anything.
fn check_cell(table: &Table, row_id: &str, column: Option<&str>) -> anyhow::Result<()> {
    if table.row_index(row_id).is_none() {
        let hint = match &table.key {
            Some(key) => format!("row IDs are the values of the key column \"{key}\""),
            None => "the table has no key, so row IDs are #0, #1, …".to_string(),
        };
        bail!("no row \"{row_id}\": {hint}");
    }
    if let Some(column) = column
        && table.column_index(column).is_none()
    {
        bail!("no column \"{column}\"");
    }
    Ok(())
}

fn goto(
    sessions: &Sessions,
    id: &str,
    row_id: &str,
    column: Option<&str>,
) -> anyhow::Result<Focus> {
    let session = sessions.open(id)?;
    let table = session.load_table()?;
    check_cell(&table, row_id, column)?;
    let viewer_open = session.viewer_open();
    // Without `--col` the cursor keeps its column: the open viewer's, which
    // `view.json` names, or the one a new viewer starts on. A closed viewer's
    // `view.json` doesn't say, since a new viewer doesn't restore it.
    let focus_column = match column {
        Some(column) => Some(column.to_string()),
        None if viewer_open => session.read_view()?.and_then(|view| view.column),
        None => table
            .columns
            .get(table.start_column())
            .map(|column| column.name.clone()),
    };
    session.write_goto(&Goto {
        row_id: row_id.to_string(),
        column: column.map(str::to_string),
    })?;
    Ok(Focus::at(
        &table,
        viewer_open,
        Some(row_id.to_string()),
        focus_column,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn prs() -> Table {
        Table::from_path("examples/prs.json").unwrap()
    }

    fn at(row_id: Option<&str>, column: Option<&str>) -> Focus {
        Focus::at(
            &prs(),
            true,
            row_id.map(str::to_string),
            column.map(str::to_string),
        )
    }

    #[test]
    fn focus_gives_the_cell_and_its_row() {
        let focus = at(Some("404"), Some("title"));
        assert_eq!(focus.value, json!("Bump serde to 1.0.229"));
        assert_eq!(focus.row.unwrap()["number"], 404);
    }

    #[test]
    fn focus_on_a_row_thats_gone_keeps_the_ids() {
        let focus = at(Some("412"), Some("title"));
        assert_eq!(focus.row_id.as_deref(), Some("412"));
        assert_eq!((focus.value, focus.row), (Value::Null, None));
    }

    #[test]
    fn focus_on_a_column_thats_gone_still_has_the_row() {
        let focus = at(Some("404"), Some("nope"));
        assert_eq!(focus.value, Value::Null);
        assert!(focus.row.is_some());
    }

    #[test]
    fn focus_with_no_position_is_all_nulls() {
        let focus = at(None, None);
        assert_eq!(
            serde_json::to_value(&focus).unwrap(),
            json!({"viewer_open": true, "row_id": null, "column": null, "value": null, "row": null})
        );
    }

    #[test]
    fn check_cell_names_what_row_ids_are() {
        let error = check_cell(&prs(), "1", None).unwrap_err().to_string();
        assert_eq!(
            error,
            "no row \"1\": row IDs are the values of the key column \"number\""
        );
        let unkeyed = Table::parse(r#"[{"a": 1}]"#).unwrap();
        let error = check_cell(&unkeyed, "0", None).unwrap_err().to_string();
        assert!(error.ends_with("row IDs are #0, #1, …"), "{error}");
        assert!(check_cell(&unkeyed, "#0", Some("a")).is_ok());
        let error = check_cell(&unkeyed, "#0", Some("b"))
            .unwrap_err()
            .to_string();
        assert_eq!(error, "no column \"b\"");
    }

    #[test]
    fn comment_text_trims_and_refuses_blank() {
        assert_eq!(comment_text("  hi\n").unwrap(), "hi");
        assert!(comment_text(" \n ").is_err());
    }

    #[test]
    fn open_command_sets_tv_home_when_it_was_set() {
        assert_eq!(open_command("k3f9x2", None), "tv open k3f9x2");
        assert_eq!(
            open_command("k3f9x2", Some(Path::new("/tmp/it's here"))),
            r"TV_HOME='/tmp/it'\''s here' tv open k3f9x2"
        );
    }
}
