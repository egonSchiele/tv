# Plan: ADI-200 Data layer: load from file, save with :w

Linear: https://linear.app/aditya-bhargava/issue/ADI-200
Sub-issues: ADI-217 (load from a path or stdin), ADI-218 (save with `:w`, `s`).
Depends on: ADI-199 merged.

## Goal

- `tv file.json`, `tv open file.json` and `cat file.json | tv -` all open the viewer.
- In the viewer, `:w [path]` writes the table (with comments) as JSON, safely: write to a temp file, then rename. `s` opens the same command, prefilled with the default path.
- Load → save → load gives back the same table.

## Context for the executing agent

- Read `SPEC.md`: "Table format" (`comments` is only present in saved files), "Human CLI", "Viewer / Keys" (`s`), and "Changes after review" (the command line).
- Build on what's there; don't duplicate it:
  - `Table::from_path` / `Table::parse` load and validate.
  - `Table` already serializes to the full format; inferred columns and types are written out.
  - `ColonCommand::Write` and its parser exist, and `run_command` shows a "not implemented yet" message for it.
  - The command line (`Mode::Command`) is the only text input in the viewer.
- **No new crates.** `std` covers stdin, `~` expansion, slugs, temp files and rename. See "Dependencies" in README.md.
- **Two commits, one per sub-issue, each passing `make check`.** This is a binary crate, so anything a commit adds but doesn't use trips rustc's `dead_code` lint, and `make lint` runs with `-D warnings`. That's why all of ADI-218 (the file helpers, `Table::save` and the viewer wiring) is one commit: the helpers have no caller until `:w` uses them.
- **Tests that write files use a `tempfile::TempDir`**, never the working directory or the repo.
- **SPEC.md's module layout** lists atomic writes under `session.rs`. They go in `files.rs` (below) so sessions and `:w` share them; update the SPEC layout in the ADI-218 commit so the two agree.

## Moved out of this ticket: `tv save <id> <file.json>`

ADI-200 and ADI-218 list `tv save <id> <file.json>`, but `<id>` is a session ID, and sessions arrive in ADI-201. Move it to ADI-222, next to `tv ls` and `tv rm`. The same goes for ADI-217's file-or-session rule ("a file if it contains `.` or `/`"): until sessions exist, every argument is a file. `files::write_atomic` (below) is written so `session.rs` can reuse it.

## Part 1: Load from a path or stdin (ADI-217)

### CLI

```rust
#[derive(Parser)]
#[command(name = "tv", version, args_conflicts_with_subcommands = true, arg_required_else_help = true)]
struct Cli {
    /// A JSON file to open, or `-` to read stdin. Same as `tv open <path>`.
    path: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}
```

- **One path out of the parse.** `main` turns the two forms into one value up front: `Some(Command::Open { path })` gives that `path`, and `None` gives `cli.path`. Nothing after that cares which form was used.
- **Bare `tv`** prints help (`arg_required_else_help`). The spec's "open the most recent session" comes with sessions.
- **Subcommands win.** `tv open` and the later `tv ls` are parsed as subcommands, not as file names. Check this with `Cli::try_parse_from` tests, not by reading clap's docs.

### Loading

`-` meaning stdin is a CLI convention, so it lives in `main.rs`, not in `table.rs`. `Table` keeps only `parse` and `from_path`:

```rust
fn load(source: &str) -> anyhow::Result<Table> {
    if source == "-" {
        let text = std::io::read_to_string(std::io::stdin()).context("reading stdin")?;
        return Table::parse(&text).context("parsing stdin");
    }
    Table::from_path(source)
}
```

`main` calls only `load`, so the "is it stdin?" check lives in one place. `tv open -` works too, through the same path.

