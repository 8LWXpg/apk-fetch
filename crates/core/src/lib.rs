//! Core contract: the [`Provider`] trait, domain types, error taxonomy, and the
//! [`ProviderRegistry`] fallback resolver. Everything else depends on this staying
//! stable.

pub mod ui;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// A search hit for an app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppResult {
    pub package: String,
    pub title: String,
    pub developer: Option<String>,
    /// Which provider produced this result.
    pub provider: String,
}

/// One published version of an app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub version_code: Option<String>,
    pub uploaded: Option<String>,
    pub provider: String,
}

/// A concrete, fetchable APK: URL plus any headers the host requires (referer,
/// cookies, UA) to serve the file rather than a challenge page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadTarget {
    pub url: String,
    pub filename: String,
    pub version: Option<String>,
    pub provider: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
}

/// Failure modes a provider can hit. `Blocked` is deliberately distinct from
/// `ParseError`: a Cloudflare challenge or 403/429 is not our markup assumptions
/// breaking, it means "try someone else", so the resolver treats it specially.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("blocked by anti-bot / rate limit (challenge page, 403, or 429)")]
    Blocked { retry_after: Option<Duration> },
    #[error("not found")]
    NotFound,
    #[error("parse error: {0}")]
    ParseError(String),
    #[error("network error: {0}")]
    Network(#[from] std::io::Error),
    #[error("rate limited")]
    RateLimited,
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn search(&self, query: &str) -> Result<Vec<AppResult>, ProviderError>;
    async fn versions(&self, pkg: &str) -> Result<Vec<VersionInfo>, ProviderError>;
    async fn download_url(
        &self,
        pkg: &str,
        version: Option<&str>,
    ) -> Result<DownloadTarget, ProviderError>;

    /// Lightweight reachability probe for `providers check`. Default: a canned
    /// search. Override if a provider has a cheaper health endpoint.
    async fn check(&self) -> Result<(), ProviderError> {
        self.search("firefox").await.map(|_| ())
    }
}

/// Aggregated failure when every provider in the fallback chain gave up.
#[derive(Debug)]
pub struct ResolveError {
    pub pkg: String,
    pub attempts: Vec<(String, ProviderError)>,
}

impl ResolveError {
    /// True if every attempt was a block / rate-limit (distinct exit code).
    pub fn all_blocked(&self) -> bool {
        !self.attempts.is_empty()
            && self
                .attempts
                .iter()
                .all(|(_, e)| matches!(e, ProviderError::Blocked { .. } | ProviderError::RateLimited))
    }

    /// True if every attempt was a clean "not found".
    pub fn all_not_found(&self) -> bool {
        !self.attempts.is_empty()
            && self
                .attempts
                .iter()
                .all(|(_, e)| matches!(e, ProviderError::NotFound))
    }

    /// True if any attempt failed on the network (vs. parse / not-found).
    pub fn any_network(&self) -> bool {
        self.attempts
            .iter()
            .any(|(_, e)| matches!(e, ProviderError::Network(_)))
    }
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "all providers failed for '{}':", self.pkg)?;
        for (name, err) in &self.attempts {
            write!(f, "\n  - {name}: {err}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ResolveError {}

/// Holds configured providers and drives priority-ordered fallback.
#[derive(Default)]
pub struct ProviderRegistry {
    providers: Vec<Box<dyn Provider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, provider: Box<dyn Provider>) {
        self.providers.push(provider);
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.providers.iter().map(|p| p.name()).collect()
    }

    pub fn get(&self, name: &str) -> Option<&dyn Provider> {
        self.providers
            .iter()
            .find(|p| p.name() == name)
            .map(|b| b.as_ref())
    }

    /// Providers named in `order`, in that order, skipping unknown names.
    fn ordered<'a>(&'a self, order: &[&str]) -> Vec<&'a dyn Provider> {
        order.iter().filter_map(|n| self.get(n)).collect()
    }

    /// Try each provider in `order`; return the first `download_url` success.
    /// Any `ProviderError` (especially `Blocked`) moves on to the next provider.
    pub async fn resolve_with_fallback(
        &self,
        pkg: &str,
        version: Option<&str>,
        order: &[&str],
    ) -> Result<DownloadTarget, ResolveError> {
        let mut attempts = Vec::new();
        for provider in self.ordered(order) {
            match provider.download_url(pkg, version).await {
                Ok(target) => return Ok(target),
                Err(e) => attempts.push((provider.name().to_string(), e)),
            }
        }
        Err(ResolveError {
            pkg: pkg.to_string(),
            attempts,
        })
    }
}
