//! Snapshot tests without a snapshot crate: compare rendered text to a file
//! committed under `src/viewer/snapshots/`.

use std::fmt::Write;
use std::path::PathBuf;

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use unicode_width::UnicodeWidthStr;

/// Compare `actual` to `src/viewer/snapshots/<name>.txt`. With
/// `UPDATE_SNAPSHOTS=1` (`make snapshots`), write the file instead and pass.
pub fn assert_snapshot(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/viewer/snapshots")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let Ok(expected) = std::fs::read_to_string(&path) else {
        panic!(
            "no snapshot at {}; run `make snapshots` to create it",
            path.display()
        );
    };
    // Not `assert_eq!`: it would print each side as one long escaped string.
    assert!(
        actual == expected,
        "snapshot `{name}` changed.\n\n--- expected\n{expected}\n--- actual\n{actual}\n\
         If the change is intended, run `make snapshots` and review it with `git diff`."
    );
}

/// A buffer as text for snapshots: each line in quotes (so trailing spaces
/// show), then a `--- styles` section listing each line's runs of non-default
/// style, like `row 2: 1..9 REVERSED | BOLD`. `TestBackend`'s own text has no
/// styles, so without this a highlight change would not show up in a diff.
pub fn buffer_view(buffer: &Buffer) -> String {
    let width = usize::from(buffer.area.width).max(1);
    let mut text = String::new();
    let mut styles = String::new();
    for (y, cells) in buffer.content.chunks(width).enumerate() {
        // A wide character's symbol covers the cells after it. Those hidden
        // cells are skipped in the text and take the character's style, since
        // that's how the terminal draws them.
        let mut descriptions = Vec::with_capacity(cells.len());
        let mut hidden = 0;
        text.push('"');
        for cell in cells {
            if hidden == 0 {
                text.push_str(cell.symbol());
                hidden = cell.symbol().width().max(1);
                descriptions.push(describe(cell.fg, cell.bg, cell.modifier));
            } else {
                descriptions.push(descriptions.last().cloned().unwrap_or_default());
            }
            hidden -= 1;
        }
        text.push_str("\"\n");

        let runs = style_runs(descriptions.into_iter());
        if !runs.is_empty() {
            // Writing to a String can't fail.
            writeln!(styles, "row {y}: {}", runs.join(", ")).unwrap();
        }
    }
    format!("{text}--- styles\n{styles}")
}

/// `start..end description` for each run of equal, non-default styles.
fn style_runs(descriptions: impl Iterator<Item = String>) -> Vec<String> {
    let mut runs: Vec<(usize, usize, String)> = Vec::new();
    for (x, description) in descriptions.enumerate() {
        match runs.last_mut() {
            Some((_, end, last)) if *end == x && *last == description => *end = x + 1,
            _ if description.is_empty() => {}
            _ => runs.push((x, x + 1, description)),
        }
    }
    runs.into_iter()
        .map(|(start, end, description)| format!("{start}..{end} {description}"))
        .collect()
}

/// Empty for the default style.
fn describe(fg: Color, bg: Color, modifier: Modifier) -> String {
    let mut parts = Vec::new();
    if fg != Color::Reset {
        parts.push(format!("fg={fg:?}"));
    }
    if bg != Color::Reset {
        parts.push(format!("bg={bg:?}"));
    }
    if !modifier.is_empty() {
        parts.push(format!("{modifier:?}"));
    }
    parts.join(" ")
}
