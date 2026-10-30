# Plan: ADI-203 skills and agent help

Linear: https://linear.app/aditya-bhargava/issue/ADI-203 (parent)
Sub-issues:
- ADI-231: `tv skills`, `tv skills show`, `tv skills install`, and the first skill, `tv-tables`.
- ADI-230: `tv agent help`, which prints the guide, and pointers to it from `tv --help` and `tv agent --help`.

Depends on: PR #9 merged (`c6962fc`). This plan covers all of ADI-203, so merging it closes the parent. Publishing is ADI-204.

## Goal

An agent can learn tv by itself, and a user can give their agent a skill that teaches it:

- **`tv agent help`** prints a short, example-driven guide: what tv is, when to use it, how to make and update a table, and how to handle "check my comments" and "this one".
- **`tv skills`** lists the skills that come with tv. `tv skills show <name>` prints one, and `tv skills install <name> --to <dir>` writes it where the user's agent looks for skills, such as `.claude/skills`.
- **One text for both.** The guide is the `tv-tables` skill without its header, so the two can't disagree. ADI-231 asks for this.
- `tv --help` and `tv agent --help` end with a line pointing at `tv agent help`.

## Context for the executing agent

- Read `SPEC.md`:
  - "Agent CLI": `tv agent help` and "Agent guide", the ten-point outline the guide follows.
  - "Principles": every `tv agent` command except `help` prints JSON.
  - "Human CLI", "Module layout", and the "Skills" bullet under "Changes after review".
- ADI-230's description has PR #9's carry-overs: the `help` subcommand clap adds, and the points the guide must make. They're all in this plan, under Decisions 4 and 8.
- Build on what's there:
  - `main.rs`: `format_table`, which lays out `tv ls`. The test module already imports `clap::CommandFactory`, so `Cli::command()` works there.
  - `agent.rs`: `run`, which prints every command's JSON.
  - `files::expand_tilde`, which `:w` uses for its path.
  - `tests/cli.rs`: `tv`, `succeeds` and `assert_fails_with`.
- **No new crates.** `include_str!` embeds each skill, and a few lines read its header. `include_dir` would add a crate to save one line per skill. `cargo tree` stays at 89.
- **No deletes.** `install` makes directories and writes one new file, and it never replaces one (Decision 5). `clippy.toml` doesn't change. Tests write only inside temp dirs.
- **Two code commits, skills first.** `dead_code` fires in a binary crate, so everything a commit adds needs a non-test caller in that commit. `Skill::body` and `skills::guide` have no caller until `tv agent help` exists, so ADI-231 (Part 1) comes before ADI-230 (Part 2), and Part 2 adds them.
- **Scratch check.** I built both parts in a scratch copy of `main`, and Part 1 on its own:
  - It's clean under `cargo clippy --all-targets -- -D warnings` (pedantic).
  - With both parts, 276 unit tests and 45 CLI tests pass. With Part 1 alone, 272 and 42; its CLI test isn't counted there, because I didn't add it to that copy.
  - I ran every example in the guide, in order, against a temp `TV_HOME` with `jq`. That's how Decision 8's `del(.columns)` point was found. It's also why the `--row '#3'` example is in the prose: the other examples' session has a key, and there `#3` is an error.
  - `cargo package --list` includes `skills/tv-tables/SKILL.md`, because `Cargo.toml` has no `include` list.
  - No traps came up. `Option::get_or_insert_default` needs Rust 1.83, and the toolchain is 1.99.
  - After the anti-pattern check, I compiled its changes again in the same copy: `bundled`, the `ls`-style rows, and the two reworked guide tests. It's still clean, with the same test counts.
  - The review (`plans/09-skills-and-help-review.md`) found holes in the two guide tests, which are fixed here. Every piece of a pipeline is now parsed, so the `get`s are too. The Reference's flags are checked, and its lines are compared as sorted lists. I kept the "blocks are `sh` or `json`" rule from the anti-pattern check in place of the review's `checked >= 10` count.

## Decisions

These go beyond what the spec and the Linear issues say. Each one is cheap, and SPEC.md is updated to match (Deliverables).

