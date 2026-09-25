//! `<provider>/tests/<app>/` fixture plumbing shared by the offline flow
//! tests. Every file is captured by the *same* requests the provider makes
//! (see each provider's `refresh_fixtures`), so a recorded page is by
//! construction the one the code fetches. The flow tests replay them through
//! `HttpFetcher::playback` and diff against the `record.json` file written at
//! refresh time.
//!
//! For APKCombo the dir name *is* the search query (name-based provider); the
//! other two search by package id and treat it as a label.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::common::contract::{AppResult, Arch, DownloadTarget, Provider, ProviderConst, VersionInfo};
use crate::common::fetch::{FixtureName, HttpFetcher};

/// `(dir name, package id)` captured by every provider's `refresh_fixtures`.
pub const APPS: [(&str, &str); 2] = [
	("youtube", "com.google.android.youtube"),
	("youtube-music", "com.google.android.apps.youtube.music"),
];

/// `src/providers/<provider>/tests/`.
pub fn root(provider: &str) -> PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR"))
		.join("src/providers")
		.join(provider)
		.join("tests")
}

/// Assert if `url` is an absolute `https://` URL with a plausible host
pub fn assert_absolute(url: &str, app: &str) {
	let rest = url
		.strip_prefix("https://")
		.unwrap_or_else(|| panic!("{app}: not absolute https: {url}"));

	let authority = rest.split('/').next().unwrap_or_default();
	assert!(
		authority.split('.').count() >= 2,
		"{app}: host looks wrong: {authority}"
	);

	let path = rest.split_once('/').map(|x| x.1).unwrap_or_default();
	assert!(!path.starts_with('/'), "{app}: double slash after host: {url}");
	assert!(!rest.contains("https://"), "{app}: repeated scheme: {url}");
}

/// Everything the canonical flow returns for one app: what search, versions
/// and download_url produce on the recorded pages.
#[derive(Serialize)]
pub struct FixtureRecord {
	pub hits: Vec<AppResult>,
	pub versions: Vec<VersionInfo>,
	pub target: DownloadTarget,
}

/// Run one app's canonical flow (search by app name, versions, and download by
/// package id) and return the outputs.
pub fn run_flow<P: Provider + ProviderConst>(p: &P, app: &str, pkg: &str) -> FixtureRecord {
	FixtureRecord {
		hits: p.search(app).unwrap_or_else(|e| panic!("{app}: search: {e}")),
		versions: p.versions(pkg).unwrap_or_else(|e| panic!("{app}: versions: {e}")),
		target: p
			.download_url(pkg, None, Arch::ARM64_V8A)
			.unwrap_or_else(|e| panic!("{app}: download_url: {e}")),
	}
}

/// Diff a freshly replayed [`FixtureRecord`] against the `record.json` one.
pub fn assert_record(dir: &Path, fixture: &FixtureRecord, app: &str) {
	let file = dir.join("record.json");
	let recorded = fs::read_to_string(&file).unwrap_or_else(|e| panic!("{app}: {}: {e}", file.display()));
	let recorded: Value = serde_json::from_str(&recorded).unwrap();
	assert_eq!(
		serde_json::to_value(fixture).unwrap(),
		recorded,
		"{app}: drifted from record.json"
	);
}

/// Recapture one provider's `tests/<app>/` — `*.html` pages *and* the `record.json`.
pub fn refresh_fixtures<P: Provider + ProviderConst>(fixture_name: FixtureName, build: impl Fn(HttpFetcher) -> P) {
	for (app, pkg) in APPS {
		let dir = root(P::ID.as_str()).join(app);
		let p = build(HttpFetcher::recording(dir.clone(), fixture_name));
		let record = run_flow(&p, app, pkg);
		fs::write(dir.join("record.json"), serde_json::to_string_pretty(&record).unwrap()).unwrap();
	}
}
