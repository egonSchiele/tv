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
