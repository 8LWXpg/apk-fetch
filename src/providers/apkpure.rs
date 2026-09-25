//! APKPure is package-id-addressable — `/x/{pkg}` resolves to the app page and
//! `d.apkpure.com/b/APK/{pkg}?versionCode=...` 302 straight to the APK.
//!
//! `arch` is accepted but not honored: APKPure's web endpoint serves one build
//! per app regardless of ABI so the resolved `arch` is always `Universal`.

mod parse;
use parse::Url;

use crate::common::contract::{
	AppResult, Arch, DownloadTarget, Provider, ProviderConst, ProviderError, ProviderId, VersionInfo,
};
use crate::common::fetch::HttpFetcher;
use crate::providers::scrape::{query, version_matches};

#[derive(Default)]
pub struct ApkPure {
	fetcher: HttpFetcher,
}

impl ApkPure {
	fn version_rows(&self, pkg: &str) -> Result<Vec<parse::VersionRow>, ProviderError> {
		let html = self
			.fetcher
			.get_text(Url::from(format!("/x/{pkg}/versions")).as_str())?;
		parse::parse_versions(&html)
	}
}

impl ProviderConst for ApkPure {
	const ID: ProviderId = ProviderId::Apkpure;
	const BASE_URL: &'static str = "https://apkpure.com";
}

impl Provider for ApkPure {
	fn id(&self) -> ProviderId {
		Self::ID
	}

	fn search(&self, q: &str) -> Result<Vec<AppResult>, ProviderError> {
		let html = self
			.fetcher
			.get_text(Url::from(format!("/search?q={}", query(q))).as_str())?;
		Ok(parse::parse_search(&html)?
			.into_iter()
			.map(|h| AppResult {
				package: h.package,
				title: h.title,
			})
			.collect())
	}

	fn versions(&self, pkg: &str) -> Result<Vec<VersionInfo>, ProviderError> {
		Ok(self
			.version_rows(pkg)?
			.into_iter()
			.map(|r| VersionInfo {
				version: r.version,
				uploaded: r.uploaded,
			})
			.collect())
	}

	fn download_url(&self, pkg: &str, version: Option<&str>, _arch: Arch) -> Result<DownloadTarget, ProviderError> {
		// `code` is APKPure `versionCode`, `None` means "latest".
		let (code, label) = match version {
			Some(want) => {
				let r = self
					.version_rows(pkg)?
					.into_iter()
					.find(|r| version_matches(&r.version, want))
					.ok_or_else(|| ProviderError::NotFound(format!("no version {want} for {pkg}")))?;
				(Some(r.code), r.version)
			}
			None => {
				let html = self.fetcher.get_text(Url::from(format!("/x/{pkg}")).as_str())?;
				let label = parse::latest_version(&html)
					.ok_or_else(|| ProviderError::NotFound(format!("no app page for {pkg}")))?;
				(None, label)
			}
		};

		Ok(DownloadTarget {
			url: parse::download_url(pkg, code.as_deref()),
			version: label,
			arch: Arch::all(),
			provider: Self::ID,
			headers: Vec::new(),
		})
	}
}

/// Fixture file for each URL the provider fetches.
#[cfg(test)]
fn fixture_name(url: &str) -> Option<&'static str> {
	Some(if url.contains("/search?q=") {
		"search"
	} else if url.ends_with("/versions") {
		"versions"
	} else {
		"app"
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
		fixtures::refresh_fixtures(fixture_name, |f| ApkPure { fetcher: f });
	}
}

/// Replays the whole resolution flow offline against the recorded fixtures,
/// diffing search/versions/download_url output against the `record.json` one.
#[cfg(test)]
mod tests {
	use super::*;
	use crate::providers::fixtures::{APPS, assert_absolute, assert_record, root, run_flow};

	#[test]
	fn resolves_from_fixtures() {
		for (app, pkg) in APPS {
			let dir = root("apkpure").join(app);
			let p = ApkPure {
				fetcher: HttpFetcher::playback(dir.clone(), fixture_name),
			};

			let record = run_flow(&p, app, pkg);
			assert_record(&dir, &record, app);

			assert_absolute(&record.target.url, app);
			assert!(
				record.target.url.starts_with("https://d.apkpure.com/b/APK/") && record.target.url.contains(pkg),
				"{app}: {}",
				record.target.url
			);
			assert!(record.target.arch == Arch::all());
			assert!(record.target.headers.is_empty(), "{app}: unexpected headers");
		}
	}
}
