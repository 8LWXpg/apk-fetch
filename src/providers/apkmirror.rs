//! APKMirror has no package-id index, so every entry point starts from the site's
//! own search (query = the Android package id), takes the top hit, then walks:
//! version list -> variants table -> download page -> "starting" page -> APK URL.

mod parse;
use parse::Url;

use crate::common::contract::{
	AppResult, Arch, DownloadTarget, Provider, ProviderConst, ProviderError, ProviderId,
	VersionInfo,
};
use crate::common::fetch::HttpFetcher;
use crate::providers::scrape;

const NAME: ProviderId = ProviderId::Apkmirror;

#[derive(Default)]
pub struct ApkMirror {
	fetcher: HttpFetcher,
}

impl ApkMirror {
	fn base_search(&self, arg: &str, q: &str) -> Result<Vec<parse::SearchHit>, ProviderError> {
		let url: Url = format!("/?post_type=app_release&{arg}&s={}", scrape::query(q)).into();
		parse::parse_search(&self.fetcher.get_text(url.as_str())?, q)
	}

	fn apk_search(&self, q: &str) -> Result<Vec<parse::SearchHit>, ProviderError> {
		self.base_search("searchtype=apk", q)
	}

	fn app_search(&self, q: &str) -> Result<Vec<parse::SearchHit>, ProviderError> {
		self.base_search("searchtype=app", q)
	}

	/// Best-ranked hit: for a package id, the phone app's newest release.
	fn top_app_hit(&self, pkg: &str) -> Result<parse::SearchHit, ProviderError> {
		let hits = self.app_search(pkg)?;
		parse::latest_version(&self.fetcher.get_text(hits[0].release_url.as_str())?)
	}
}

impl ProviderConst for ApkMirror {
	const ID: ProviderId = ProviderId::Apkmirror;
	const BASE_URL: &'static str = "https://www.apkmirror.com";
}

impl Provider for ApkMirror {
	fn id(&self) -> ProviderId {
		Self::ID
	}

	fn search(&self, q: &str) -> Result<Vec<AppResult>, ProviderError> {
		// One row per release: keep the first (best-ranked, newest) of each app.
		Ok(self
			.app_search(q)?
			.into_iter()
			.filter_map(|h| {
				// APKMirror identifier: "{org}/{repo}" (no Android pkg id on the page)
				let package = parse::app_slug(&h.release_url)?.to_string();
				Some(AppResult {
					package: percent_encoding::percent_decode_str(&package)
						.decode_utf8_lossy()
						.into(),
					title: parse::strip_version(&h.title),
				})
			})
			.collect())
	}

	fn versions(&self, pkg: &str) -> Result<Vec<VersionInfo>, ProviderError> {
		let hit = self.top_app_hit(pkg)?;
		let slug = parse::app_slug(&hit.release_url)
			.ok_or_else(|| ProviderError::ParseError("bad release url".into()))?;
		let html = self
			.fetcher
			.get_text(Url::from(format!("/apk/{slug}/")).as_str())?;
		parse::parse_versions(&html)
	}

