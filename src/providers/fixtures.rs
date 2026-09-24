//! `<provider>/tests/<app>/*.html` fixture plumbing shared by the offline
//! flow tests. Every file is captured by the *same* requests the provider
//! makes (see each provider's `refresh_fixtures`), so a recorded page is by
//! construction the one the code fetches; the flow tests then replay them
//! through `HttpFetcher::playback` and assert on the assembled target.
//!
//! For APKCombo the dir name *is* the search query (name-based provider); the
//! other two search by package id and treat it as a label.

use std::path::{Path, PathBuf};

use crate::common::contract::{Arch, Provider, ProviderConst};
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
	assert!(
		!path.starts_with('/'),
		"{app}: double slash after host: {url}"
	);
	assert!(!rest.contains("https://"), "{app}: repeated scheme: {url}");
}

/// Recapture one provider's `tests/<app>/*.html` by running its canonical
/// search/versions/download_url flow per app under a recording fetcher.
pub fn refresh_fixtures<P: Provider + ProviderConst>(
	fixture_name: FixtureName,
	build: impl Fn(HttpFetcher) -> P,
) {
	for (app, pkg) in APPS {
		let p = build(HttpFetcher::recording(
			root(P::ID.as_str()).join(app),
			fixture_name,
		));
		p.search(app)
			.unwrap_or_else(|e| panic!("{app}: search: {e}"));
		p.versions(pkg)
			.unwrap_or_else(|e| panic!("{app}: versions: {e}"));
		p.download_url(pkg, None, Arch::ARM64_V8A)
			.unwrap_or_else(|e| panic!("{app}: download_url: {e}"));
	}
}
