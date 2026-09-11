//! APKMirror provider. APKMirror has no package-id index, so every entrypoint
//! starts from the site's own search (query = the Android package id), takes the
//! top hit, then walks: version list -> variants table -> download page ->
//! "starting" page -> APK URL.

mod parse;

use crate::common::contract::{
	AppResult, Arch, DownloadTarget, Provider, ProviderError, ProviderId, VersionInfo,
};
use crate::common::fetch::HttpFetcher;
use crate::providers::scrape::{choose_variant, query, version_matches};
use async_trait::async_trait;

const NAME: ProviderId = ProviderId::Apkmirror;

#[derive(Default)]
pub struct ApkMirror {
	fetcher: HttpFetcher,
}

impl ApkMirror {
	async fn search_hits(&self, q: &str) -> Result<Vec<parse::SearchHit>, ProviderError> {
		let url = format!(
			"{}/?post_type=app_release&searchtype=apk&s={}",
			parse::BASE_URL,
			query(q)
		);
		parse::parse_search(&self.fetcher.get_text(&url).await?, q)
	}

	/// Best-ranked hit: for a package id, the phone app's newest release.
	async fn top_hit(&self, pkg: &str) -> Result<parse::SearchHit, ProviderError> {
		let mut hits = self.search_hits(pkg).await?;
		Ok(hits.swap_remove(0))
	}
}

#[async_trait]
impl Provider for ApkMirror {
	fn id(&self) -> ProviderId {
		NAME
	}

	async fn search(&self, q: &str) -> Result<Vec<AppResult>, ProviderError> {
		// One row per release: keep the first (best-ranked, newest) of each app.
		let mut seen = std::collections::HashSet::new();
		Ok(self
			.search_hits(q)
			.await?
			.into_iter()
			.filter_map(|h| {
				// APKMirror identifier: "{org}/{repo}" (no Android pkg id on the page)
				let package = parse::app_slug(&h.release_url)?.to_string();
				seen.insert(package.clone()).then(|| AppResult {
					package,
					title: parse::strip_version(&h.title),
				})
			})
			.collect())
	}

	async fn versions(&self, pkg: &str) -> Result<Vec<VersionInfo>, ProviderError> {
		let hit = self.top_hit(pkg).await?;
		let slug = parse::app_slug(&hit.release_url)
			.ok_or_else(|| ProviderError::ParseError("bad release url".into()))?;
		let html = self
			.fetcher
			.get_text(&format!("{}/apk/{slug}/", parse::BASE_URL))
			.await?;
		parse::parse_versions(&html)
	}

	async fn download_url(
		&self,
		pkg: &str,
		version: Option<&str>,
		arch: Arch,
	) -> Result<DownloadTarget, ProviderError> {
		// 1. Locate the version page.
		let version_page = match version {
			None => self.top_hit(pkg).await?.release_url,
			Some(want) => {
				// Searching `{pkg} {version}` lands on the release page directly. The
				// app page's version list is paginated and drops older builds.
				self.search_hits(&format!("{pkg} {want}"))
					.await?
					.into_iter()
					.find(|h| version_matches(&h.version(), want))
					.ok_or_else(|| ProviderError::NotFound(format!("no build {want} for {pkg}")))?
					.release_url
			}
		};

		// 2. Variants table -> download page. If there's no table, we may have been
		//    redirected straight onto a download page (single-variant app).
		let version_html = self.fetcher.get_text(&version_page).await?;
		let variants = parse::parse_variants(&version_html);
		let (resolved_version, resolved_arch, download_page_html) = if variants.is_empty() {
			// Single-build app: the site lists no ABI, so it's a fat/universal APK.
			(version.map(str::to_string), Arch::all(), version_html)
		} else {
			let v = choose_variant(&variants, arch).ok_or_else(|| {
				ProviderError::NotFound(format!("no downloadable variant for {pkg}"))
			})?;
			(
				Some(v.version.clone()),
				v.arch,
				self.fetcher.get_text(&v.url).await?,
			)
		};

		// 3. download page -> "starting" page -> APK URL.
		let button_url = parse::parse_download_button(&download_page_html)?;
		let starting_html = self.fetcher.get_text(&button_url).await?;
		let apk_url = parse::parse_final_link(&starting_html)?;

		Ok(DownloadTarget {
			url: apk_url,
			version: resolved_version.unwrap_or_else(|| "latest".to_string()),
			arch: resolved_arch,
			provider: NAME,
			// APKMirror's download.php checks the referring download page.
			headers: vec![("Referer".to_string(), button_url)],
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
		Some(if url.contains("post_type=app_release") {
			"search"
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

	fn recording(app: &str) -> ApkMirror {
		ApkMirror {
			fetcher: HttpFetcher::recording(fixtures::root("apkmirror").join(app), fixture_name),
		}
	}

	#[tokio::test]
	#[ignore = "network"]
	async fn refresh_fixtures() {
		for (app, pkg) in fixtures::APPS {
			let p = recording(app);
			p.versions(pkg).await.unwrap();
			p.download_url(pkg, None, Arch::ARM64_V8A).await.unwrap();
		}
		// A bogus id only needs the (empty) search page.
		let err = recording("nonexistent")
			.search("com.example.does.not.exist.xyz")
			.await
			.unwrap_err();
		assert!(matches!(err, ProviderError::NotFound(_)), "{err}");
	}
}
