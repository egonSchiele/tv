use anyhow::Context;
use clap::{Parser, Subcommand};

mod files;
mod session;
#[cfg(test)]
mod snapshot;
mod table;
mod viewer;

use session::Sessions;
use table::Table;

/// A terminal table viewer you share with an agent.
// `derive(Parser)` generates the argument parser from this struct's fields and
// doc comments, so `tv --help` text comes from the `///` comments.
#[derive(Parser)]
// The optional positional and the optional subcommand let `tv x.json` and
// `tv open x.json` both work: a subcommand name wins, and anything else is the
// target. `args_conflicts_with_subcommands` makes `tv x.json open y.json` an
// error instead of silently accepting both.
#[command(name = "tv", version, args_conflicts_with_subcommands = true)]
struct Cli {
    /// A JSON file to open (`.` or `/` in the name), `-` to read stdin, or a
    /// session ID. With none, the most recent session. Same as `tv open`.
    target: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Open a file (`.` or `/` in the name), `-` for stdin, or a session ID.
    /// With no argument, the most recent session.
    Open { target: Option<String> },
}

impl Cli {
    /// What to open, from `tv <target>` or `tv open <target>`. This is the
    /// only place that knows there are two spellings.
    fn argument(&self) -> Option<&str> {
        match &self.command {
            Some(Command::Open { target }) => target.as_deref(),
            None => self.target.as_deref(),
        }
    }
}

/// What the positional argument means. The SPEC's rule: a file if it contains
/// `.` or `/`, `-` for stdin, otherwise a session ID. No argument opens the
/// most recent session.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    MostRecent,
    Stdin,
    File(String),
    Session(String),
}

impl Target {
    fn parse(argument: Option<&str>) -> Target {
        match argument {
            None => Target::MostRecent,
            Some("-") => Target::Stdin,
            Some(text) if text.contains(['.', '/']) => Target::File(text.to_string()),
            Some(id) => Target::Session(id.to_string()),
        }
    }
}

/// Load a table from a file, or from stdin when `source` is `-` (the usual
/// command-line convention, so it lives here rather than in `table.rs`).
fn load(source: &str) -> anyhow::Result<Table> {
    if source == "-" {
        let text = std::io::read_to_string(std::io::stdin()).context("reading stdin")?;
        // The viewer reads keys from stdin. Now that the pipe is drained, point
        // stdin at the terminal: stdout, which must be the terminal for the
        // viewer to draw at all. crossterm would otherwise fall back to
        // `/dev/tty`, which its reader can't watch on macOS (kqueue doesn't
        // support that device), and fail with "Failed to initialize input reader".
        #[cfg(unix)]
        rustix::stdio::dup2_stdin(std::io::stdout()).context("attaching stdin to the terminal")?;
        return Table::parse(&text).context("parsing stdin");
    }
    Table::from_path(source)
}

/// Open the viewer on `target`. A file or stdin becomes a new session first.
fn open(sessions: &Sessions, target: Target) -> anyhow::Result<()> {
    // Load before creating, so a bad file errors out before the screen
    // changes and leaves no session behind.
    let session = match target {
        Target::Stdin => sessions.create(&load("-")?)?,
        Target::File(path) => sessions.create(&load(&path)?)?,
        Target::Session(id) => sessions.open(&id)?,
        Target::MostRecent => sessions
            .most_recent()?
            .context("no sessions yet; open a file with `tv <file.json>`")?,
    };
    // For a new session this parses the same bytes again. It's cheap, and
    // it means every case ends the same way.
    let table = session.load_table()?;
    viewer::run(table, session.id().to_string())
}

// `anyhow::Result` lets any error type bubble up with `?`. Returning `Err` from
// `main` prints `Error: …` to stderr and exits with status 1.
fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let sessions = Sessions::from_home(std::env::var_os("TV_HOME"))?;
    open(&sessions, Target::parse(cli.argument()))
}

// `#[cfg(test)]` compiles this module only for `cargo test`.
#[cfg(test)]
mod tests {
    use super::{Cli, Target};
    use clap::{CommandFactory, Parser};

    fn parsed_target(args: &[&str]) -> Target {
        Target::parse(Cli::try_parse_from(args).unwrap().argument())
    }

    fn file(path: &str) -> Target {
        Target::File(path.to_string())
    }

    fn session(id: &str) -> Target {
        Target::Session(id.to_string())
    }

    #[test]
    fn cli_definition_is_valid() {
        // Catches clap misconfiguration (duplicate flags, bad defaults) at test time.
        Cli::command().debug_assert();
    }

    #[test]
    fn target_follows_the_file_or_session_rule() {
        assert_eq!(Target::parse(None), Target::MostRecent);
        assert_eq!(Target::parse(Some("-")), Target::Stdin);
        for path in ["x.json", "./x", "dir/x", "~/x"] {
            assert_eq!(Target::parse(Some(path)), file(path));
        }
        assert_eq!(Target::parse(Some("k3f9x2")), session("k3f9x2"));
        assert_eq!(Target::parse(Some("abc")), session("abc"));
    }

    #[test]
    fn a_target_works_with_or_without_open() {
        assert_eq!(parsed_target(&["tv", "x.json"]), file("x.json"));
        assert_eq!(parsed_target(&["tv", "open", "x.json"]), file("x.json"));
        assert_eq!(parsed_target(&["tv", "-"]), Target::Stdin);
        assert_eq!(parsed_target(&["tv", "open", "-"]), Target::Stdin);
        assert_eq!(parsed_target(&["tv", "k3f9x2"]), session("k3f9x2"));
        assert_eq!(parsed_target(&["tv", "open", "k3f9x2"]), session("k3f9x2"));
    }

    #[test]
    fn no_target_means_the_most_recent_session() {
        assert_eq!(parsed_target(&["tv"]), Target::MostRecent);
        assert_eq!(parsed_target(&["tv", "open"]), Target::MostRecent);
    }
}