Reading stdin happens before the TUI starts, so a parse error still exits 1 without touching the screen. When stdin is a pipe, crossterm reads keys from `/dev/tty` instead (checked in crossterm 0.29's `tty_fd`). Confirm that in the manual check below.

### Tests

- `Cli::try_parse_from`:
  - `tv x.json` → path `x.json`.
  - `tv open x.json` → path `x.json`.
  - `tv -` → path `-`.
  - `tv open -` → path `-`.
  - `tv` → error that shows help.
  - `tv open` with no path → error.
- `tests/cli.rs` (the start of ADI-229's harness), using `std::process::Command` on `env!("CARGO_BIN_EXE_tv")`. Only cases that never start the TUI:
  - `tv -` with `not json` on stdin exits 1, and stderr contains `parsing stdin`.
  - `tv missing.json` exits 1, and stderr contains `reading missing.json`.

## Part 2: Save with `:w` and `s` (ADI-218)

### `src/files.rs`

```rust
/// Write `contents` to `path` so readers see the old file or the new one,
/// never a partial write: write a temp file in the same directory, flush it
/// to disk, then rename it over `path`.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()>;

/// `~` and `~/rest` become the home directory (`std::env::home_dir`).
/// Anything else, including `~user`, is returned unchanged.
pub fn expand_tilde(path: &str) -> PathBuf;

/// Longest slug `slug` produces, so a long title can't make an unwieldy file name.
pub const SLUG_MAX_LEN: usize = 50;

/// `text` as a file-name-safe slug: ASCII letters (lowercased) and digits are
/// kept, runs of anything else become one `-`, leading and trailing `-` are
/// trimmed, and the result is cut to `SLUG_MAX_LEN`. Can be empty.
pub fn slug(text: &str) -> String;
```

How `write_atomic` works:

- **Name:** the temp file goes next to the target, named `.{file name}.{pid}.tmp`, and is created with `OpenOptions::create_new` so it never clobbers an existing file. The same directory matters, because `rename` is only atomic within one filesystem.
- **Write:** `write_all`, then `sync_all`, then `rename`.
- **On failure:** remove the temp file. Only remove the exact path this call created; never a pattern or a directory (see "Deleting files without a guard").
- **Missing directory:** the call fails with the plain `io::Error`. Don't create directories. `std::io::Error` doesn't include the path, so `write_atomic` doesn't try to; `Table::save` adds it as context, the same way `from_path` does.

### Saving the table

`Table::save(&self, path: &Path) -> anyhow::Result<()>`: `serde_json::to_string_pretty(self)`, plus a trailing newline, written with `write_atomic` and `.with_context(|| format!("writing {}", path.display()))`, mirroring `from_path`. So a failed save reads `save failed: writing /x/a.json: No such file or directory`. Comments are kept as they were loaded (they're opaque `Value`s until ADI-223).

The default file name is `files::slug(title)` with `.json`; `"Open pull requests"` → `open-pull-requests.json`. The fallback `table` (no title, or a slug that comes out empty) is applied in `App::default_save_path`, not in `slug` or `Table`: the slug is a plain string function and `Table` stays about the format.

### Viewer behavior

`ColonCommand::Write(Option<String>)` becomes `Write { path: Option<String>, force: bool }`. `from_str` strips one trailing `!` from the command name before the match and passes it as `force`, so `("w" | "write", path)` stays one arm and `q!` gets the same treatment later for free. Until then a `!` on anything but `w`/`write` falls through to `Unknown`. `HELP` lists `:w [path]` ("save; default ./<title>.json, also after opening a file") and `:w! [path]` ("save, overwriting a file").

`App` keeps `save_path: Option<PathBuf>`: the current file in vim's sense, set after `expand_tilde` on a successful save, shown with `.display()`. It starts as `None` even when the table came from `tv file.json`: the SPEC's `s` defaults to `./<title-slug>.json`, and once `tv open file.json` creates a session the file isn't "current" anyway. That's deliberate; don't "fix" it. The rules:

| Command | Writes to | If the file exists |
|---|---|---|
| `:w` | `save_path`, or `./<title-slug>.json` if nothing was saved yet | allowed if it's `save_path`; otherwise refused |
| `:w path` | `path` (after `expand_tilde`) | refused unless it's `save_path` |
| `:w! [path]` | as above | overwritten |

When a save is refused, the message is `{path} exists; use :w! to overwrite`. It's vim's rule: re-saving your own file is fine, but clobbering a different one takes a `!`. "Is it `save_path`?" compares `fs::canonicalize` of both paths, so `./a.json` and `a.json` match. The comparison only runs when the target exists (`canonicalize` fails on a missing path, and a missing path needs no check). This is the one place `App` reasons about the filesystem; it belongs here because it's policy, not IO.

**On success:** set `save_path` and show `wrote {rows} rows to {path}`. **On failure:** show `save failed: {error:#}`. The viewer stays open either way.

**`s`:** a `GRID_BINDINGS` row with `Action::Save`. It opens `Mode::Command` with the text `w {default}` and the cursor at the end, so Enter saves and the path can be edited first. Add `CommandLine::new(text)` for this. The default is the same "`save_path`, else the slug" rule that `:w` uses, from one `App::default_save_path()`. There's no second input mode, and the help overlay picks up `s` from the table.

All of the save logic is one `App::save(path: Option<String>, force: bool)`, which `run_command` calls. The IO itself is in `Table::save` and `files`; `App` only decides the path and the message.

### Tests

- **`files`:**
  - `write_atomic` creates a file, then overwrites it.
  - The directory has no leftover `.tmp` files afterwards.
  - Writing into a missing directory returns `Err` and creates nothing.
  - `expand_tilde` with `~`, `~/x`, `a/~b` and `~user/x`.
  - `slug`: `"Open pull requests"` → `open-pull-requests`; `"Q3: Revenue — by region!"` → `q3-revenue-by-region`; a title with no ASCII letters or digits gives `""`; a long title is cut to `SLUG_MAX_LEN`.
- **`default_save_path`:** no title, and an all-punctuation title, both give `./table.json`.
- **Round trip:**
  - Load `examples/prs.json`, save it, load it again, and compare title, key, columns and rows.
  - Do the same for a shorthand array. Assert that the saved JSON has `columns` with a `type` on every one, since that's the visible difference between input and output.
  - Do the same for a table with a `comments` array (comments survive).
- **`App`**, all inside a temp dir:
  - `:w {dir}/a.json` writes the file and shows the message.
  - A second plain `:w` writes `a.json` again.
  - `:w {dir}/b.json` where `b.json` already exists is refused and leaves `b.json` unchanged.
  - `:w! {dir}/b.json` overwrites it.
  - `s` opens the command line with `w ./open-pull-requests.json` on a fresh app, and with `w {dir}/a.json` after a save. Don't press Enter on the `./` default, because that would write into the repo.
  - A save into a missing directory shows `save failed: writing {path}: …` and keeps the viewer open.
- **Snapshots:** none for saving. The success message contains a temp-dir path, so it can't be snapshotted; `status_message.txt` already covers a message in the status line, and the `App` tests assert the text. `help.txt` does change (`s`, `:w!`, the new `:w` text): run `make snapshots` and review the diff in the ADI-218 commit.

## Manual check

Every step runs in a scratch directory, on a copy of the example, so nothing here can touch the repo even if the refusal logic is broken:

```sh
mkdir -p /tmp/tv-check && cd /tmp/tv-check && cp ~/tv/examples/prs.json .
```

1. `cat prs.json | tv -`. The grid appears, the keys work, and `q` exits cleanly. This confirms the `/dev/tty` fallback.
2. `tv prs.json`, then `s` and Enter. `./open-pull-requests.json` is written, and the status line confirms it.
3. `:w` again succeeds. `:w prs.json` is refused with `prs.json exists; use :w! to overwrite`. `:w! prs.json` overwrites the copy.
4. `tv open-pull-requests.json` opens the saved file and looks the same as the original.

## Acceptance

```sh
make check
cat examples/prs.json | cargo run -q -- -           # viewer opens; q exits 0
echo 'not json' | cargo run -q -- -; echo $?        # Error: parsing stdin … ; 1
```

Plus the manual check above.

## Deliverables

- `src/main.rs`: optional positional path, one `path` out of the parse, `load` (stdin or file)
- `src/table.rs`: `save`
- `src/files.rs`: `write_atomic`, `expand_tilde`, `slug`, `SLUG_MAX_LEN`
- `src/viewer/app.rs`: `ColonCommand::Write { path, force }`, `Action::Save`, `save_path`, `default_save_path`, `save`, `CommandLine::new`
- `src/viewer/snapshots/help.txt`: regenerated
- `SPEC.md`: module layout lists `files.rs`
- `tests/cli.rs`
- Two commits: `cli: open a path without "open", or stdin with -` (ADI-217) and `save: :w, :w!, and s with atomic writes` (ADI-218)

## Out of scope

- `tv save <id>` and the file-or-session rule: sessions, ADI-222 and ADI-219.
- Editing cells in the viewer: not planned. The table only changes through agent pushes.
- Saving view state such as the cursor: that's `view.json` in sessions (ADI-220).
