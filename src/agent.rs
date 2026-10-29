//! `tv agent`: the commands an agent runs. Each one prints one line of JSON
//! on stdout. Errors go to stderr through `main`, with exit status 1.

use std::io::{self, IsTerminal};
use std::path::Path;

use anyhow::{Context, bail};
use clap::{Args, Subcommand};
use serde::Serialize;

use crate::session::{Session, Sessions};
use crate::table::{Meta, Table};
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
    /// List sessions, most recently modified first.
    Sessions,
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
        AgentCommand::New(input) => serde_json::to_string(&new(sessions, input)?)?,
        AgentCommand::Push { id, input } => serde_json::to_string(&push(sessions, id, input)?)?,
        AgentCommand::Get { id } => serde_json::to_string(&sessions.open(id)?.load_table()?)?,
        AgentCommand::Sessions => serde_json::to_string(&list_sessions(sessions)?)?,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_command_sets_tv_home_when_it_was_set() {
        assert_eq!(open_command("k3f9x2", None), "tv open k3f9x2");
        assert_eq!(
            open_command("k3f9x2", Some(Path::new("/tmp/it's here"))),
            r"TV_HOME='/tmp/it'\''s here' tv open k3f9x2"
        );
    }
}