1. **The guide lives in `skills/tv-tables/SKILL.md`, and `tv agent help` prints it without the frontmatter** (the `---` block at the top of the file).
   - ADI-231 asks for the skill and the guide to share their text. With one file, an agent that has the skill installed has already read the guide.
   - The file is where ADI-231 puts skills, so it can be read, or copied by hand, on GitHub.
   - It replaces the spec's `src/agent_help.txt`.
2. **The guide is Markdown that reads as plain text.** A skill has to be Markdown. The spec says plain text, and an agent reads headings, short paragraphs and fenced code fine in a terminal. Prose is wrapped at 80 columns.
3. **A skill's name and description come only from its frontmatter.** `Skill::parse` reads the `name` and `description` lines. Rust keeps no second copy, so adding a skill means adding its file and one `include_str!` line. Tests check that:
   - every bundled file parses, and is the file at `skills/<name>/SKILL.md`, so its name matches its directory;
   - every directory under `skills/` is bundled.
4. **`tv agent help` is the one agent command that prints text, not JSON.** The spec's Principles already allow it.
   - clap adds a `help` subcommand to every group, which would shadow ours, so the `Agent` group gets `disable_help_subcommand = true`.
   - `tv agent --help`, `tv agent <command> --help` and `tv help agent` still print clap's help.
5. **`install` never replaces a file.**
   - If the same text is already there, it says so and exits 0. If something else is there, it's an error ending `move it aside to install`, and the file is left alone.
   - No `--force`: the file may hold the user's edits, so replacing it would be a delete in all but name. Upgrading a skill is rare, and moving a file aside is one command.
   - It opens the file with `create_new`, so a file can't appear between a check and the write and get replaced.
6. **`install` makes the directories, and `--to` is required.**
   - A project usually has no `.claude/skills` yet, so `install` runs `create_dir_all` on `<dir>/<name>`.
   - ADI-231 says there's no default location, so `--to` has no default.
   - The path goes through `files::expand_tilde`, as `:w`'s does, so a quoted `'~/.claude/skills'` works too.
7. **A test checks the guide against the CLI.** The guide is the agent's manual, and a renamed flag would otherwise break it without anyone noticing.
   - Every `tv` command in a `sh` block parses with `Cli::try_parse_from`, and each line has at least one. Code blocks must be `sh` or `json`, so none is skipped.
   - The Reference has exactly one line per `tv agent` subcommand, and every `--flag` on it exists.
   - The JSON blocks are shortened illustrations, so nothing checks them.
8. **The guide covers what agents trip over,** beyond the spec's ten points. These come from PR #9's carry-overs and from running the examples:
   - Quote row IDs that start with `#` (`--row '#3'`): an unquoted `#` starts a shell comment.
   - Comment text is one quoted argument, and it may start with `-`.
   - `push` keeps the session's key and title, and `"key": null` drops the key.
   - **When adding or renaming a field, drop `columns` with `del(.columns)`.** `get` prints `columns`, and a declared column list turns off inference. A field added with `jq` is stored but never shown, and `push` says nothing. I found this by running `get | jq '.rows[0].priority = "high"' | push`: it still printed `"columns":9`. With `del(.columns)` it printed 10.
   - Give the user `open_command` exactly as printed, since it may set `TV_HOME`.
   - `goto` waits until the user is back in the grid, and works with no viewer open.
9. **Bare `tv skills` lists the skills,** with a header row, through `format_table` like `tv ls`. ADI-231 has no `list` subcommand, so there isn't one.
10. **`tv --help` and `tv agent --help` point to the guide,** through clap's `after_help`, as ADI-203's "Done when" asks.

## What this plan defers, and to where

| Item | Lands in |
|---|---|
| A README section on using tv with an agent, including `tv skills install tv-tables --to .claude/skills` | ADI-204, with the install instructions |
| Package contents: `cargo package --list` includes `skills/tv-tables/SKILL.md` today, because `Cargo.toml` has no `include` list. An `include` list added for publishing must keep `skills/`, or the build breaks. | ADI-204 |

## Part 1: `tv skills` (ADI-231)

### The skill: `skills/tv-tables/SKILL.md`

The frontmatter's `description` is what an agent like Claude Code reads to decide when to load the skill, so it names the situations: a big table, the user asking for tv, "check my comments", and "this one".

