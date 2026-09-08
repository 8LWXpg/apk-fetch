//! APKPure provider — stub this session. Exists to prove the [`Provider`] trait
//! boundary supports a second source cleanly. All methods return `NotFound` so
//! the fallback resolver treats it as "nothing here" rather than panicking.

use apk_fetch_core::{AppResult, DownloadTarget, Provider, ProviderError, VersionInfo};
use apk_fetch_fetch::HttpFetcher;
use async_trait::async_trait;

pub struct ApkPure {
    #[allow(dead_code)]
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
        "apkpure"
    }

    async fn search(&self, _query: &str) -> Result<Vec<AppResult>, ProviderError> {
        Err(ProviderError::NotFound) // TODO(apkpure): implement
    }

    async fn versions(&self, _pkg: &str) -> Result<Vec<VersionInfo>, ProviderError> {
        Err(ProviderError::NotFound) // TODO(apkpure): implement
    }

    async fn download_url(
        &self,
        _pkg: &str,
        _version: Option<&str>,
    ) -> Result<DownloadTarget, ProviderError> {
        Err(ProviderError::NotFound) // TODO(apkpure): implement
    }

    async fn check(&self) -> Result<(), ProviderError> {
        Ok(()) // stub is always "up"
    }
}
