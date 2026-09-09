//! APKPure provider. APKPure is package-id-addressable — `/x/{pkg}` resolves to
//! the app page and `d.apkpure.com/b/APK/{pkg}?version=...` 302s straight to the
//! APK — so there's no search-walk to reach a download.
//!
//! `arch` is accepted but not honoured: APKPure's web endpoint serves one build
//! per app regardless of ABI, so resolved `arch` is left `None`.

mod parse;

use async_trait::async_trait;
use contract::{AppResult, DownloadTarget, Provider, ProviderError, ProviderId, VersionInfo};
use fetch::{Fetcher, HttpFetcher};

const NAME: ProviderId = ProviderId::Apkpure;

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
    fn name(&self) -> ProviderId {
        NAME
    }

    async fn search(&self, query: &str) -> Result<Vec<AppResult>, ProviderError> {
        let url = format!(
            "{}/search?q={}",
            parse::BASE_URL,
            query.trim().replace(' ', "+")
        );
        let html = self.fetcher.get_text(&url).await?;
        Ok(parse::parse_search(&html)?
            .into_iter()
            .map(|h| AppResult {
                package: h.package,
                title: h.title,
                version: None,
                developer: h.developer,
                provider: NAME,
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
                provider: NAME,
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
        // A pinned version is confirmed against the versions list (and normalised
        // to apkpure's own string) so a typo / missing build fails over instead of
        // silently downloading "latest". Unpinned: ask for "latest" but resolve the
        // real number for the name. Either GET also 404s -> NotFound for an unknown
        // package.
        let (endpoint_version, label) = match version {
            Some(want) => {
                let html = self
                    .fetcher
                    .get_text(&format!("{}/x/{}/versions", parse::BASE_URL, pkg))
                    .await?;
                let v = parse::parse_versions(&html)?
                    .into_iter()
                    .map(|r| r.version)
                    .find(|v| parse::version_matches(v, want))
                    .ok_or_else(|| ProviderError::NotFound(format!("no build {want} for {pkg}")))?;
                (v.clone(), v)
            }
            None => {
                let html = self
                    .fetcher
                    .get_text(&format!("{}/x/{}", parse::BASE_URL, pkg))
                    .await?;
                let label = parse::latest_version(&html)
                    .ok_or_else(|| ProviderError::NotFound(format!("no app page for {pkg}")))?;
                ("latest".to_string(), label)
            }
        };

        Ok(DownloadTarget {
            filename: contract::download_filename(pkg, &label, None, "apk"),
            url: parse::download_url(pkg, &endpoint_version),
            version: Some(label),
            arch: None,
            provider: NAME,
            headers: Vec::new(),
        })
    }
}
