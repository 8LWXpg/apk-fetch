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

/// Recaptures `tests/<app>/*.html` through the provider's own requests, so a
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
