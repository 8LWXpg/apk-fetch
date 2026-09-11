//! APKPure provider. APKPure is package-id-addressable — `/x/{pkg}` resolves to
//! the app page and `d.apkpure.com/b/APK/{pkg}?versionCode=...` 302s straight to
//! the APK — so there's no search-walk to reach a download.
//!
//! `arch` is accepted but not honoured: APKPure's web endpoint serves one build
//! per app regardless of ABI — nearly always a universal APK — so the resolved
//! `arch` is always `Universal`.

mod parse;

use crate::common::contract::{
	AppResult, Arch, DownloadTarget, Provider, ProviderError, ProviderId, VersionInfo,
};
use crate::common::fetch::HttpFetcher;
use crate::providers::scrape::{query, version_matches};
use async_trait::async_trait;

const NAME: ProviderId = ProviderId::Apkpure;

#[derive(Default)]
pub struct ApkPure {
	fetcher: HttpFetcher,
}

impl ApkPure {
	async fn version_rows(&self, pkg: &str) -> Result<Vec<parse::VersionRow>, ProviderError> {
		let html = self
			.fetcher
			.get_text(&format!("{}/x/{pkg}/versions", parse::BASE_URL))
			.await?;
		parse::parse_versions(&html)
	}
}

#[async_trait]
impl Provider for ApkPure {
	fn id(&self) -> ProviderId {
		NAME
	}

	async fn search(&self, q: &str) -> Result<Vec<AppResult>, ProviderError> {
		let html = self
			.fetcher
			.get_text(&format!("{}/search?q={}", parse::BASE_URL, query(q)))
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
		// `code` is what we hand apkpure (its `versionCode`; `None` means "latest");
		// `label` is for the filename. A pinned version is looked up in the versions
		// list both to get that code and so a typo / missing build fails over
		// instead of silently downloading "latest". Unpinned: ask for "latest" but
		// resolve the real number for the name. Either GET also 404s -> NotFound for
		// an unknown package.
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
					.get_text(&format!("{}/x/{pkg}", parse::BASE_URL))
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
			provider: NAME,
			headers: Vec::new(),
		})
	}
}

/// Re-captures `tests/<app>/*.html` through the provider's own requests, so a
/// fixture is by construction the page the code fetches:
/// `cargo test refresh_fixtures -- --ignored`. Trims the diff-heavy noise;
/// check the diff, then `cargo test`.
#[cfg(test)]
mod refresh {
	use super::*;
	use crate::providers::fixtures;

	/// Fixture file for each URL the provider fetches.
	fn fixture_name(url: &str) -> Option<&'static str> {
		Some(if url.contains("/search?q=") {
			"search"
		} else if url.ends_with("/versions") {
			"versions"
		} else {
			"app"
		})
	}

	#[tokio::test]
	#[ignore = "network"]
	async fn refresh_fixtures() {
		for (app, pkg) in fixtures::APPS {
			let p = ApkPure {
				fetcher: HttpFetcher::recording(fixtures::root("apkpure").join(app), fixture_name),
			};
			p.search(pkg).await.unwrap();
			p.versions(pkg).await.unwrap();
			p.download_url(pkg, None, Arch::ARM64_V8A).await.unwrap();
		}
	}
}