Rules for editing it:
- Keep prose lines to 80 columns.
- A `sh` block holds only commands, and every line has at least one `tv` command (Decision 7's test). Use the made-up session ID `k3f9x2` throughout.
- Every agent command has one `` - `tv agent <name> …` `` line in the Reference, and its flags are real.
- The frontmatter is read as YAML by agents. Keep `: ` and ` #` out of the `description`: in YAML the first is an error, and the second starts a comment. `Skill::parse` accepts both, so a test checks for them.
- After any edit, run the examples again by hand (Manual check, step 2).

````markdown
---
name: tv-tables
description: Show tabular data to the user in tv, a terminal table viewer they browse and comment on while you read their cursor and comments. Use it for a table with more than about 6 columns or 20 rows, when the user asks to see something in tv, or when they say to check their tv comments or point at "this one" in a table.
---

# tv: show the user a table

tv is a terminal table viewer you share with the user. You make a session
from JSON rows, and the user opens it in their own terminal, where they can
scroll, read whole cells and comment on them. While they look, you can
replace the table, see which cell their cursor is on, and answer their
comments. Every `tv agent` command prints one line of JSON. Errors go to
stderr, with a non-zero exit status.

## When to use it

Use tv instead of printing a table when it has more than about 6 columns or
20 rows, or whenever the user asks for it. A small table is fine inline.

## Quick start

Pipe a JSON array of objects into `tv agent new`, or give it a file:

```sh
gh pr list --json number,title,headRefName | tv agent new --key number --title "Open PRs"
tv agent new rows.json --key id --title "Failing tests"
```

It prints the session:

```json
{"session":"k3f9x2","open_command":"tv open k3f9x2","rows":38,"columns":3}
```

Give the user the `open_command` exactly as printed, to run in another
terminal. You can't open the viewer yourself: it needs their terminal.

## Always set a key

`--key` names a column whose values are unique and never null, like an ID or
a PR number. Comments and the user's cursor follow rows by key, so they stay
on the right row when you update the table. Without a key, rows are named by
position, from `#0`, so a comment on `#3` stays on the fourth row, whichever
row a push puts there.

## Updating the table

`push` replaces the whole table. An open viewer reloads it at once, keeps
the cursor on its row, and highlights what changed. `push` keeps the
session's key and title unless you pass `--key` or `--title`:

```sh
gh pr list --json number,title,headRefName | tv agent push k3f9x2
```

For a small edit, `get` the table, change it, and push it back:

```sh
tv agent get k3f9x2 | jq '(.rows[] | select(.number == 404)).title = "Fixed"' | tv agent push k3f9x2
```

`get` prints `columns` too, and only those columns are shown. To add or
rename a field, drop them with `del(.columns)`, and tv works them out again.
That also drops any `width` a column set:

```sh
tv agent get k3f9x2 | jq 'del(.columns) | .rows[].reviewed = false' | tv agent push k3f9x2
```

A push with missing or repeated key values is rejected, and the table stays
as it was. To drop the key on purpose, push the full format with
`"key": null`.

## Comments

When the user says to check their comments, list the threads waiting for
you:

```sh
tv agent comments k3f9x2
```

```json
[{"id":"c7h2kq","row_id":"404","column":"title","value":"Bump serde","awaiting_reply":true,"orphaned":false,"messages":[{"id":"c7h2kq","author":"user","text":"why the rename?","ts":"2026-10-28T19:03:00Z"}]}]
```

Answer each one, and push any change it asked for. The text is one
argument, so quote it. It may start with `-`:

```sh
tv agent reply k3f9x2 c7h2kq "To match the issue title."
```

`orphaned: true` means a push removed the thread's row or column. To point
the user at a cell yourself, start a thread on it. They jump between
threads with `]`:

```sh
tv agent comment k3f9x2 --row 404 --col title "This one fails CI."
```

## "This one"

When the user says "this one", "here" or "this row", ask where their cursor
is:

```sh
tv agent focus k3f9x2
```

```json
{"viewer_open":true,"row_id":"404","column":"title","value":"Bump serde","row":{"number":404,"title":"Bump serde"}}
```

To move their cursor to a cell:

```sh
tv agent goto k3f9x2 --row 404 --col title
```

It moves once they're back in the grid, not while they're reading a popup
or typing. With no viewer open, the next one opens there.

Row IDs, here and in `comment`, are the key's values as text. Without a key
they're `#0`, `#1`, …, so quote them, as in `--row '#3'`: an unquoted `#`
starts a shell comment.

## Finding a session

If you don't have the session ID, list the sessions, most recent first:

```sh
tv agent sessions
```

## Don't poll

Check comments or focus only when the user asks, or refers to the table.
Nothing tells you when they comment, and nothing needs to: they'll say so.

## Reference

- `tv agent new [FILE] [--title T] [--key K]`: make a session from FILE, or
  from stdin.
- `tv agent push <id> [FILE] [--title T] [--key K]`: replace the table.
- `tv agent get <id>`: print the table.
- `tv agent focus <id>`: the cell the user's cursor is on, and its row.
- `tv agent goto <id> --row R [--col C]`: move the user's cursor.
- `tv agent comments <id> [--all]`: threads awaiting your reply, or all of
  them.
- `tv agent reply <id> <thread-id> <text>`: answer a thread.
- `tv agent comment <id> --row R --col C <text>`: start a thread on a cell.
- `tv agent sessions`: list sessions.
- `tv agent help`: this guide.

`tv agent <command> --help` shows one command's options.
````

### `src/skills.rs` (new)

This is Part 1's version. Part 2 adds a `body` field and `guide`.

```rust
//! Agent skills bundled into the binary: `tv skills`. Each skill is a
//! `skills/<name>/SKILL.md` in the repo, in the format agents such as Claude
//! Code read: a `---` block with `name` and `description`, then Markdown.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use anyhow::{Context, bail};
use clap::Subcommand;

use crate::files;

/// The `tv-tables` skill: when and how to show the user a table in tv.
const TV_TABLES: &str = include_str!("../skills/tv-tables/SKILL.md");

/// Every bundled skill's `SKILL.md`. To add a skill, add its file here.
const SKILLS: &[&str] = &[TV_TABLES];

#[derive(Subcommand)]
pub enum SkillsCommand {
    /// Print a skill's SKILL.md.
    Show { name: String },
    /// Write a skill to DIR/NAME/SKILL.md, for example with
    /// `--to .claude/skills`.
    Install {
        name: String,
        /// The skills directory to write into. Made if it doesn't exist.
        #[arg(long)]
        to: String,
    },
}

/// A bundled `SKILL.md`, split at its frontmatter.
struct Skill {
    name: &'static str,
    description: &'static str,
    /// The whole file, as `show` prints it and `install` writes it.
    text: &'static str,
}

impl Skill {
    /// Read `name` and `description` from `text`'s frontmatter, or `None`
    /// unless it starts with a `---` block holding both. Only one-line
    /// `key: value` fields are read, which is all the bundled skills use.
    fn parse(text: &'static str) -> Option<Skill> {
        let (header, _body) = text.strip_prefix("---\n")?.split_once("\n---\n")?;
        let field = |key: &str| {
            header
                .lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix(':'))
                .map(str::trim)
        };
        Some(Skill {
            name: field("name")?,
            description: field("description")?,
            text,
        })
    }
}

/// A bundled skill, parsed. A test parses each one, so this can't fail.
fn bundled(text: &'static str) -> Skill {
    Skill::parse(text).expect("a test parses every bundled skill")
}

/// Every bundled skill.
fn skills() -> impl Iterator<Item = Skill> {
    SKILLS.iter().copied().map(bundled)
}

fn find(name: &str) -> anyhow::Result<Skill> {
    match skills().find(|skill| skill.name == name) {
        Some(skill) => Ok(skill),
        None => bail!("no skill \"{name}\": `tv skills` lists them"),
    }
}

/// Run `tv skills` (no subcommand: list them) or one of its subcommands.
pub fn run(command: Option<&SkillsCommand>) -> anyhow::Result<()> {
    match command {
        None => {
            let mut rows = vec![["NAME", "DESCRIPTION"].map(String::from).to_vec()];
            rows.extend(
                skills().map(|skill| [skill.name, skill.description].map(String::from).to_vec()),
            );
            print!("{}", crate::format_table(&rows));
        }
        Some(SkillsCommand::Show { name }) => print!("{}", find(name)?.text),
        Some(SkillsCommand::Install { name, to }) => {
            println!("{}", install(&find(name)?, &files::expand_tilde(to))?);
        }
    }
    Ok(())
}

/// Write `skill` to `dir/<name>/SKILL.md`, making the directories, and say
/// what happened. A file that's already there is never replaced: the same
/// text is fine, and anything else is an error, since it may be the user's
/// own edit.
fn install(skill: &Skill, dir: &Path) -> anyhow::Result<String> {
    let skill_dir = dir.join(skill.name);
    fs::create_dir_all(&skill_dir).with_context(|| format!("creating {}", skill_dir.display()))?;
    let path = skill_dir.join("SKILL.md");
    let writing = || format!("writing {}", path.display());
    // `create_new` fails if the file exists, so nothing can appear between
    // a check and the write and be replaced.
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            file.write_all(skill.text.as_bytes())
                .with_context(writing)?;
            Ok(format!("installed {} at {}", skill.name, path.display()))
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let existing =
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            if existing != skill.text {
                bail!(
                    "{} already exists and isn't this version of {}; move it aside to install",
                    path.display(),
                    skill.name
                );
            }
            Ok(format!(
                "{} is already installed at {}",
                skill.name,
                path.display()
            ))
        }
        Err(error) => Err(error).with_context(writing),
    }
}
```

Notes:
- `crate::format_table` is a private function in `main.rs`. A child module can use its parent's private items, so it doesn't need to become `pub`. The rows are built the way `ls` builds them: `[…].map(String::from).to_vec()`.
- `skills()` builds each `Skill` on every call. There's one skill, and each command calls it once, so caching the parsed skills would buy nothing.
- If the write fails partway (a full disk), a partial file is left, and the next `install` calls it a different file. Moving it aside fixes that, the same as for an edit.

### `src/main.rs`

```rust
mod skills;
```

```rust
    /// Agent skills that come with tv: list them, print one, or install one.
    Skills {
        #[command(subcommand)]
        command: Option<skills::SkillsCommand>,
    },
```

That goes after `Agent` in `Command`. The arm goes after `Agent`'s:

```rust
        Some(Command::Skills { command }) => skills::run(command.as_ref()),
```

`main` builds `Sessions` before it dispatches, and `tv skills` doesn't use it. That's harmless: `from_home` reads `TV_HOME` and touches no files.

### Tests

In `skills.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_skill_parses_and_is_its_own_file() {
        for text in SKILLS {
            let skill = Skill::parse(text).expect("frontmatter with name and description");
            let path = format!("skills/{}/SKILL.md", skill.name);
            assert_eq!(fs::read_to_string(&path).unwrap(), *text, "{path}");
            assert!(!skill.description.is_empty(), "{path}");
            // Agents read the frontmatter as YAML, where `: ` is an error and
            // ` #` starts a comment. `parse` accepts both.
            assert!(
                !skill.description.contains(": ") && !skill.description.contains(" #"),
                "{path}"
            );
        }
    }

    #[test]
    fn every_skill_directory_is_bundled() {
        for entry in fs::read_dir("skills").unwrap() {
            let entry = entry.unwrap();
            // Skip files such as macOS's `.DS_Store`.
            if !entry.file_type().unwrap().is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_str().unwrap();
            assert!(find(name).is_ok(), "skills/{name} isn't in SKILLS");
        }
    }

    #[test]
    fn parse_needs_a_name_and_a_description() {
        let skill = Skill::parse("---\nname: x\ndescription: does y\n---\n\n# X\n").unwrap();
        assert_eq!((skill.name, skill.description), ("x", "does y"));
        assert!(Skill::parse("# no frontmatter\n").is_none());
        assert!(Skill::parse("---\nname: x\n---\n# X\n").is_none());
    }

    #[test]
    fn install_writes_once_and_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join(".claude/skills");
        let skill = find("tv-tables").unwrap();
        let path = skills_dir.join("tv-tables/SKILL.md");

        let message = install(&skill, &skills_dir).unwrap();
        assert!(message.starts_with("installed tv-tables at "), "{message}");
        assert_eq!(fs::read_to_string(&path).unwrap(), skill.text);

        let message = install(&skill, &skills_dir).unwrap();
        assert!(message.contains("already installed"), "{message}");

        fs::write(&path, "my edits").unwrap();
        let error = install(&skill, &skills_dir).unwrap_err().to_string();
        assert!(error.contains("move it aside"), "{error}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "my edits");
    }
}
```

Tests run from the package root, so `skills/…` paths work, as `examples/prs.json` does in other tests. I checked `every_skill_directory_is_bundled` with a `.DS_Store` in `skills/`.

In `tests/cli.rs`:

```rust
// `tv skills` and `tv agent help`

