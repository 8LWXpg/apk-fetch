//! APKPure provider. APKPure is package-id-addressable — `/x/{pkg}` resolves to
//! the app page and `d.apkpure.com/b/APK/{pkg}?version=...` 302s straight to the
//! APK — so there's no search-walk to reach a download.
//!
//! `arch` is accepted but not honoured: APKPure's web endpoint serves one build
//! per app regardless of ABI, so resolved `arch` is left `None`.

mod parse;

use provider::{AppResult, DownloadTarget, Provider, ProviderError, VersionInfo};
use fetch::{Fetcher, HttpFetcher};
use async_trait::async_trait;

const NAME: &str = "apkpure";

pub struct ApkPure {
    fetcher: HttpFetcher,
}

impl ApkPure {
    pub fn new() -> Self {
        Self {
            fetcher: HttpFetcher::new(),
        }
    }
}

impl Default for ApkPure {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for ApkPure {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn search(&self, query: &str) -> Result<Vec<AppResult>, ProviderError> {
        let url = format!("{}/search?q={}", parse::BASE_URL, query.trim().replace(' ', "+"));
        let html = self.fetcher.get_text(&url).await?;
        Ok(parse::parse_search(&html)?
            .into_iter()
            .map(|h| AppResult {
                package: h.package,
                title: h.title,
                developer: h.developer,
                provider: NAME.to_string(),
            })
            .collect())
    }

    async fn versions(&self, pkg: &str) -> Result<Vec<VersionInfo>, ProviderError> {
        let url = format!("{}/x/{}/versions", parse::BASE_URL, pkg);
        let html = self.fetcher.get_text(&url).await?;
        Ok(parse::parse_versions(&html)?
            .into_iter()
            .map(|r| VersionInfo {
                version: r.version,
                version_code: r.version_code,
                uploaded: None,
                provider: NAME.to_string(),
            })
            .collect())
    }

    async fn download_url(
        &self,
        pkg: &str,
        version: Option<&str>,
        _arch: &str,
    ) -> Result<DownloadTarget, ProviderError> {
        // `endpoint_version` is what we hand apkpure; `label` is for the filename.
        // When no version is pinned we ask for "latest" (a specific version string
        // isn't always URL-safe) but still resolve the real one for the name — the
        // app-page GET also 404s -> NotFound for an unknown package.
        let (endpoint_version, label) = match version {
            Some(v) => (v.to_string(), v.to_string()),
            None => {
                let html = self
                    .fetcher
                    .get_text(&format!("{}/x/{}", parse::BASE_URL, pkg))
                    .await?;
                let label = parse::latest_version(&html).ok_or(ProviderError::NotFound)?;
                ("latest".to_string(), label)
            }
        };

        Ok(DownloadTarget {
            filename: provider::download_filename(pkg, &label, None, "apk"),
            url: parse::download_url(pkg, &endpoint_version),
            version: Some(label),
            arch: None,
            provider: NAME.to_string(),
            headers: Vec::new(),
        })
    }
}
