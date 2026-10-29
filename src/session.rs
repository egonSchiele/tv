//! Sessions: a directory per open table under `$TV_HOME/sessions/`.
//!
//! A directory counts as a session only once it holds `table.json`. That's
//! what makes `create` atomic to readers: `table.json` is written atomically
//! as the last step, so a concurrent `tv ls` sees either no session or a
//! complete one, never a half-made one. Keep that rule when adding other
//! ways to create sessions (`tv agent new`, ADI-225).

use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder};
use std::hash::{BuildHasher, Hasher, RandomState};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, bail};

use crate::table::Table;

/// Session IDs are `ID_LEN` characters from `ID_ALPHABET`, like `k3f9x2`:
/// short enough to type.
const ID_LEN: usize = 6;
const ID_ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

const TABLE_FILE: &str = "table.json";

/// Every file a session directory may hold. `remove` deletes exactly these
/// and nothing else; add a name here when a new file joins the layout.
/// `goto.json` is ADI-228's cursor request from the agent; it's listed now so
/// `tv rm` doesn't refuse a session with a pending goto.
const SESSION_FILES: &[&str] = &[TABLE_FILE, "view.json", "comments.jsonl", "goto.json"];

/// The sessions root. Holds the path only; every method does its IO on call.
pub struct Sessions {
    root: PathBuf, // …/sessions
}

/// One session directory. Only `Sessions` makes these, so the ID is always
/// well-formed and the directory is always under the root.
#[derive(Debug)]
pub struct Session {
    id: String,
    dir: PathBuf,
}

/// What `tv ls` shows about a session.
pub struct Summary {
    pub id: String,
    pub title: Option<String>,
    pub rows: usize,
    pub columns: usize,
    pub modified_at: SystemTime,
}

impl Sessions {
    /// `$TV_HOME/sessions`, or `~/.tv/sessions`. Takes the variable's value
    /// rather than reading the environment, so tests can pass their own.
    pub fn from_home(tv_home: Option<OsString>) -> anyhow::Result<Sessions> {
        // An empty `TV_HOME=` counts as unset, as with XDG variables. Otherwise
        // the root would be the relative path `sessions`, in whatever
        // directory `tv` runs from.
        let home = match tv_home.filter(|home| !home.is_empty()) {
            Some(home) => PathBuf::from(home),
            None => std::env::home_dir()
                .context("can't find your home directory; set TV_HOME")?
                .join(".tv"),
        };
        Ok(Sessions::new(home.join("sessions")))
    }

    pub fn new(root: PathBuf) -> Sessions {
        Sessions { root }
    }

    /// A new session holding `table`. Creates the root if needed.
    pub fn create(&self, table: &Table) -> anyhow::Result<Session> {
        create_root(&self.root).with_context(|| format!("creating {}", self.root.display()))?;
        let session = loop {
            let id = random_id();
            let claimed = self
                .claim_dir(&id)
                .with_context(|| format!("creating {}", self.root.join(&id).display()))?;
            if let Some(dir) = claimed {
                break Session { id, dir };
            }
        };
        // If this fails (disk full, say), the empty directory stays. `list`
        // and `open` ignore it, and removing it would need another exempted
        // delete call (see `clippy.toml`).
        session.write_table(table)?;
        Ok(session)
    }