/// The bundled skill's file, as the repo has it.
fn tv_tables() -> String {
    fs::read_to_string("skills/tv-tables/SKILL.md").unwrap()
}

#[test]
fn skills_lists_shows_and_installs() {
    let home = tempfile::tempdir().unwrap();
    let listed = succeeds(&tv(home.path(), &["skills"], ""));
    assert!(listed.starts_with("NAME"), "{listed}");
    assert!(listed.contains("\ntv-tables  Show tabular data"), "{listed}");

    let shown = succeeds(&tv(home.path(), &["skills", "show", "tv-tables"], ""));
    assert_eq!(shown, tv_tables());
    let output = tv(home.path(), &["skills", "show", "nope"], "");
    assert_fails_with(&output, "no skill \"nope\"");

    let to = home.path().join(".claude/skills");
    let install = || {
        tv(
            home.path(),
            &["skills", "install", "tv-tables", "--to", to.to_str().unwrap()],
            "",
        )
    };
    assert!(succeeds(&install()).starts_with("installed tv-tables at "));
    let path = to.join("tv-tables/SKILL.md");
    assert_eq!(fs::read_to_string(&path).unwrap(), tv_tables());
    assert!(succeeds(&install()).contains("already installed"));
    fs::write(&path, "my edits").unwrap();
    assert_fails_with(&install(), "move it aside");
    assert_eq!(fs::read_to_string(&path).unwrap(), "my edits");
}
```

## Part 2: `tv agent help` (ADI-230)

### `src/skills.rs`

The module doc and `TV_TABLES`'s doc now mention the guide:

```rust
//! Agent skills bundled into the binary: `tv skills`, and the guide that
//! `tv agent help` prints. Each skill is a `skills/<name>/SKILL.md` in the
//! repo, in the format agents such as Claude Code read: a `---` block with
//! `name` and `description`, then Markdown.
```

```rust
/// The `tv-tables` skill. Its body is also `tv agent help`'s guide, so an
/// agent that has the skill has read the guide.
const TV_TABLES: &str = include_str!("../skills/tv-tables/SKILL.md");
```

`Skill` gets a `body` field, which `parse` fills in. `parse`'s doc becomes "Split `text` into its frontmatter's `name` and `description` and its body, or `None` unless …":

```rust
    /// The Markdown after the frontmatter.
    body: &'static str,
