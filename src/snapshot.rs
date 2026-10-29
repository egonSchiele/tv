//! Snapshot tests without a snapshot crate: compare rendered text to a file
//! committed under `src/viewer/snapshots/`.

use std::path::PathBuf;

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
