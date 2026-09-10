//! `<provider>/tests/<app>/*.html` fixture plumbing shared by the parser tests.

use std::fs;
use std::path::{Path, PathBuf};

/// Every `src/providers/<provider>/tests/<app>/` holding a non-empty `search.html`.
pub fn app_dirs(provider: &str) -> Vec<(String, PathBuf)> {
	let root = Path::new(env!("CARGO_MANIFEST_DIR"))
		.join("src/providers")
		.join(provider)
		.join("tests");
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

pub fn read(dir: &Path, name: &str) -> String {
	fs::read_to_string(dir.join(name))
		.unwrap_or_else(|e| panic!("{}: {e}", dir.join(name).display()))
}