    /// Make the directory for `id`, or `None` if it's taken. Making it is the
    /// collision check, so there's no gap between checking and creating.
    fn claim_dir(&self, id: &str) -> io::Result<Option<PathBuf>> {
        let dir = self.root.join(id);
        match fs::create_dir(&dir) {
            Ok(()) => Ok(Some(dir)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// An existing session. Fails with `no session "{id}"` if `id` is
    /// malformed, the directory is missing, or it fails `is_session_dir`
    /// (for example, an empty directory left by a `create` whose
    /// `write_table` failed).
    pub fn open(&self, id: &str) -> anyhow::Result<Session> {
        // Check the ID before joining: `root.join("/etc")` is `/etc`.
        if is_session_id(id)
            && let Some(session) = self.session_at(self.root.join(id))
        {
            return Ok(session);
        }
        bail!("no session \"{id}\"")
    }

    /// Every session, most recently modified first. A missing root is an
    /// empty list, not an error.
    pub fn list(&self) -> anyhow::Result<Vec<Session>> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", self.root.display()));
            }
        };
        // `modified_at` can fail, and a sort key can't use `?`, so read every
        // time first and sort the pairs.
        let mut found = Vec::new();
        for entry in entries {
            let entry = entry.with_context(|| format!("reading {}", self.root.display()))?;
            if let Some(session) = self.session_at(entry.path()) {
                found.push((session.modified_at()?, session));
            }
        }
        // Newest first. Ties (file clocks are coarse) go by ID, so the order
        // and `most_recent` are the same every time.
        found.sort_by(|(a_time, a), (b_time, b)| b_time.cmp(a_time).then_with(|| a.id.cmp(&b.id)));
        Ok(found.into_iter().map(|(_, session)| session).collect())
    }

    pub fn most_recent(&self) -> anyhow::Result<Option<Session>> {
        Ok(self.list()?.into_iter().next())
    }

    /// Delete a session: its known files one by one, then the empty
    /// directory. Never recursive, and it deletes nothing if the directory
    /// holds anything it doesn't recognize. It lives on `Sessions` because
    /// that's what knows the root; a `Session` shouldn't be handed its own
    /// root back.
    #[expect(
        clippy::disallowed_methods,
        reason = "the one place sessions are deleted"
    )]
    pub fn remove(&self, session: Session) -> anyhow::Result<()> {
        let Session { dir, .. } = session;
        // Redundant with how a `Session` is made, by design: it's cheap, and
        // being wrong here is expensive.
        if !self.is_session_dir(&dir) {
            bail!(
                "refusing to delete {}: not a session directory",
                dir.display()
            );
        }
        let table = dir.join(TABLE_FILE);
        for path in session_files(&dir)? {
            if path != table {
                fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
            }
        }
        // `table.json` goes last, and only if nothing arrived meanwhile (a
        // viewer writing `view.json`, say). Refusing here leaves a session
        // that's still listed, so `tv rm` can simply be run again.
        if session_files(&dir)? != [table.clone()] {
            bail!(
                "refusing to delete {}: it changed while being deleted; try again",
                dir.display()
            );
        }
        fs::remove_file(&table).with_context(|| format!("removing {}", table.display()))?;
        // Not recursive: if anything arrived in the last moment, this fails
        // with "directory not empty" instead of deleting it.
        fs::remove_dir(&dir).with_context(|| format!("removing {}", dir.display()))
    }

    /// The session at `dir`, if `dir` passes `is_session_dir`.
    fn session_at(&self, dir: PathBuf) -> Option<Session> {
        if !self.is_session_dir(&dir) {
            return None;
        }
        let id = dir.file_name()?.to_str()?.to_string();
        Some(Session { id, dir })
    }

    /// Whether `dir` is a session: a real directory (not a symlink) directly
    /// under the root, named with a session ID, holding `table.json`. `list`,
    /// `open` and `remove` all use this one check.
    fn is_session_dir(&self, dir: &Path) -> bool {
        let named_like_a_session = dir
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(is_session_id);
        named_like_a_session
            && dir.parent() == Some(self.root.as_path())
            && fs::symlink_metadata(dir).is_ok_and(|metadata| metadata.is_dir())
            && dir.join(TABLE_FILE).is_file()
    }
}

impl Session {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn load_table(&self) -> anyhow::Result<Table> {
        Table::from_path(self.dir.join(TABLE_FILE))
    }

    /// Replace the session's table. `Table::save` writes atomically, so a
    /// reader sees the old table or the new one, never a mix.
    pub fn write_table(&self, table: &Table) -> anyhow::Result<()> {
        table.save(&self.dir.join(TABLE_FILE))
    }

    /// The newest mtime of any file in the directory. Covers `comments.jsonl`
    /// and `view.json` once they exist, with no change here.
    pub fn modified_at(&self) -> anyhow::Result<SystemTime> {
        newest_mtime(&self.dir).with_context(|| format!("reading {}", self.dir.display()))
    }

