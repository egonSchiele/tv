use std::time::{Duration, SystemTime};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};

mod agent;
mod comments;
mod files;
mod formats;
mod session;
mod skills;
#[cfg(test)]
mod snapshot;
mod table;
mod time;
mod viewer;

use session::{COMMENTS_FILE, Sessions};
use table::{Meta, Table, single_line};
use viewer::layout::{Align, display_width, fit};

/// A terminal table viewer you share with an agent.
// `derive(Parser)` generates the argument parser from this struct's fields and
// doc comments, so `tv --help` text comes from the `///` comments.
#[derive(Parser)]
// The optional positional and the optional subcommand let `tv x.json` and
// `tv open x.json` both work: a subcommand name wins, and anything else is the
// target. `args_conflicts_with_subcommands` makes `tv x.json open y.json` an
// error instead of silently accepting both.
#[command(
    name = "tv",
    version,
    args_conflicts_with_subcommands = true,
    after_help = "Agents: `tv agent help` is a guide to using tv."
)]
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
    /// List sessions, most recent first.
    Ls,
    /// Delete sessions. Refuses a session whose viewer is open.
    Rm {
        #[arg(required = true)]
        ids: Vec<String>,
        /// Delete even if a viewer has the session open.
        #[arg(long)]
        force: bool,
    },
    /// Write a session's table, comments included, to a JSON file.
    Save { id: String, path: String },
    /// Commands for agents. Each prints one line of JSON, except `help`.
    // clap adds a `help` subcommand to every group, which would shadow
    // `AgentCommand::Help`. `tv agent --help` still prints clap's help.
    #[command(
        disable_help_subcommand = true,
        after_help = "`tv agent help` is a guide to using tv: when, and how."
    )]
    Agent {
        #[command(subcommand)]
        command: agent::AgentCommand,
    },
    /// Agent skills that come with tv: list them, print one, or install one.
    Skills {
        #[command(subcommand)]
        command: Option<skills::SkillsCommand>,
    },
    /// Session maintenance.
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
}

#[derive(Subcommand)]
enum SessionCommand {
    /// Delete sessions that match every given filter. At least one filter is
    /// required. A session whose viewer is open is never deleted.
    Prune {
        /// Modified longer ago than this, e.g. `7d` or `12h`.
        #[arg(long, value_parser = parse_duration)]
        older_than: Option<Duration>,
        /// Only sessions whose viewer is closed.
        #[arg(long)]
        closed: bool,
        /// Print what would be deleted without deleting it.
        #[arg(long)]
        dry_run: bool,
    },
}

/// `7d` or `12h`: a whole number of days or hours.
fn parse_duration(text: &str) -> Result<Duration, String> {
    let error = || "expected a duration like 7d or 12h".to_string();
    // `strip_suffix`, not `split_at(len - 1)`, which panics when the last
    // character is more than one byte (`7é`).
    let (number, unit_secs) = if let Some(days) = text.strip_suffix('d') {
        (days, 86_400)
    } else if let Some(hours) = text.strip_suffix('h') {
        (hours, 3_600)
    } else {
        return Err(error());
    };
    let number: u64 = number.parse().map_err(|_| error())?;
    // A huge number would overflow, and overflow panics in debug builds.
    number
        .checked_mul(unit_secs)
        .map(Duration::from_secs)
        .ok_or_else(error)
}

