//! Aptoide ws75 JSON API (no scraping), standalone APKs only:
//!   `apps/search?query=`            -> one hit per uploader store
//!   `listAppVersions?package_name=` -> builds uploaded by third-party stores, no download link;
//!                                      the CDN URL is derived, see `App::apk_url`
//!
//! Deliberately not used: `app/getMeta`, which serves the `appupdater` store's current build as a
//! base APK + split APKs and ignores any ABI/device parameter. Stores are user-run, so a build's
//! signature is not vouched for here; verify it downstream.
mod parse;
use parse::Url;

use crate::common::contract::{
	AppResult, Arch, DownloadTarget, Provider, ProviderConst, ProviderError, ProviderId, VersionInfo,
};
use crate::common::fetch::HttpFetcher;
use crate::providers::scrape::{query, version_matches};

#[derive(Default)]
pub struct Aptoide {
	fetcher: HttpFetcher,
}

impl Aptoide {
	fn version_list(&self, pkg: &str) -> Result<Vec<parse::App>, ProviderError> {
		parse::parse_versions(
			&self
				.fetcher
				.get_text(Url::from(format!("listAppVersions?package_name={pkg}")).as_str())?,
		)
	}
}

impl ProviderConst for Aptoide {
	const ID: ProviderId = ProviderId::Aptoide;
	const BASE_URL: &'static str = "https://ws75.aptoide.com/api/7";
}

impl Provider for Aptoide {
	fn id(&self) -> ProviderId {
		Self::ID
	}

	fn search(&self, q: &str) -> Result<Vec<AppResult>, ProviderError> {
		parse::parse_search(
			&self
				.fetcher
				.get_text(Url::from(format!("apps/search?query={}&limit=20", query(q))).as_str())?,
		)
	}

	fn versions(&self, pkg: &str) -> Result<Vec<VersionInfo>, ProviderError> {
		Ok(self
			.version_list(pkg)?
			.into_iter()
			.map(|a| {
				Ok(VersionInfo {
					uploaded: a.uploaded()?,
					version: a.file.vername,
				})
			})
			.collect::<Result<Vec<_>, ProviderError>>()?)
	}

	fn download_url(&self, pkg: &str, version: Option<&str>, arch: Arch) -> Result<DownloadTarget, ProviderError> {
		// Newest first; take the first build that runs on `arch`.
		let mut cands = self.version_list(pkg)?;
		if let Some(want) = version {
			cands.retain(|a| version_matches(&a.file.vername, want));
		}
		let app = cands.into_iter().find(|a| a.arch().contains(arch)).ok_or_else(|| {
			ProviderError::no_match(format!("no {arch} build for {pkg}"), version.unwrap_or("latest"))
		})?;

		Ok(DownloadTarget {
			arch: app.arch(),
			url: app.apk_url().to_string(),
			version: app.file.vername,
			provider: Self::ID,
			headers: Vec::new(),
		})
	}
}

/// Fixture file for each URL the provider fetches.
#[cfg(test)]
fn fixture_name(url: &str) -> Option<&'static str> {
	Some(if url.contains("/apps/search?") {
		"search"
	} else if url.contains("/listAppVersions?") {
		"versions"
	} else {
		return None;
	})
}

/// `cargo test refresh_fixtures -- --ignored`
#[cfg(test)]
mod refresh {
	use super::*;
	use crate::providers::fixtures;

	#[test]
	#[ignore = "network"]
	fn refresh_fixtures() {
		fixtures::refresh_fixtures(fixture_name, |f| Aptoide { fetcher: f });
	}
}

/// Replays the whole resolution flow offline against the recorded fixtures,
/// diffing search/versions/download_url output against the `record.json` one.
#[cfg(test)]
mod tests {
	use super::*;
	use crate::providers::fixtures::{APPS, assert_invariants, assert_record, root, run_flow};

	#[test]
	fn resolves_from_fixtures() {
		for (app, pkg) in APPS {
			let dir = root("aptoide").join(app);
			let p = Aptoide {
				fetcher: HttpFetcher::playback(dir.clone(), fixture_name),
			};

			let record = run_flow(&p, app, pkg);
			assert_record(&dir, &record, app);
			assert_invariants(&record, pkg, app);

			assert!(
				record.target.url.starts_with("https://pool.apk.aptoide.com/")
					&& record.target.url.contains(&pkg.replace('.', "-")),
				"{app}: {}",
				record.target.url
			);
			assert!(record.target.headers.is_empty(), "{app}: unexpected headers");
		}
	}
}