    /// Title, row and column counts, and mtime for listings. Parses the
    /// table, so it fails if `table.json` doesn't.
    pub fn summary(&self) -> anyhow::Result<Summary> {
        let table = self.load_table()?;
        Ok(Summary {
            id: self.id.clone(),
            title: table.title,
            rows: table.rows.len(),
            columns: table.columns.len(),
            modified_at: self.modified_at()?,
        })
    }
}

/// Every entry in session directory `dir`, or an error, before anything is
/// deleted, if one of them isn't a file `remove` may delete.
fn session_files(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let reading = || format!("reading {}", dir.display());
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).with_context(reading)? {
        let entry = entry.with_context(reading)?;
        let name = entry.file_name();
        // `file_type` doesn't follow symlinks, so a link isn't a file here.
        let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
        if !is_file || !is_session_file(&name) {
            bail!(
                "refusing to delete {}: unexpected entry {}",
                dir.display(),
                name.to_string_lossy()
            );
        }
        files.push(entry.path());
    }
    Ok(files)
}

/// Whether a session directory may hold a file named `name`: one of
/// `SESSION_FILES`, or a `.{name}.{pid}.tmp` left by a `write_atomic` that
/// crashed before its rename. Nothing looser.
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "write_atomic only writes `.tmp`; matching more loosely would delete more"
)]
fn is_session_file(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        SESSION_FILES.contains(&name) || (name.starts_with('.') && name.ends_with(".tmp"))
    })
}

/// The newest mtime among `dir`'s entries, starting from the epoch so there's
/// no empty case. The directory's own mtime is left out: it changes whenever
/// an entry is added or renamed, which every write does, so it would hide the
/// file times.
fn newest_mtime(dir: &Path) -> io::Result<SystemTime> {
    let mut newest = SystemTime::UNIX_EPOCH;
    for entry in fs::read_dir(dir)? {
        newest = newest.max(entry?.metadata()?.modified()?);
    }
    Ok(newest)
}

/// Make the root and any missing parents. On Unix they're readable only by
/// you: a table piped in on stdin has no permissions of its own, and on a
/// machine with world-readable home directories other users could read it.
fn create_root(root: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(root)
}

/// A random session ID. IDs only need to avoid collisions, not resist
/// guessing. std seeds `RandomState`'s keys from the OS once per thread and
/// increments one of them on every `RandomState::new()`, so each call hashes
/// differently. That's what lets `create` just draw again on a collision.
fn random_id() -> String {
    let mut bits = RandomState::new().build_hasher().finish();
    let base = ID_ALPHABET.len() as u64;
    (0..ID_LEN)
        .map(|_| {
            let digit = usize::try_from(bits % base).expect("a digit is less than the base");
            bits /= base;
            char::from(ID_ALPHABET[digit])
        })
        .collect()
}

