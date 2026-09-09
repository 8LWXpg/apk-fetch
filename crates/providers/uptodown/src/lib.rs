//! Uptodown provider. `search` and `versions` work over plain HTTP. The APK
//! download endpoint is gated behind Cloudflare Turnstile (a captcha), which a
//! headless HTTP client can't solve — so `download_url` returns
//! `ProviderError::Blocked` and the fallback resolver moves on.
//!
//! Uptodown is keyed by per-app subdomains derived from the app *name*, with no
//! package-id index, so resolving a package id means: search by name fragments of
//! the id, then confirm each candidate's app page carries the exact package id.

mod parse;

use contract::{AppResult, DownloadTarget, Provider, ProviderError, VersionInfo};
use fetch::{Fetcher, HttpFetcher};
use async_trait::async_trait;

const NAME: &str = "uptodown";
/// How many search candidates to check app pages for when resolving a package id.
const MAX_CANDIDATES: usize = 6;

pub struct Uptodown {
    fetcher: HttpFetcher,
}

impl Uptodown {
    pub fn new() -> Self {
        Self { fetcher: HttpFetcher::new() }
    }

    fn search_url(query: &str) -> String {
        format!("{}?query={}", parse::SEARCH_URL, query.trim().replace(' ', "+"))
    }

    /// package id -> (app base URL, numeric app code), by searching name fragments
    /// of the id and confirming the package on candidate app pages.
    async fn resolve(&self, pkg: &str) -> Result<(String, String), ProviderError> {
        let mut checked = std::collections::HashSet::new();
        for term in parse::search_terms(pkg) {
            let html = match self.fetcher.get_text(&Self::search_url(&term)).await {
                Ok(h) => h,
                Err(ProviderError::NotFound) => continue,
                Err(e) => return Err(e),
            };
            let Ok(hits) = parse::parse_search(&html) else { continue };
            for hit in hits.into_iter().take(MAX_CANDIDATES) {
                if !checked.insert(hit.base.clone()) {
                    continue;
                }
                let app_html = self
                    .fetcher
                    .get_text(&format!("{}/android", hit.base))
                    .await?;
                if parse::app_package(&app_html).as_deref() == Some(pkg) {
                    let code = parse::app_code(&app_html).unwrap_or(hit.code);
                    return Ok((hit.base, code));
                }
            }
        }
        Err(ProviderError::NotFound)
    }
}

impl Default for Uptodown {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for Uptodown {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn search(&self, query: &str) -> Result<Vec<AppResult>, ProviderError> {
        let html = self.fetcher.get_text(&Self::search_url(query)).await?;
        Ok(parse::parse_search(&html)?
            .into_iter()
            .map(|h| AppResult {
                // Uptodown search rows carry no package id; the subdomain is its
                // identifier.
                package: h
                    .base
                    .trim_start_matches("https://")
                    .split('.')
                    .next()
                    .unwrap_or(&h.base)
                    .to_string(),
                title: h.name,
                developer: None,
                provider: NAME.to_string(),
            })
            .collect())
    }

    async fn versions(&self, pkg: &str) -> Result<Vec<VersionInfo>, ProviderError> {
        let (base, code) = self.resolve(pkg).await?;
        let json = self
            .fetcher
            .get_text(&format!("{base}/android/apps/{code}/versions/1"))
            .await?;
        Ok(parse::parse_versions_json(&json)?
            .into_iter()
            .map(|r| VersionInfo {
                version: r.version,
                version_code: None,
                uploaded: r.uploaded,
                provider: NAME.to_string(),
            })
            .collect())
    }

    async fn download_url(
        &self,
        pkg: &str,
        _version: Option<&str>,
        _arch: &str,
    ) -> Result<DownloadTarget, ProviderError> {
        // Confirm the app exists (useful error ordering), then report the block.
        self.resolve(pkg).await?;
        Err(ProviderError::Blocked { retry_after: None })
    }

    async fn check(&self) -> Result<(), ProviderError> {
        self.search("firefox").await.map(|_| ())
    }
}
