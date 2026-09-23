//! APKPure is package-id-addressable — `/x/{pkg}` resolves to the app page and
//! `d.apkpure.com/b/APK/{pkg}?versionCode=...` 302 straight to the APK.
//!
//! `arch` is accepted but not honored: APKPure's web endpoint serves one build
//! per app regardless of ABI so the resolved `arch` is always `Universal`.

mod parse;
use parse::Url;

use crate::common::contract::{
	AppResult, Arch, DownloadTarget, Provider, ProviderConst, ProviderError, ProviderId,
	VersionInfo,
};
use crate::common::fetch::HttpFetcher;
use crate::providers::scrape::{query, version_matches};
use async_trait::async_trait;

#[derive(Default)]
pub struct ApkPure {
	fetcher: HttpFetcher,
}

impl ApkPure {
	async fn version_rows(&self, pkg: &str) -> Result<Vec<parse::VersionRow>, ProviderError> {
		let html = self
			.fetcher
			.get_text(Url::from(format!("/x/{pkg}/versions")).as_str())
			.await?;
		parse::parse_versions(&html)
	}
}

impl ProviderConst for ApkPure {
	const ID: ProviderId = ProviderId::Apkpure;
	const BASE_URL: &'static str = "https://apkpure.com";
}

#[async_trait]
impl Provider for ApkPure {
	fn id(&self) -> ProviderId {
		Self::ID
	}

	async fn search(&self, q: &str) -> Result<Vec<AppResult>, ProviderError> {
		let html = self
			.fetcher
			.get_text(Url::from(format!("/search?q={}", query(q))).as_str())
			.await?;
		Ok(parse::parse_search(&html)?
			.into_iter()
			.map(|h| AppResult {
				package: h.package,
				title: h.title,
			})
			.collect())
	}

	async fn versions(&self, pkg: &str) -> Result<Vec<VersionInfo>, ProviderError> {
		Ok(self
			.version_rows(pkg)
			.await?
			.into_iter()
			.map(|r| VersionInfo {
				version: r.version,
				uploaded: r.uploaded,
			})
			.collect())
	}

	async fn download_url(
		&self,
		pkg: &str,
		version: Option<&str>,
		_arch: Arch,
	) -> Result<DownloadTarget, ProviderError> {
		// `code` is APKPure `versionCode`, `None` means "latest".
		let (code, label) = match version {
			Some(want) => {
				let r = self
					.version_rows(pkg)
					.await?
					.into_iter()
					.find(|r| version_matches(&r.version, want))
					.ok_or_else(|| {
						ProviderError::NotFound(format!("no version {want} for {pkg}"))
					})?;
				(Some(r.code), r.version)
			}
			None => {
				let html = self
					.fetcher
					.get_text(Url::from(format!("/x/{pkg}")).as_str())
					.await?;
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

	#[tokio::test]
	#[ignore = "network"]
	async fn refresh_fixtures() {
		fixtures::refresh_fixtures(fixture_name, |f| ApkPure { fetcher: f }).await;
	}
}

/// Replays the whole resolution flow offline against the recorded fixtures,
/// asserting on the assembled `DownloadTarget` rather than per-parser output.
#[cfg(test)]
mod tests {
	use super::*;
	use crate::providers::fixtures::{APPS, assert_absolute, root};

	#[tokio::test]
	async fn resolves_from_fixtures() {
		for (app, pkg) in APPS {
			let p = ApkPure {
				fetcher: HttpFetcher::playback(root("apkpure").join(app), fixture_name),
			};

			let hits = p
				.search(app)
				.await
				.unwrap_or_else(|e| panic!("{app}: search: {e}"));
			assert!(!hits.is_empty(), "{app}: empty search");
			for h in &hits {
				assert!(h.package.contains('.'), "{app}: {}", h.package);
				assert!(!h.title.trim().is_empty(), "{app}: blank title");
			}

			let vers = p
				.versions(pkg)
				.await
				.unwrap_or_else(|e| panic!("{app}: versions: {e}"));
			assert!(!vers.is_empty(), "{app}: no versions");
			assert!(
				vers.iter()
					.all(|v| v.version.contains('.') && !v.version.contains('(')),
				"{app}: malformed version"
			);

			let t = p
				.download_url(pkg, None, Arch::ARM64_V8A)
				.await
				.unwrap_or_else(|e| panic!("{app}: download_url: {e}"));
			assert_absolute(&t.url, app);
			assert!(
				t.url.starts_with("https://d.apkpure.com/b/APK/") && t.url.contains(pkg),
				"{app}: {}",
				t.url
			);
			assert!(t.version.contains('.'), "{app}: {}", t.version);
			assert_eq!(t.arch, Arch::all());
			assert!(t.headers.is_empty(), "{app}: unexpected headers");
		}
	}
}