	fn download_url(
		&self,
		pkg: &str,
		version: Option<&str>,
		arch: Arch,
	) -> Result<DownloadTarget, ProviderError> {
		// 1. Locate the version page.
		let version_page = match version {
			None => self.top_app_hit(pkg)?.release_url,
			Some(want) => {
				// Searching `{pkg} {version}` lands on the release page directly. The
				// app page's version list is paginated and drops older builds.
				self.apk_search(&format!("{pkg} {want}"))?
					.into_iter()
					.find(|h| scrape::version_matches(&h.version(), want))
					.ok_or_else(|| ProviderError::NotFound(format!("no build {want} for {pkg}")))?
					.release_url
			}
		};

		// 2. Variants table -> download page. If there's no table, we may have been
		//    redirected straight onto a download page (single-variant app).
		let version_html = self.fetcher.get_text(version_page.as_str())?;
		let variants = parse::parse_variants(&version_html);
		let (resolved_version, resolved_arch, download_page_html) = if variants.is_empty() {
			// Single-build app: the site lists no ABI, so it's a universal APK.
			(version.map(str::to_string), Arch::all(), version_html)
		} else {
			let v = scrape::choose_variant(&variants, arch).ok_or_else(|| {
				ProviderError::NotFound(format!("no downloadable variant for {pkg}"))
			})?;
			(
				Some(v.version.clone()),
				v.arch,
				self.fetcher.get_text(&v.url)?,
			)
		};

		// 3. Download page -> "starting" page -> APK URL.
		let button_url = parse::parse_download_button(&download_page_html)?;
		let starting_html = self.fetcher.get_text(button_url.as_str())?;
		let apk_url = parse::parse_final_link(&starting_html)?;

		Ok(DownloadTarget {
			url: apk_url.into_string(),
			version: resolved_version.unwrap_or_else(|| "latest".to_string()),
			arch: resolved_arch,
			provider: NAME,
			// APKMirror `download.php` checks the referring download page.
			headers: vec![("Referer".to_string(), button_url.into_string())],
		})
	}
}

/// Fixture file for each URL the provider fetches ([`fixture_name`] also maps
/// what `resolve_url` is asked for, so redirects replay too).
#[cfg(test)]
fn fixture_name(url: &str) -> Option<&'static str> {
	Some(if url.contains("post_type=app_release") {
		// `s=` last parameter: package ids contain a dot, names don't
		if url.split("&s=").nth(1).is_some_and(|q| q.contains('.')) {
			"search-pkg"
		} else {
			"search"
		}
	} else if url.contains("/download/?key=") {
		"download-starting"
	} else if url.ends_with("-android-apk-download/") {
		"download-page"
	} else if url.ends_with("-release/") {
		"version"
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
		fixtures::refresh_fixtures(fixture_name, |f| ApkMirror { fetcher: f });
	}
}

/// Replays the whole resolution flow offline against the recorded fixtures:
/// every URL the code fetches goes through [`fixture_name`] into a saved page.
/// The assertions target the assembled `DownloadTarget`, so a mangled URL —
/// the bug class that a per-parser test sails past — fails here.
#[cfg(test)]
mod tests {
	use super::*;
	use crate::providers::fixtures::{APPS, assert_absolute, root};

	#[test]
	fn resolves_from_fixtures() {
		for (app, pkg) in APPS {
			let p = ApkMirror {
				fetcher: HttpFetcher::playback(root("apkmirror").join(app), fixture_name),
			};

			let hits = p
				.search(app)
				.unwrap_or_else(|e| panic!("{app}: search: {e}"));
			assert!(!hits.is_empty(), "{app}: empty search");
			assert!(
				hits.iter().any(|h| !h.title.trim().is_empty()),
				"{app}: blank title"
			);

			let vers = p
				.versions(pkg)
				.unwrap_or_else(|e| panic!("{app}: versions: {e}"));
			assert!(!vers.is_empty(), "{app}: no versions");
			assert!(
				vers.iter().any(|v| v.version.contains('.')
					&& v.version.starts_with(|c: char| c.is_ascii_digit())),
				"{app}: no release-shaped versions"
			);

			let t = p
				.download_url(pkg, None, Arch::ARM64_V8A)
				.unwrap_or_else(|e| panic!("{app}: download_url: {e}"));
			assert_absolute(&t.url, app);
			assert!(
				t.url.contains("download.php") || t.url.contains("downloadr"),
				"{app}: {}",
				t.url
			);
			assert!(!t.version.is_empty(), "{app}: empty version");
			assert_eq!(t.provider, ProviderId::Apkmirror);
			assert!(
				t.headers.iter().any(|(k, _)| k == "Referer"),
				"{app}: no Referer"
			);
		}
	}
}
