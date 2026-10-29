use anyhow::Context;
use clap::{Parser, Subcommand};

#[cfg(test)]
mod snapshot;
mod table;
mod viewer;

use table::Table;

/// A terminal table viewer you share with an agent.
// `derive(Parser)` generates the argument parser from this struct's fields and
// doc comments, so `tv --help` text comes from the `///` comments.
#[derive(Parser)]
// `args_conflicts_with_subcommands` lets `tv x.json` and `tv open x.json` both
// work: a subcommand name wins, and anything else is the path.
#[command(
    name = "tv",
    version,
    args_conflicts_with_subcommands = true,
    arg_required_else_help = true
)]
struct Cli {
    /// A JSON file to open, or `-` to read stdin. Same as `tv open <path>`.
    path: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Open a table from a JSON file, or `-` for stdin (sessions come later).
    Open { path: String },
}

impl Cli {
    /// The path to open, whichever form was used.
    fn path(self) -> String {
        match self.command {
            Some(Command::Open { path }) => path,
            // `arg_required_else_help` means clap prints help and exits
            // before we get here with neither.
            None => self.path.expect("clap requires a path or a subcommand"),
        }
    }
}

/// Load a table from a file, or from stdin when `source` is `-` (the usual
/// command-line convention, so it lives here rather than in `table.rs`).
fn load(source: &str) -> anyhow::Result<Table> {
    if source == "-" {
        let text = std::io::read_to_string(std::io::stdin()).context("reading stdin")?;
        return Table::parse(&text).context("parsing stdin");
    }
    Table::from_path(source)
}

// `anyhow::Result` lets any error type bubble up with `?`. Returning `Err` from
// `main` prints `Error: …` to stderr and exits with status 1.
fn main() -> anyhow::Result<()> {
    let path = Cli::parse().path();
    // Load first, so a bad file errors out before the screen changes. When
    // stdin is a pipe, the viewer reads keys from the terminal (`/dev/tty`).
    let table = load(&path)?;
    viewer::run(table)
}

// `#[cfg(test)]` compiles this module only for `cargo test`.
#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::error::ErrorKind;
    use clap::{CommandFactory, Parser};

    fn parsed_path(args: &[&str]) -> String {
        Cli::try_parse_from(args).unwrap().path()
    }

    #[test]
    fn cli_definition_is_valid() {
        // Catches clap misconfiguration (duplicate flags, bad defaults) at test time.
        Cli::command().debug_assert();
    }

    #[test]
    fn a_path_works_with_or_without_open() {
        assert_eq!(parsed_path(&["tv", "x.json"]), "x.json");
        assert_eq!(parsed_path(&["tv", "open", "x.json"]), "x.json");
        assert_eq!(parsed_path(&["tv", "-"]), "-");
        assert_eq!(parsed_path(&["tv", "open", "-"]), "-");
    }

    #[test]
    fn bare_tv_shows_help() {
        let error = Cli::try_parse_from(["tv"]).err().unwrap();
        assert_eq!(
            error.kind(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }

    #[test]
    fn open_needs_a_path() {
        let error = Cli::try_parse_from(["tv", "open"]).err().unwrap();
        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
    }
}
