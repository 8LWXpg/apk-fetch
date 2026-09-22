//! APKCombo download flow:
//!   search              -> `/{slug}/{pkg}/`
//!   old-versions page   -> `/{slug}/{pkg}/old-versions`      (version list)
//!   download page       -> `/{slug}/{pkg}/download/phone-{v}-apk`  (carries `xid`)
//!   POST variant frag   -> `/{slug}/{pkg}/{xid}/dl`   (form: package_name, version)
//!   POST `/checkin`     -> `fp=...&ip=...` token
//!   final = `{BASE}{r2_href}&{checkin}&package_name={pkg}&lang=en`  -> 302 -> CDN

mod parse;
use parse::Url;

use crate::common::contract::{
	AppResult, Arch, DownloadTarget, Provider, ProviderConst, ProviderError, ProviderId,
	VersionInfo,
};
use crate::common::fetch::HttpFetcher;
use crate::providers::scrape::{choose_variant, query, version_matches};
use async_trait::async_trait;

#[derive(Default)]
pub struct ApkCombo {
	fetcher: HttpFetcher,
}

impl ApkCombo {
	/// `/en/{pkg}/` 301 to the `/{slug}/{pkg}/`.
	async fn slug_for(&self, pkg: &str) -> Result<String, ProviderError> {
		let final_url: Url = self
			.fetcher
			.resolve_url(parse::app_url(parse::LOOKUP_LOCALE, pkg, "").as_str())
			.await?
			.into();
		parse::slug_from_canonical_url(&final_url, pkg)
			.ok_or_else(|| ProviderError::NotFound(format!("no app page for {pkg}")))
	}

	async fn version_rows(
		&self,
		slug: &str,
		pkg: &str,
	) -> Result<Vec<parse::VersionRow>, ProviderError> {
		let html = self
			.fetcher
			.get_text(parse::app_url(slug, pkg, "old-versions").as_str())
			.await?;
		parse::parse_versions(&html)
	}
}

impl ProviderConst for ApkCombo {
	const ID: ProviderId = ProviderId::Apkcombo;
	const BASE_URL: &'static str = "https://apkcombo.com";
}

#[async_trait]
impl Provider for ApkCombo {
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
		let slug = self.slug_for(pkg).await?;
		Ok(self
			.version_rows(&slug, pkg)
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
		arch: Arch,
	) -> Result<DownloadTarget, ProviderError> {
		let slug = self.slug_for(pkg).await?;

		// Locate the download page (carries the `xid` build tag).
		let dl_page = match version {
			None => parse::app_url(&slug, pkg, "download/phone-latest-apk"),
			Some(want) => self
				.version_rows(&slug, pkg)
				.await?
				.into_iter()
				.find(|r| version_matches(&r.version, want))
				.map(|r| r.download_page_url)
				.ok_or_else(|| ProviderError::NotFound(format!("no build {want} for {pkg}")))?,
		};
		let xid = parse::extract_xid(&self.fetcher.get_text(dl_page.as_str()).await?);

		// POST the variant fragment.
		let frag = self
			.fetcher
			.post_form(
				parse::app_url(&slug, pkg, &format!("{xid}/dl")).as_str(),
				&[("package_name", pkg), ("version", version.unwrap_or(""))],
			)
			.await?;
		let variants = parse::parse_variants(&frag)?;
		let variant = choose_variant(&variants, arch)
			.ok_or_else(|| ProviderError::NotFound(format!("no downloadable variant for {pkg}")))?;

		// Checkin token, then decorate the r2 link.
		let checkin = self
			.fetcher
			.post_form(Url::from("/checkin").as_str(), &[])
			.await?;

		Ok(DownloadTarget {
			url: parse::final_download_url(&variant.url, &checkin, pkg),
			version: if variant.version.is_empty() {
				version.unwrap_or("latest").to_string()
			} else {
				variant.version.clone()
			},
			arch: variant.arch,
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

	/// Fixture file for each URL the provider fetches; `/checkin` isn't kept.
	fn fixture_name(url: &str) -> Option<&'static str> {
		Some(if url.contains("/search?q=") {
			"search"
		} else if url.ends_with("/old-versions") {
			"old-versions"
		} else if url.contains("/download/phone-") {
			"download-page"
		} else if url.ends_with("/dl") {
			"variants"
		} else {
			return None;
		})
	}

	#[tokio::test]
	#[ignore = "network"]
	async fn refresh_fixtures() {
		for (app, pkg) in fixtures::APPS {
			let p = ApkCombo {
				fetcher: HttpFetcher::recording(fixtures::root("apkcombo").join(app), fixture_name),
			};
			// APKCombo search is name-based.
			p.search(app).await.unwrap();
			p.versions(pkg).await.unwrap();
			p.download_url(pkg, None, Arch::ARM64_V8A).await.unwrap();
		}
	}
}
