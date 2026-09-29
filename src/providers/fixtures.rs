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

use colored::Colorize;
use serde::Serialize;
use serde_json::Value;
use versions::Versioning;

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
///
/// Run it with `--nocapture` and read the printed diffs.
pub fn refresh_fixtures<P: Provider + ProviderConst>(fixture_name: FixtureName, build: impl Fn(HttpFetcher) -> P) {
	for (app, pkg) in APPS {
		let dir = root(P::ID.as_str()).join(app);
		let record_path = dir.join("record.json");
		let previous: Value = fs::read_to_string(&record_path)
			.ok()
			.and_then(|s| serde_json::from_str(&s).ok())
			.unwrap_or(Value::Null);

		let p = build(HttpFetcher::recording(dir.clone(), fixture_name));
		let record = run_flow(&p, app, pkg);
		let new = serde_json::to_value(&record).unwrap();
		fs::write(&record_path, serde_json::to_string_pretty(&new).unwrap()).unwrap();

		print_diff(app, pkg, &previous, &new);
	}
}

/// `old`/`new` at a JSON pointer, `"-"` if either side is missing/not a string.
fn str_at<'a>(v: &'a Value, pointer: &str) -> &'a str {
	v.pointer(pointer).and_then(Value::as_str).unwrap_or("-")
}

fn hit_count(v: &Value) -> usize {
	v.pointer("/hits").and_then(Value::as_array).map_or(0, Vec::len)
}

fn line(label: &str, old: &str, new: &str) -> String {
	if old == new {
		format!("  {}: {new}", label.bold())
	} else {
		format!("  {}:\n  - {}\n  + {}", label.bold(), old.dimmed(), new.yellow())
	}
}

/// Display form of a download URL for the refresh diff.
fn url_shape(url: &str) -> String {
	let inner = url.split_once('?').and_then(|(_, q)| {
		form_urlencoded::parse(q.as_bytes())
			.find(|(k, _)| k == "u")
			.map(|(_, v)| v.into_owned())
	});
	let url = inner.unwrap_or_else(|| url.to_string());
	let Some((base, query)) = url.split_once('?') else {
		return url.clone();
	};

	let volatile = |k: &str| {
		k.starts_with("X-Amz-")
			|| k.starts_with("response-")
			|| ["key", "fp", "ip", "lang", "package_name"].contains(&k)
	};
	let kept: Vec<String> = form_urlencoded::parse(query.as_bytes())
		.filter(|(k, _)| !volatile(k))
		.map(|(k, v)| format!("{k}={v}"))
		.collect();
	if kept.is_empty() {
		base.to_string()
	} else {
		format!("{base}?{}", kept.join("&"))
	}
}

fn print_diff(app: &str, pkg: &str, old: &Value, new: &Value) {
	let (old_hits, new_hits) = (hit_count(old), hit_count(new));
	println!(
		"{}: {pkg}\n{}\n{}\n{}\n{}\n{}\n",
		app.cyan(),
		line("top hit", str_at(old, "/hits/0/title"), str_at(new, "/hits/0/title")),
		line("hit count", &old_hits.to_string(), &new_hits.to_string()),
		line(
			"latest",
			str_at(old, "/versions/0/version"),
			str_at(new, "/versions/0/version")
		),
		line(
			"target version",
			str_at(old, "/target/version"),
			str_at(new, "/target/version")
		),
		line(
			"target url",
			&url_shape(str_at(old, "/target/url")),
			&url_shape(str_at(new, "/target/url"))
		),
	);
}

/// Cheap structural checks.
pub fn assert_invariants(fixture: &FixtureRecord, _pkg: &str, app: &str) {
	assert!(!fixture.versions.is_empty(), "{app}: no versions returned");
	assert!(
		fixture
			.versions
			.windows(2)
			.all(|w| Versioning::new(&w[0].version) >= Versioning::new(&w[1].version)),
		"{app}: versions not sorted newest-first (by version)"
	);
	let mut seen = std::collections::HashSet::new();
	assert!(
		fixture.versions.iter().all(|v| seen.insert(&v.version)),
		"{app}: duplicate version in list"
	);

	assert_eq!(
		fixture.target.version,
		fixture.versions.first().unwrap().version,
		"{app}: download target isn't the latest version in the versions list"
	);

	assert_absolute(&fixture.target.url, app);
}