impl Cli {
    /// What to open, from `tv <target>` or `tv open <target>`. This is the
    /// only place that knows there are two spellings. Other subcommands never
    /// have a positional, so for them it's `None`.
    fn argument(&self) -> Option<&str> {
        match &self.command {
            Some(Command::Open { target }) => target.as_deref(),
            _ => self.target.as_deref(),
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

/// Read the table on stdin, then hand stdin to the terminal for the viewer.
/// Reading stdin for `-` is a command-line convention, so this lives here
/// rather than in `table.rs`; `Target::parse` is what recognizes the `-`.
fn load_stdin() -> anyhow::Result<Table> {
    let text = std::io::read_to_string(std::io::stdin()).context("reading stdin")?;
    // The viewer reads keys from stdin. Now that the pipe is drained, point
    // stdin at the terminal: stdout, which must be the terminal for the
    // viewer to draw at all. crossterm would otherwise fall back to
    // `/dev/tty`, which its reader can't watch on macOS (kqueue doesn't
    // support that device), and fail with "Failed to initialize input reader".
    #[cfg(unix)]
    rustix::stdio::dup2_stdin(std::io::stdout()).context("attaching stdin to the terminal")?;
    Table::parse_input(&text, Meta::default(), Meta::default()).context("parsing stdin")
}

/// Open the viewer on `target`. A file or stdin becomes a new session first.
fn open(sessions: &Sessions, target: Target) -> anyhow::Result<()> {
    // Load before creating, so a bad file errors out before the screen
    // changes and leaves no session behind.
    let session = match target {
        Target::Stdin => sessions.create(&load_stdin()?)?,
        Target::File(path) => sessions.create(&Table::from_input_path(
            &path,
            Meta::default(),
            Meta::default(),
        )?)?,
        Target::Session(id) => sessions.open(&id)?,
        Target::MostRecent => sessions
            .most_recent()?
            .context("no sessions yet; open a file with `tv <file.json>`")?,
    };
    // For a new session this parses the same bytes again. It's cheap, and
    // it means every case ends the same way.
    let table = session.load_table()?;
    viewer::run(table, session)
}

/// For `ls` and `prune` output when `table.json` doesn't parse.
const UNREADABLE: &str = "(can't read table.json)";

/// A session's title on one line of `ls` or `prune` output.
fn title_text(title: Option<&str>) -> String {
    title.map_or_else(|| "(untitled)".to_string(), single_line)
}

/// A count in `ls`: `-` for none, like VIEWER's `-`, so a session that
/// needs you stands out.
fn count_text(count: usize) -> String {
    if count == 0 {
        "-".to_string()
    } else {
        count.to_string()
    }
}

/// `tv ls`: one row per session, newest first.
fn ls(sessions: &Sessions) -> anyhow::Result<()> {
    let list = sessions.list()?;
    if list.is_empty() {
        println!("no sessions");
        return Ok(());
    }
    // TITLE goes last: it's the only free text, so a long one wraps its own
    // line instead of pushing the other columns off every row.
    let mut rows = vec![
        ["ID", "ROWS×COLS", "VIEWER", "AWAITING", "MODIFIED", "TITLE"]
            .map(String::from)
            .to_vec(),
    ];
    for session in &list {
        // Asked here rather than in `summary`, so a broken `table.json` (no
        // summary) shows it the same way.
        let viewer = if session.viewer_open() { "open" } else { "-" };
        // One broken `table.json` shouldn't hide the rest: `ls` is how you
        // find its ID to `tv rm` it.
        rows.push(match session.summary() {
            Ok(summary) => vec![
                summary.id,
                format!("{}×{}", summary.rows, summary.columns),
                viewer.to_string(),
                count_text(summary.awaiting_reply),
                time::rfc3339(summary.modified_at),
                title_text(summary.title.as_deref()),
            ],
            Err(_) => vec![
                session.id().to_string(),
                "?".to_string(),
                viewer.to_string(),
                "?".to_string(),
                session
                    .modified_at()
                    .map_or_else(|_| "?".to_string(), time::rfc3339),
                UNREADABLE.to_string(),
            ],
        });
    }
    print!("{}", format_table(&rows));
    Ok(())
}

/// `rows` as aligned columns: each padded to its widest cell, two spaces
/// between, no trailing spaces, and a newline after every row. Widths are
/// display widths, so wide characters line up; `format!("{cell:<width$}")`
/// would count characters instead.
fn format_table(rows: &[Vec<String>]) -> String {
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<u16> = (0..column_count)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| display_width(cell))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut text = String::new();
    for row in rows {
        // `fit` pads by display width, and never truncates here because each
        // width is the column's widest cell.
        let cells: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(cell, &width)| fit(cell, width, Align::Left))
            .collect();
        text.push_str(cells.join("  ").trim_end());
        text.push('\n');
    }
    text
}

/// `tv rm`. Opens every ID, and without `force` checks that none has a
/// viewer open, before deleting any: a typo or an open viewer in the second
/// fails before the first is gone.
fn rm(sessions: &Sessions, ids: &[String], force: bool) -> anyhow::Result<()> {
    let mut unique: Vec<&str> = Vec::new();
    for id in ids {
        if !unique.contains(&id.as_str()) {
            unique.push(id);
        }
    }
    let found = unique
        .into_iter()
        .map(|id| sessions.open(id))
        .collect::<anyhow::Result<Vec<_>>>()?;
    if !force && let Some(open) = found.iter().find(|session| session.viewer_open()) {
        bail!(
            "session {} has a viewer open; close it or use --force",
            open.id()
        );
    }
    for session in found {
        let id = session.id().to_string();
        sessions.remove(session)?;
        println!("removed {id}");
    }
    Ok(())
}

/// `tv save`. Overwrites `path` like `cp` does. The viewer's `:w` refuses to,
/// because there a default path makes clobbering easy; neither applies to a
/// command with an explicit output path.
fn save(sessions: &Sessions, id: &str, path: &str) -> anyhow::Result<()> {
    let session = sessions.open(id)?;
    let mut table = session.load_table()?;
    let log = session.read_log()?;
    table.comments = comments::threads(&log.events, &table);
    table.save(&files::expand_tilde(path))?;
    println!("wrote {} rows to {path}", table.rows.len());
    // Not an error, so stdout, next to the line above.
    if let Some(warning) = log.warning() {
        println!("{COMMENTS_FILE}: {warning}");
    }
    Ok(())
}

/// `tv session prune`'s filters, as data. A session is pruned if it matches
/// every filter that is set.
struct PruneFilter {
    older_than: Option<Duration>,
    closed: bool,
}

impl PruneFilter {
    fn is_empty(&self) -> bool {
        self.older_than.is_none() && !self.closed
    }

