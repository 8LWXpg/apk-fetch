//! APKCombo download flow:
//!   search -> old-versions page -> download page -> POST variant frag -> POST `/checkin`
//!   -> final = `{BASE}{r2_href}&{checkin}&package_name={pkg}&lang=en` -> 302 -> CDN

mod parse;
use parse::Url;

use crate::common::contract::{
	AppResult, Arch, DownloadTarget, Provider, ProviderConst, ProviderError, ProviderId,
	VersionInfo,
};
use crate::common::fetch::HttpFetcher;
use crate::providers::scrape::{choose_variant, query, version_matches};

#[derive(Default)]
pub struct ApkCombo {
	fetcher: HttpFetcher,
}

impl ApkCombo {
	/// `/en/{pkg}/` 301 to the `/{slug}/{pkg}/`.
	fn slug_for(&self, pkg: &str) -> Result<String, ProviderError> {
		let final_url: Url = self
			.fetcher
			.resolve_url(parse::app_url(parse::LOOKUP_LOCALE, pkg, "").as_str())?
			.into();
		parse::slug_from_canonical_url(&final_url, pkg)
			.ok_or_else(|| ProviderError::NotFound(format!("no app page for {pkg}")))
	}

	fn version_rows(&self, slug: &str, pkg: &str) -> Result<Vec<parse::VersionRow>, ProviderError> {
		let html = self
			.fetcher
			.get_text(parse::app_url(slug, pkg, "old-versions").as_str())?;
		parse::parse_versions(&html)
	}
}

impl ProviderConst for ApkCombo {
	const ID: ProviderId = ProviderId::Apkcombo;
	const BASE_URL: &'static str = "https://apkcombo.com";
}

impl Provider for ApkCombo {
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
		let slug = self.slug_for(pkg)?;
		Ok(self
			.version_rows(&slug, pkg)?
			.into_iter()
			.map(|r| VersionInfo {
				version: r.version,
				uploaded: r.uploaded,
			})
			.collect())
	}

	fn download_url(
		&self,
		pkg: &str,
		version: Option<&str>,
		arch: Arch,
	) -> Result<DownloadTarget, ProviderError> {
		let slug = self.slug_for(pkg)?;

		// Locate the download page (carries the `xid` build tag).
		let dl_page = match version {
			None => parse::app_url(&slug, pkg, "download/phone-latest-apk"),
			Some(want) => self
				.version_rows(&slug, pkg)?
				.into_iter()
				.find(|r| version_matches(&r.version, want))
				.map(|r| r.download_page_url)
				.ok_or_else(|| ProviderError::NotFound(format!("no build {want} for {pkg}")))?,
		};
		let xid = parse::extract_xid(&self.fetcher.get_text(dl_page.as_str())?);

		// POST the variant fragment.
		let frag = self.fetcher.post_form(
			parse::app_url(&slug, pkg, &format!("{xid}/dl")).as_str(),
			&[("package_name", pkg), ("version", version.unwrap_or(""))],
		)?;
		let variants = parse::parse_variants(&frag)?;
		let variant = choose_variant(&variants, arch)
			.ok_or_else(|| ProviderError::NotFound(format!("no downloadable variant for {pkg}")))?;

		// Checkin token.
		let checkin = self
			.fetcher
			.post_form(Url::from("/checkin").as_str(), &[])?;

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

/// Fixture file for each URL the provider fetches.
#[cfg(test)]
fn fixture_name(url: &str) -> Option<&'static str> {
	Some(if url.contains("/search?q=") {
		"search"
	} else if url.ends_with("/old-versions") {
		"old-versions"
	} else if url.contains("/download/phone-") {
		"download-page"
	} else if url.ends_with("/dl") {
		"variants"
	} else if url.contains("/en/") {
		"redirect"
	} else if url.ends_with("/checkin") {
		"checkin"
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
		fixtures::refresh_fixtures(fixture_name, |f| ApkCombo { fetcher: f });
	}
}

/// Replays the whole resolution flow offline against the recorded fixtures,
/// asserting on the assembled `DownloadTarget`.
#[cfg(test)]
mod tests {
	use super::*;
	use crate::providers::fixtures::{APPS, assert_absolute, root};

	#[test]
	fn resolves_from_fixtures() {
		for (app, pkg) in APPS {
			let p = ApkCombo {
				fetcher: HttpFetcher::playback(root("apkcombo").join(app), fixture_name),
			};

			let hits = p
				.search(app)
				.unwrap_or_else(|e| panic!("{app}: search: {e}"));
			assert!(!hits.is_empty(), "{app}: empty search");
			assert!(
				hits.iter().any(|h| h.package == pkg),
				"{app}: {pkg} missing from search"
			);

			let vers = p
				.versions(pkg)
				.unwrap_or_else(|e| panic!("{app}: versions: {e}"));
			assert!(!vers.is_empty(), "{app}: no versions");
			assert!(
				vers.iter().any(|v| v.version.contains('.')),
				"{app}: no dotted versions"
			);

			let t = p
				.download_url(pkg, None, Arch::ARM64_V8A)
				.unwrap_or_else(|e| panic!("{app}: download_url: {e}"));
			assert_absolute(&t.url, app);
			assert!(
				t.url.contains(pkg) && t.url.contains("package_name="),
				"{app}: {}",
				t.url
			);
			assert!(!t.version.is_empty(), "{app}: empty version");
		}
	}
}
