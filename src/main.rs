use clap::{Parser, Subcommand};

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

// Placeholder. Once opening a file can fail, change this to return
// `anyhow::Result<()>` so errors can bubble up with `?`. (Clippy's
// `unnecessary_wraps` rejects a `Result` that can never be `Err`.)
fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Open { path } => println!("would open {path}"),
    }
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