```

```rust
        let (header, body) = text.strip_prefix("---\n")?.split_once("\n---\n")?;
        // …
        Some(Skill {
            name: field("name")?,
            description: field("description")?,
            text,
            body: body.trim_start_matches('\n'),
        })
```

`guide` goes after `find`:

```rust
/// What `tv agent help` prints: the `tv-tables` skill without its
/// frontmatter.
pub fn guide() -> &'static str {
    bundled(TV_TABLES).body
}
```

Tests:
- `every_skill_parses_and_is_its_own_file` adds `assert!(skill.body.starts_with("# "), "{path}");`.
- `parse_needs_a_name_and_a_description` checks the body too:

```rust
        assert_eq!(
            (skill.name, skill.description, skill.body),
            ("x", "does y", "# X\n")
        );
```

### `src/agent.rs`

The module doc's first sentence becomes "Each one prints one line of JSON on stdout, except `help`." Add `use crate::skills;`, then the last variant of `AgentCommand`:

```rust
    /// Print the guide: when to use tv, and how. Plain text, not JSON.
    Help,
```

`run`'s match gets a first arm:

```rust
        // The one command that isn't JSON: the guide is for reading.
        AgentCommand::Help => {
            print!("{}", skills::guide());
            return Ok(());
        }
```

`print!`, not `println!`, because the file already ends with a newline.

### `src/main.rs`

On `Cli`:

```rust
#[command(
    name = "tv",
    version,
    args_conflicts_with_subcommands = true,
    after_help = "Agents: `tv agent help` is a guide to using tv."
)]
```

On the `Agent` variant:

```rust
    /// Commands for agents. Each prints one line of JSON, except `help`.
    // clap adds a `help` subcommand to every group, which would shadow
    // `AgentCommand::Help`. `tv agent --help` still prints clap's help.
    #[command(
        disable_help_subcommand = true,
        after_help = "`tv agent help` is a guide to using tv: when, and how."
    )]
    Agent {
```

What I saw in the scratch build:
- `tv agent` with no subcommand still prints the command list. `help` is in it, with the doc comment above.
- `tv agent help extra` is a clap usage error, with exit status 2.
- `tv help agent` prints clap's help for the group, ending with the `after_help` line.

### Tests

In `main.rs`'s test module:

````rust
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
    /// skipped by being labelled something else.
    #[test]
    fn the_guides_examples_parse() {
        let mut in_sh = false;
        for line in skills::guide().lines() {
            match line {
                "```sh" => in_sh = true,
                "```json" | "```" => in_sh = false,
                fence if fence.starts_with("```") => panic!("not sh or json: {fence}"),
                _ if in_sh => {
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
                _ => {}
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
````

Notes:
- The test module already imports `clap::CommandFactory`, which `Cli::command()` needs.
- `the_guides_examples_parse` checks parsing only, not behavior. The manual check runs the examples for real. The guide has 13 `tv` commands today, on 11 lines. Both `get`s are first in a pipeline, so every piece of a pipeline is checked, not only the last. Splitting on `" | "` also splits the `jq` programs, which contain ` | ` inside their quotes. That's harmless: none of those pieces starts with `tv `, so they're skipped.
- `the_guides_reference_matches_the_cli` compares sorted lists, so a missing line, a stale one, and a command listed twice all fail. A substring search would let a missing `comment` line pass, because `` - `tv agent comments `` starts with the same text. It also checks every `--flag` on a Reference line, since `--all` appears nowhere else. Both lists include `help`, since our `Help` is a real subcommand now.
- In the scratch copy, I checked that each test fails when it should: with the `comment` line deleted, with `--all` renamed in the Reference, with both `get`s renamed, and with one block relabelled `bash`.

In `tests/cli.rs`:

```rust
#[test]
fn agent_help_prints_the_skill_without_its_frontmatter() {
    let home = tempfile::tempdir().unwrap();
    let guide = succeeds(&tv(home.path(), &["agent", "help"], ""));
    // The guide is the end of the file: everything after the frontmatter.
    assert!(tv_tables().ends_with(&guide), "{guide}");
    assert!(guide.starts_with("# tv"), "{guide}");
}

#[test]
fn help_points_agents_at_the_guide() {
    let home = tempfile::tempdir().unwrap();
    for args in [&["--help"][..], &["agent", "--help"]] {
        let help = succeeds(&tv(home.path(), args, ""));
        assert!(help.contains("`tv agent help`"), "{args:?}: {help}");
    }
}
```

## Manual check

Build, and put the build on `PATH`, since the printed `open_command` runs `tv`:

```sh
cd ~/tv && cargo build
export PATH="$PWD/target/debug:$PATH"
export TV_HOME=$(mktemp -d)
```

1. **Reading the guide.** `tv agent help` in an 80-column terminal reads cleanly. It starts at `# tv: show the user a table`, with no `---` block.
2. **Following the guide.** Make the rows with the `jq` line below, since this repo's PRs don't include 404, which the examples use. Run each `sh` example in order: pipe those rows in place of `gh pr list …`, and use the real session ID in place of `k3f9x2`. Open the viewer in a second terminal right after `new`. Check that each example does what the text around it says.

   ```sh
   jq -c '[.rows[] | {number, title, headRefName: "b\(.number)"}]' examples/prs.json > /tmp/prs.json
   ```

   - Make a small `rows.json` with an `id` field.
   - After the `del(.columns)` push, `get` lists `reviewed` in `columns`.
   - Before running `comments`, comment on 404's title in the viewer (`c`). `comments` lists that thread, and `reply` answers it using that thread's ID. Its marker turns green.
   - `focus` shows `viewer_open: true`, and `goto` moves the cursor.
3. **Skills.**
   - `tv skills` lists `tv-tables` with its description.
   - `tv skills show tv-tables | head -4` shows the frontmatter.
   - `tv skills install tv-tables --to "$TV_HOME/.claude/skills"` prints `installed tv-tables at …`. Run it again: it says `already installed`.
   - Append a line to the installed file, and run it once more. It fails with `move it aside to install`, and the line is still there.
4. **Help pointers.** `tv --help` ends with ``Agents: `tv agent help` is a guide to using tv.`` `tv agent --help` lists `help` among the commands and ends with its own pointer line.
5. `unset TV_HOME`.

## Acceptance

```sh
make check
cargo tree --locked -e normal --prefix none | sort -u | wc -l   # 89, unchanged
```

Plus the manual check above.

## Deliverables

- **`skills/tv-tables/SKILL.md` (new):** the skill and the guide.
- **`src/skills.rs` (new):** `SkillsCommand`, `Skill` and `Skill::parse`, `bundled`, `skills`, `find`, `run`, `install`, `guide`, and their tests.
- **`src/main.rs`:**
  - `mod skills`, `Command::Skills` and its arm.
  - The `after_help` on `Cli`, and the attributes and doc on `Agent`.
  - `shell_words` and its test, and the three guide tests.
- **`src/agent.rs`:** `AgentCommand::Help`, its arm in `run`, and the module doc.
- **`tests/cli.rs`:** `tv_tables` and the three tests above.
- **`SPEC.md`:**
  - **`tv agent help`:** "Prints the agent guide. The guide is the body of the bundled `tv-tables` skill, `skills/tv-tables/SKILL.md`, embedded with `include_str!`, so an agent with the skill installed has read the same text. It's Markdown, wrapped at 80 columns to read as plain text. `tv --help` and `tv agent --help` point to it."
  - **"Agent guide":** after the outline, say that the guide also covers the points in Decision 8, and that a unit test parses every command in its `sh` blocks and checks that its Reference lists every `tv agent` command, and the flags it names.
  - **"Human CLI":** three rows:
    - `tv skills`: list the agent skills that come with tv.
    - `tv skills show <name>`: print a skill's `SKILL.md`.
    - `tv skills install <name> --to <dir>`: write `<dir>/<name>/SKILL.md`, making the directories. It never replaces a file: the same text is fine, and different text is an error.
  - **"Changes after review":** the Skills bullet becomes a pointer, `**Skills:** \`tv skills\`, under "Human CLI".`, as the `goto` bullet became in PR #9.
  - **Module layout:** `agent_help.txt` becomes `skills.rs  # bundled skills (tv skills) and the guide tv agent help prints`. Below the block, add: "Skills live outside `src/`, one directory each: `skills/<name>/SKILL.md`. `skills.rs` `include_str!`s them."
  - **"Testing":** a bullet saying that unit tests check the agent guide's examples and reference against the CLI.
- **Three commits:**
  1. `Plans: skills and agent help (ADI-230, ADI-231)`: this file, after its review.
  2. `tv skills: list, show, install (ADI-231)`: Part 1, with the whole `SKILL.md`. Its callers keep `dead_code` quiet: `run` reads every `Skill` field. The SPEC's Human CLI, Changes-after-review and module-layout edits go here. The guide's `tv agent help` Reference line only becomes true in commit 3, which is fine inside one PR.
  3. `tv agent help: the guide (ADI-230)`: Part 2. `Skill::body` and `guide` are read by `AgentCommand::Help`. The SPEC's `tv agent help`, Agent-guide and Testing edits go here.

## Out of scope

- **A `push` warning for row fields that aren't in `columns`.** The guide's `del(.columns)` covers it for agents. A warning would change `push`, which is outside ADI-203, but it's cheap if wanted later.
- **`install --force`, and `tv skills uninstall`.** Both would replace or remove a file that may hold the user's edits (Decision 5). The user moves or removes it themselves.
- **YAML beyond one-line `key: value` fields.** The bundled skills are ours, and a test parses each one.
- **Windows line endings.** `Skill::parse` expects `\n`. A Windows checkout with `core.autocrlf` would fail `every_skill_parses_and_is_its_own_file`, which would catch it. tv's CI and its pid check are Unix-only today.
- **Generating the guide's Reference from clap.** The test catches a missing command, and hand-written lines read better than clap's.