    /// True when a session last modified at `modified_at`, whose viewer is
    /// open or not, matches every filter that is set.
    fn matches(&self, modified_at: SystemTime, viewer_open: bool, now: SystemTime) -> bool {
        // Compare ages: `now - limit` would panic for a huge limit. A future
        // mtime has age zero, and `>=` makes `0d` match everything.
        let age = now.duration_since(modified_at).unwrap_or_default();
        let old_enough = self.older_than.is_none_or(|limit| age >= limit);
        let closed_enough = !self.closed || !viewer_open;
        old_enough && closed_enough
    }
}

/// `tv session prune`. Decides on mtimes and `view.json` alone, so a session
/// whose `table.json` is broken (the one that most needs cleaning up) can
/// still be pruned; the title is only for the printed line. A session whose
/// viewer is open is skipped whatever the filters: a bulk delete shouldn't
/// pull a table out from under someone, so there's no `--force` here.
fn prune(sessions: &Sessions, filter: &PruneFilter, dry_run: bool) -> anyhow::Result<()> {
    if filter.is_empty() {
        bail!("give a filter: --older-than <duration> and/or --closed");
    }
    let now = SystemTime::now();
    let mut pruned = 0;
    for session in sessions.list()? {
        let viewer_open = session.viewer_open();
        if !filter.matches(session.modified_at()?, viewer_open, now) {
            continue;
        }
        let id = session.id().to_string();
        if viewer_open {
            println!("skipped {id}  (viewer open)");
            continue;
        }
        pruned += 1;
        let title = session.load_table().map_or_else(
            |_| UNREADABLE.to_string(),
            |table| title_text(table.title.as_deref()),
        );
        if dry_run {
            println!("would remove {id}  {title}");
        } else {
            sessions.remove(session)?;
            println!("removed {id}  {title}");
        }
    }
    // Skipped sessions don't count, so this prints after `skipped` lines too.
    if pruned == 0 {
        println!("nothing to prune");
    }
    Ok(())
}

// `anyhow::Result` lets any error type bubble up with `?`. Returning `Err` from
// `main` prints `Error: …` to stderr and exits with status 1.
fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let sessions = Sessions::from_home(std::env::var_os("TV_HOME"))?;
    // Only the first arm reads the target, so `tv ls` can't parse as one.
    // Matching on `&cli.command` borrows the field instead of moving it out
    // of `cli`, which keeps `cli.argument()` callable.
    match &cli.command {
        None | Some(Command::Open { .. }) => open(&sessions, Target::parse(cli.argument())),
        Some(Command::Ls) => ls(&sessions),
        Some(Command::Rm { ids, force }) => rm(&sessions, ids, *force),
        Some(Command::Save { id, path }) => save(&sessions, id, path),
        Some(Command::Agent { command }) => agent::run(&sessions, command),
        Some(Command::Skills { command }) => skills::run(command.as_ref()),
        Some(Command::Session {
            command:
                SessionCommand::Prune {
                    older_than,
                    closed,
                    dry_run,
                },
        }) => prune(
            &sessions,
            &PruneFilter {
                older_than: *older_than,
                closed: *closed,
            },
            *dry_run,
        ),
    }
}

