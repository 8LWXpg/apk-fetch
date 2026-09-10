//! Fixture-directory plumbing shared by the providers' parser tests. Every
//! provider stores saved HTML the same way — `tests/<app>/*.html`, one directory
//! per app, refreshed by that crate's `refresh-fixtures.sh` — so the walk and the
//! read live here instead of three times over.
//!
//! A dev-dependency of its sibling provider crates: nothing here ships. (Named
//! `fixtures`, not `test` — a crate called `test` shadows the sysroot `test` that
//! the `#[test]` macro expands into, and nothing compiles.)

use std::fs;
use std::path::{Path, PathBuf};

/// Every `<manifest_dir>/tests/<app>/` directory holding a non-empty
/// `search.html`, sorted by name. Adding a directory extends a provider's
/// coverage with no code change.
///
/// Call as `app_dirs(env!("CARGO_MANIFEST_DIR"))` — `env!` has to expand in the
/// calling crate to name that crate's own fixtures.
pub fn app_dirs(manifest_dir: &str) -> Vec<(String, PathBuf)> {
    let root = Path::new(manifest_dir).join("tests");
    let mut dirs: Vec<(String, PathBuf)> = fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("{}: {e}", root.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && p.join("search.html").metadata().is_ok_and(|m| m.len() > 0))
        .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), p))
        .collect();
    dirs.sort();
    assert!(!dirs.is_empty(), "no tests/<app>/ fixture dirs in {root:?}");
    dirs
}

/// One fixture file's contents. Panics with the full path — a missing fixture is
/// a broken checkout or a stale `refresh-fixtures.sh`, not a test failure to
/// puzzle over.
pub fn read(dir: &Path, name: &str) -> String {
    fs::read_to_string(dir.join(name))
        .unwrap_or_else(|e| panic!("{}: {e}", dir.join(name).display()))
}
