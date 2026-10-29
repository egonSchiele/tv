//! File helpers shared by saving (`:w`) and sessions.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Serialize;

/// Write `value` as pretty JSON with a trailing newline, atomically. Tables
/// (`Table::save`) and `view.json` both go through here.
pub fn write_json(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    let mut json = serde_json::to_string_pretty(value)?;
    json.push('\n');
    write_atomic(path, json.as_bytes()).with_context(|| format!("writing {}", path.display()))
}

/// Write `contents` to `path` so readers see the old file or the new one,
/// never a partial write: write a temp file in the same directory, flush it
/// to disk, then rename it over `path`.
///
/// Errors are plain `io::Error`s, which don't name the path; callers add it as
/// context, the way `Table::from_path` does.
#[expect(
    clippy::disallowed_methods,
    reason = "removes only the temp file this call created"
)]
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let temp = temp_path(path)?;
    // `create_new` fails rather than clobber an existing file. If it fails,
    // there's nothing of ours to clean up.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = file
        .write_all(contents)
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        // Remove only the temp file this call created. The write's error is
        // the one worth reporting, so a failure here is ignored.
        let _ = fs::remove_file(&temp);
    }
    result
}

/// `.{file name}.{pid}.tmp` next to `path`. The same directory matters:
/// `rename` is only atomic within one filesystem.
fn temp_path(path: &Path) -> io::Result<PathBuf> {
    let Some(name) = path.file_name() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a file path",
        ));
    };
    let temp_name = format!(".{}.{}.tmp", name.to_string_lossy(), std::process::id());
    Ok(path.with_file_name(temp_name))
}

/// `~` and `~/rest` become the home directory. Anything else, including
/// `~user`, is returned unchanged.
pub fn expand_tilde(path: &str) -> PathBuf {
    let Some(home) = std::env::home_dir() else {
        return PathBuf::from(path);
    };
    if path == "~" {
        home
    } else if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(path)
    }
}

/// Longest slug `slug` produces, so a long title can't make an unwieldy file name.
pub const SLUG_MAX_LEN: usize = 50;

/// `text` as a file-name-safe slug: ASCII letters (lowercased) and digits are
/// kept, runs of anything else become one `-`, leading and trailing `-` are
/// trimmed, and the result is cut to `SLUG_MAX_LEN`. Can be empty.
pub fn slug(text: &str) -> String {
    let words: Vec<String> = text
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let mut slug = words.join("-");
    // The slug is ASCII, so cutting at a byte count can't split a character.
    slug.truncate(SLUG_MAX_LEN);
    slug.trim_end_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn write_atomic_creates_then_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.json");
        write_atomic(&path, b"first").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first");
        write_atomic(&path, b"second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        // No temp file is left behind.
        assert_eq!(file_names(dir.path()), ["a.json"]);
    }

    #[test]
    fn write_atomic_into_a_missing_directory_fails_and_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("a.json");
        let error = write_atomic(&path, b"x").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(file_names(dir.path()), Vec::<String>::new());
    }

    #[test]
    fn expand_tilde_only_expands_the_home_forms() {
        let home = std::env::home_dir().unwrap();
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("~/x/y.json"), home.join("x/y.json"));
        assert_eq!(expand_tilde("a/~b"), PathBuf::from("a/~b"));
        assert_eq!(expand_tilde("~user/x"), PathBuf::from("~user/x"));
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Open pull requests"), "open-pull-requests");
        assert_eq!(slug("Q3: Revenue — by region!"), "q3-revenue-by-region");
        assert_eq!(slug("日本語 — !!"), "");
        let long = slug(&"word ".repeat(30));
        assert!(long.len() <= SLUG_MAX_LEN, "{long}");
        assert!(!long.ends_with('-'), "{long}");
    }
}