// `#[cfg(test)]` compiles this module only for `cargo test`.
#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use clap::error::ErrorKind;

    const DAY: Duration = Duration::from_hours(24);

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

    fn parsed_command(args: &[&str]) -> Command {
        Cli::try_parse_from(args).unwrap().command.unwrap()
    }

    #[test]
    fn session_commands_parse() {
        assert!(matches!(parsed_command(&["tv", "ls"]), Command::Ls));

        let Command::Rm { ids, force } = parsed_command(&["tv", "rm", "a", "b"]) else {
            panic!("expected rm");
        };
        assert_eq!(ids, ["a", "b"]);
        assert!(!force);
        let Command::Rm { force, .. } = parsed_command(&["tv", "rm", "--force", "a"]) else {
            panic!("expected rm");
        };
        assert!(force);

        let Command::Save { id, path } = parsed_command(&["tv", "save", "k3f9x2", "out.json"])
        else {
            panic!("expected save");
        };
        assert_eq!((id.as_str(), path.as_str()), ("k3f9x2", "out.json"));

        let args = ["tv", "session", "prune", "--older-than", "7d", "--dry-run"];
        let Command::Session {
            command:
                SessionCommand::Prune {
                    older_than,
                    closed,
                    dry_run,
                },
        } = parsed_command(&args)
        else {
            panic!("expected session prune");
        };
        assert_eq!(older_than, Some(7 * DAY));
        assert!(!closed);
        assert!(dry_run);

        let Command::Session {
            command: SessionCommand::Prune { closed, .. },
        } = parsed_command(&["tv", "session", "prune", "--closed"])
        else {
            panic!("expected session prune");
        };
        assert!(closed);
    }

    #[test]
    fn rm_needs_an_id() {
        let error = Cli::try_parse_from(["tv", "rm"]).err().unwrap();
        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn a_bad_duration_names_the_flag() {
        let args = ["tv", "session", "prune", "--older-than", "soon"];
        let message = Cli::try_parse_from(args).err().unwrap().to_string();
        assert!(message.contains("--older-than"), "{message}");
        assert!(message.contains("expected a duration like 7d"), "{message}");
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("7d"), Ok(7 * DAY));
        assert_eq!(parse_duration("12h"), Ok(Duration::from_hours(12)));
        assert_eq!(parse_duration("0d"), Ok(Duration::ZERO));
        for bad in ["7", "d", "7w", "-1d", "7é", "99999999999999999d", ""] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
    }

    fn older_than(limit: Duration) -> PruneFilter {
        PruneFilter {
            older_than: Some(limit),
            closed: false,
        }
    }

    #[test]
    fn prune_filter_compares_ages() {
        let now = SystemTime::now();
        let week = older_than(7 * DAY);
        assert!(week.matches(now - 8 * DAY, false, now));
        assert!(!week.matches(now - 6 * DAY, false, now));
        assert!(!week.matches(now + DAY, false, now)); // a future mtime has age zero
        assert!(older_than(Duration::ZERO).matches(now, false, now));
        assert!(!older_than(Duration::MAX).matches(now - 8 * DAY, false, now));
        assert!(!week.is_empty());
    }

    #[test]
    fn prune_filter_closed() {
        let now = SystemTime::now();
        let closed = PruneFilter {
            older_than: None,
            closed: true,
        };
        assert!(closed.matches(now, false, now));
        assert!(!closed.matches(now, true, now));
        assert!(!closed.is_empty());

        // Without `--closed`, whether the viewer is open doesn't matter here;
        // `prune` skips open sessions on its own.
        assert!(older_than(Duration::ZERO).matches(now, true, now));

        let both = PruneFilter {
            older_than: Some(7 * DAY),
            closed: true,
        };
        assert!(both.matches(now - 8 * DAY, false, now));
        assert!(!both.matches(now - 8 * DAY, true, now));
        assert!(!both.matches(now - 6 * DAY, false, now));

        let none = PruneFilter {
            older_than: None,
            closed: false,
        };
        assert!(none.is_empty());
    }

    fn row(cells: &[&str]) -> Vec<String> {
        cells.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn format_table_pads_to_the_widest_cell() {
        let text = format_table(&[row(&["ID", "TITLE"]), row(&["abc123", "x"])]);
        assert_eq!(text, "ID      TITLE\nabc123  x\n");
    }

    #[test]
    fn format_table_pads_by_display_width() {
        // `日本` is 2 characters but 4 cells wide.
        let text = format_table(&[row(&["日本", "a"]), row(&["abcde", "b"])]);
        assert_eq!(text, "日本   a\nabcde  b\n");
    }

    #[test]
    fn format_table_leaves_no_trailing_spaces() {
        let text = format_table(&[row(&["a", "long title"]), row(&["b", "x"])]);
        assert!(text.lines().all(|line| !line.ends_with(' ')), "{text:?}");
    }

    /// `line` split into words the way a POSIX shell splits the guide's
    /// examples: spaces separate words, and '…' or "…" quote. The guide uses
    /// no escapes or variables.
    fn shell_words(line: &str) -> Vec<String> {
        let mut words = Vec::new();
        let mut word: Option<String> = None;
        let mut quote = None;
        for character in line.chars() {
            match (quote, character) {
                (Some(open), _) if character == open => quote = None,
                (None, '\'' | '"') => {
                    quote = Some(character);
                    word.get_or_insert_default();
                }
                (None, ' ') => words.extend(word.take()),
                _ => word.get_or_insert_default().push(character),
            }
        }
        words.extend(word);
        words
    }

    #[test]
    fn shell_words_handles_quotes() {
        assert_eq!(
            shell_words(r#"tv agent reply k3f9x2 c1 "to match it" --row '#3' ''"#),
            [
                "tv",
                "agent",
                "reply",
                "k3f9x2",
                "c1",
                "to match it",
                "--row",
                "#3",
                ""
            ]
        );
    }

    /// Every `tv` command in the guide's `sh` blocks parses, so the guide
    /// can't drift from the CLI. In a pipeline, the other commands (`gh`,
    /// `jq`) are skipped. Code blocks are `sh` or `json`, so a block can't be
    /// skipped by being labelled something else, or nothing.
    #[test]
    fn the_guides_examples_parse() {
        let mut block = None; // the open code block's language
        for line in skills::guide().lines() {
            if let Some(language) = line.strip_prefix("```") {
                block = match (block, language) {
                    (None, "sh" | "json") => Some(language),
                    (Some(_), "") => None,
                    _ => panic!("not an sh or json block: {line}"),
                };
            } else if block == Some("sh") {
                let commands: Vec<&str> = line
                    .split(" | ")
                    .filter(|command| command.starts_with("tv "))
                    .collect();
                assert!(!commands.is_empty(), "no tv command: {line}");
                for command in commands {
                    if let Err(error) = Cli::try_parse_from(shell_words(command)) {
                        panic!("{line}\n{error}");
                    }
                }
            }
        }
    }

    /// The Reference has one line per `tv agent` command, and every `--flag`
    /// on it is one that command takes.
    #[test]
    fn the_guides_reference_matches_the_cli() {
        let cli = Cli::command();
        let agent = cli.find_subcommand("agent").unwrap();
        let mut listed = Vec::new();
        for line in skills::guide().lines() {
            let Some(usage) = line.strip_prefix("- `tv agent ") else {
                continue;
            };
            let mut words = usage.split('`').next().unwrap().split(' ');
            let name = words.next().unwrap();
            let command = agent
                .find_subcommand(name)
                .unwrap_or_else(|| panic!("no such command: {line}"));
            for flag in words.filter_map(|word| word.trim_matches(['[', ']']).strip_prefix("--")) {
                let takes_it = command
                    .get_arguments()
                    .any(|arg| arg.get_long() == Some(flag));
                assert!(takes_it, "no --{flag}: {line}");
            }
            listed.push(name);
        }
        let mut commands: Vec<&str> = agent
            .get_subcommands()
            .map(clap::Command::get_name)
            .collect();
        listed.sort_unstable();
        commands.sort_unstable();
        assert_eq!(listed, commands);
    }

    #[test]
    fn agent_help_is_the_guide_not_clap() {
        let cli = Cli::try_parse_from(["tv", "agent", "help"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Agent {
                command: agent::AgentCommand::Help
            })
        ));
    }
}
