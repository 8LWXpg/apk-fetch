//! APKMirror provider. APKMirror has no package-id index, so every entrypoint
//! starts from the site's own search (query = the Android package id), takes the
//! top hit, then walks: version list -> variants table -> download page ->
//! "starting" page -> APK URL.

mod parse;

use crate::common::contract::{
	AppResult, DownloadTarget, Provider, ProviderError, ProviderId, VersionInfo,
};
use crate::common::fetch::{Fetcher, HttpFetcher};
use async_trait::async_trait;

const NAME: ProviderId = ProviderId::Apkmirror;

pub struct ApkMirror {
	fetcher: HttpFetcher,
}

impl ApkMirror {
	pub fn new() -> Self {
		Self {
			fetcher: HttpFetcher::new(),
		}
	}

	fn search_url(query: &str) -> String {
		// ponytail: spaces only. Real form-encoding when a query needs `&`/`#`.
		format!(
			"{}/?post_type=app_release&searchtype=apk&s={}",
			parse::BASE_URL,
			query.trim().replace(' ', "+")
		)
	}

	/// search -> first hit that isn't a beta/alpha channel (unless the query asked
	/// for one).
	async fn top_release_url(&self, pkg: &str) -> Result<String, ProviderError> {
		let html = self.fetcher.get_text(&Self::search_url(pkg)).await?;
		let hits = parse::parse_search(&html)?;
		let wants_prerelease = ["beta", "alpha", "dev", "canary"]
			.iter()
			.any(|k| pkg.contains(k));
		let pick = hits
			.iter()
			.find(|h| {
				wants_prerelease
					|| !["-beta", "-alpha", "-dev", "-canary"]
						.iter()
						.any(|k| h.release_url.contains(k))
			})
			.or_else(|| hits.first())
			.ok_or_else(|| ProviderError::NotFound(format!("search matched no app for {pkg:?}")))?;
		Ok(pick.release_url.clone())
	}
}

impl Default for ApkMirror {
	fn default() -> Self {
		Self::new()
	}
}

#[async_trait]
impl Provider for ApkMirror {
	fn id(&self) -> ProviderId {
		NAME
	}

	async fn search(&self, query: &str) -> Result<Vec<AppResult>, ProviderError> {
		let html = self.fetcher.get_text(&Self::search_url(query)).await?;
		let hits = parse::parse_search(&html)?;
		let mut results: Vec<AppResult> = hits
			.into_iter()
			.filter_map(|h| {
				let package = parse::app_page_from_release(&h.release_url)?
					.trim_start_matches(parse::BASE_URL)
					.trim_matches('/')
					.trim_start_matches("apk/")
					.to_string();
				let (title, version) = parse::split_title_version(&h.title);
				Some(AppResult {
					package, // APKMirror identifier: "{org}/{repo}" (no Android pkg id on the page)
					title,
					version,
					developer: None,
					provider: NAME,
				})
			})
			.collect();
		// APKMirror ranks on a blind substring match and lists one row per
		// release, so re-rank by query relevance (stable) and drop repeat apps.
		results.sort_by_key(|r| parse::relevance(&r.title, query));
		let mut seen = std::collections::HashSet::new();
		results.retain(|r| seen.insert(r.package.clone()));
		Ok(results)
	}

	async fn versions(&self, pkg: &str) -> Result<Vec<VersionInfo>, ProviderError> {
		let release_url = self.top_release_url(pkg).await?;
		let app_page = parse::app_page_from_release(&release_url)
			.ok_or_else(|| ProviderError::ParseError("bad release url".into()))?;
		let html = self.fetcher.get_text(&app_page).await?;
		let rows = parse::parse_versions(&html)?;
		Ok(rows
			.into_iter()
			.map(|r| VersionInfo {
				version: parse::version_token(&r.title),
				uploaded: r.uploaded,
				provider: NAME,
			})
			.collect())
	}

	async fn download_url(
		&self,
		pkg: &str,
		version: Option<&str>,
		arch: &str,
	) -> Result<DownloadTarget, ProviderError> {
		// 1. Locate the version page.
		let version_page = match version {
			None => self.top_release_url(pkg).await?,
			Some(want) => {
				let release_url = self.top_release_url(pkg).await?;
				let app_page = parse::app_page_from_release(&release_url)
					.ok_or_else(|| ProviderError::ParseError("bad release url".into()))?;
				let html = self.fetcher.get_text(&app_page).await?;
				let rows = parse::parse_versions(&html)?;
				rows.into_iter()
					.find(|r| r.title.contains(want) || parse::version_token(&r.title) == want)
					.ok_or_else(|| ProviderError::NotFound(format!("no build {want} for {pkg}")))?
					.version_page_url
			}
		};

		// 2. Variants table -> download page. If there's no table, we may have been
		//    redirected straight onto a download page (single-variant app).
		let version_html = self.fetcher.get_text(&version_page).await?;
		let variants = parse::parse_variants(&version_html);
		let (resolved_version, resolved_arch, download_page_html) = if variants.is_empty() {
			(version.map(str::to_string), None, version_html)
		} else {
			let v = parse::choose_variant(&variants, arch).ok_or_else(|| {
				ProviderError::NotFound(format!("no downloadable variant for {pkg}"))
			})?;
			let arch_token = v
				.arch
				.split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
				.find(|t| !t.is_empty())
				.map(str::to_string);
			(
				Some(v.version.clone()),
				arch_token,
				self.fetcher.get_text(&v.download_page_url).await?,
			)
		};

		// 3. download page -> "starting" page -> APK URL.
		let button_url = parse::parse_download_button(&download_page_html)?;
		let starting_html = self.fetcher.get_text(&button_url).await?;
		let apk_url = parse::parse_final_link(&starting_html)?;

		let version_label = resolved_version.unwrap_or_else(|| "latest".to_string());
		Ok(DownloadTarget {
			url: apk_url,
			version: Some(version_label),
			arch: resolved_arch,
			provider: NAME,
			// APKMirror's download.php checks the referring download page.
			headers: vec![("Referer".to_string(), button_url)],
		})
	}
}
