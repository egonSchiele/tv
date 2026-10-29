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
#[command(name = "tv", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Open a table from a JSON file (sessions come later).
    Open { path: String },
}

// `anyhow::Result` lets any error type bubble up with `?`. Returning `Err` from
// `main` prints `Error: …` to stderr and exits with status 1.
fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Open { path } => {
            // Parse first, so a bad file errors out before the screen changes.
            let table = Table::from_path(&path)?;
            viewer::run(table)?;
        }
    }
    Ok(())
}

// `#[cfg(test)]` compiles this module only for `cargo test`.
#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        // Catches clap misconfiguration (duplicate flags, bad defaults) at test time.
        Cli::command().debug_assert();
    }
}