fn is_session_id(text: &str) -> bool {
    text.len() == ID_LEN && text.bytes().all(|byte| ID_ALPHABET.contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::time::Duration;

    fn table() -> Table {
        Table::parse(r#"{"title": "Things", "rows": [{"a": 1, "b": "x"}, {"a": 2, "b": "y"}]}"#)
            .unwrap()
    }

    /// A `Sessions` rooted in a fresh temp dir. Keep the `TempDir`: dropping
    /// it deletes the directory.
    fn sessions() -> (tempfile::TempDir, Sessions) {
        let dir = tempfile::tempdir().unwrap();
        let sessions = Sessions::new(dir.path().join("sessions"));
        (dir, sessions)
    }

    fn error_text(result: anyhow::Result<Session>) -> String {
        format!("{:#}", result.unwrap_err())
    }

    fn ids(sessions: &[Session]) -> Vec<&str> {
        sessions.iter().map(Session::id).collect()
    }

    fn set_table_mtime(session: &Session, time: SystemTime) {
        File::options()
            .write(true)
            .open(session.dir.join(TABLE_FILE))
            .unwrap()
            .set_modified(time)
            .unwrap();
    }

    #[test]
    fn create_writes_a_table_that_loads_back() {
        let (_dir, sessions) = sessions();
        let session = sessions.create(&table()).unwrap();
        assert!(is_session_id(session.id()), "{}", session.id());
        assert!(sessions.root.join(session.id()).join(TABLE_FILE).is_file());
        assert_eq!(session.load_table().unwrap().rows, table().rows);
    }

    #[test]
    fn two_creates_give_different_ids() {
        let (_dir, sessions) = sessions();
        let first = sessions.create(&table()).unwrap();
        let second = sessions.create(&table()).unwrap();
        assert_ne!(first.id(), second.id());
    }

    #[cfg(unix)]
    #[test]
    fn create_makes_the_root_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, sessions) = sessions();
        sessions.create(&table()).unwrap();
        let mode = fs::metadata(&sessions.root).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "{mode:o}");
    }

    #[test]
    fn claim_dir_skips_a_taken_id() {
        let (_dir, sessions) = sessions();
        fs::create_dir_all(sessions.root.join("taken1")).unwrap();
        assert_eq!(sessions.claim_dir("taken1").unwrap(), None);
        assert_eq!(
            sessions.claim_dir("free22").unwrap(),
            Some(sessions.root.join("free22"))
        );
    }

    #[test]
    fn open_finds_a_created_session() {
        let (_dir, sessions) = sessions();
        let created = sessions.create(&table()).unwrap();
        assert_eq!(sessions.open(created.id()).unwrap().id(), created.id());
    }

    #[test]
    fn open_refuses_missing_malformed_and_incomplete_sessions() {
        let (dir, sessions) = sessions();
        sessions.create(&table()).unwrap();
        fs::create_dir(sessions.root.join("empty1")).unwrap();
        for id in ["zzzzzz", "../x", "a/b", "empty1"] {
            let message = error_text(sessions.open(id));
            assert_eq!(message, format!("no session \"{id}\""));
        }
        // Nothing was created by the failed opens.
        assert!(!dir.path().join("x").exists());
        assert!(!sessions.root.join("a").exists());
    }

    #[test]
    fn list_of_a_missing_root_is_empty() {
        let (_dir, sessions) = sessions();
        assert!(sessions.list().unwrap().is_empty());
        assert!(sessions.most_recent().unwrap().is_none());
    }

    #[test]
    fn list_ignores_what_isnt_a_session() {
        let (_dir, sessions) = sessions();
        let session = sessions.create(&table()).unwrap();
        fs::write(sessions.root.join("stray1"), "not a dir").unwrap();
        fs::create_dir(sessions.root.join("empty1")).unwrap();
        assert_eq!(ids(&sessions.list().unwrap()), [session.id()]);
    }

    #[test]
    fn list_puts_the_most_recently_modified_first() {
        let (_dir, sessions) = sessions();
        let now = SystemTime::now();
        let hours_ago = |hours: u64| now - Duration::from_secs(hours * 3600);
        let middle = sessions.create(&table()).unwrap();
        let newest = sessions.create(&table()).unwrap();
        let oldest = sessions.create(&table()).unwrap();
        set_table_mtime(&middle, hours_ago(2));
        set_table_mtime(&newest, hours_ago(1));
        set_table_mtime(&oldest, hours_ago(3));
        assert_eq!(
            ids(&sessions.list().unwrap()),
            [newest.id(), middle.id(), oldest.id()]
        );
        assert_eq!(sessions.most_recent().unwrap().unwrap().id(), newest.id());
    }

    #[test]
    fn is_session_dir_checks_name_place_and_contents() {
        let (_dir, sessions) = sessions();
        let session = sessions.create(&table()).unwrap();
        assert!(sessions.is_session_dir(&session.dir));

        fs::write(sessions.root.join("file11"), "x").unwrap();
        let not_an_id = sessions.root.join("Not-An-ID");
        fs::create_dir(&not_an_id).unwrap();
        fs::write(not_an_id.join(TABLE_FILE), "{}").unwrap();
        fs::create_dir(sessions.root.join("empty1")).unwrap();
        for path in [
            sessions.root.clone(),
            sessions.root.join("file11"),
            not_an_id,
            sessions.root.join("empty1"),
        ] {
            assert!(!sessions.is_session_dir(&path), "{}", path.display());
        }
    }

    #[cfg(unix)]
    #[test]
    fn is_session_dir_refuses_a_symlink_to_a_session() {
        let (_dir, sessions) = sessions();
        let session = sessions.create(&table()).unwrap();
        let link = sessions.root.join("link11");
        std::os::unix::fs::symlink(&session.dir, &link).unwrap();
        assert!(!sessions.is_session_dir(&link));
    }

    #[test]
    fn remove_deletes_the_session_and_nothing_else() {
        let (_dir, sessions) = sessions();
        let target = sessions.create(&table()).unwrap();
        let sibling = sessions.create(&table()).unwrap();
        let stray = sessions.root.join("notes.txt");
        fs::write(&stray, "keep me").unwrap();
        let target_dir = target.dir.clone();
        sessions.remove(target).unwrap();
        assert!(!target_dir.exists());
        assert!(sessions.open(sibling.id()).is_ok());
        assert!(stray.exists());
    }

    #[test]
    fn remove_deletes_a_leftover_temp_file() {
        let (_dir, sessions) = sessions();
        let session = sessions.create(&table()).unwrap();
        fs::write(session.dir.join(".table.json.123.tmp"), "partial").unwrap();
        let dir = session.dir.clone();
        sessions.remove(session).unwrap();
        assert!(!dir.exists());
    }

    /// Make an entry called `name` in a new session's directory, then check
    /// that `remove` refuses and leaves both it and `table.json` in place.
    fn assert_remove_refuses(name: &str, make: impl Fn(&Path) -> io::Result<()>) {
        let (_dir, sessions) = sessions();
        let session = sessions.create(&table()).unwrap();
        let entry = session.dir.join(name);
        make(&entry).unwrap();
        let table_path = session.dir.join(TABLE_FILE);
        let message = format!("{:#}", sessions.remove(session).unwrap_err());
        assert!(message.contains("refusing to delete"), "{message}");
        assert!(
            message.contains(&format!("unexpected entry {name}")),
            "{message}"
        );
        assert!(entry.exists());
        assert!(table_path.is_file());
    }

    #[test]
    fn remove_refuses_an_unknown_file() {
        assert_remove_refuses("notes.txt", |path| fs::write(path, "mine"));
    }

    #[test]
    fn remove_refuses_a_subdirectory() {
        assert_remove_refuses("subdir", |path| fs::create_dir(path));
    }

    #[test]
    fn remove_refuses_a_session_from_another_root() {
        let (_one, mine) = sessions();
        let (_two, theirs) = sessions();
        let session = mine.create(&table()).unwrap();
        let dir = session.dir.clone();
        let message = format!("{:#}", theirs.remove(session).unwrap_err());
        assert!(message.contains("not a session directory"), "{message}");
        assert!(dir.join(TABLE_FILE).is_file());
    }

    #[test]
    fn summary_describes_the_table() {
        let (_dir, sessions) = sessions();
        let session = sessions.create(&table()).unwrap();
        let summary = session.summary().unwrap();
        assert_eq!(summary.id, session.id());
        assert_eq!(summary.title.as_deref(), Some("Things"));
        assert_eq!((summary.rows, summary.columns), (2, 2));
        assert_eq!(summary.modified_at, session.modified_at().unwrap());
    }

    #[test]
    fn from_home_appends_sessions() {
        let sessions = Sessions::from_home(Some("/x".into())).unwrap();
        assert_eq!(sessions.root, PathBuf::from("/x/sessions"));
        for unset in [None, Some(OsString::new())] {
            let sessions = Sessions::from_home(unset).unwrap();
            assert!(
                sessions.root.ends_with(".tv/sessions"),
                "{:?}",
                sessions.root
            );
        }
    }

    #[test]
    fn random_ids_are_well_formed() {
        for _ in 0..500 {
            let id = random_id();
            assert_eq!(id.len(), 6, "{id}");
            assert!(
                id.chars()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit()),
                "{id}"
            );
        }
    }
}
