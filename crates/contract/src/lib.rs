//! Core contract: the [`Provider`] trait, domain types, error taxonomy, and the
//! [`ProviderRegistry`] fallback resolver. Everything else depends on this staying
//! stable.

pub mod ui;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// The known providers. One enum, used end to end: the CLI parses `--provider`
/// into it, the registry is keyed by it, and every [`ProviderFailure`] carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Apkmirror,
    Apkpure,
    Apkcombo,
}

impl ProviderId {
    /// Fallback order for `get`, and the single provider `search` / `versions`
    /// use when none is named. (`ProviderId::value_variants()` from `ValueEnum`
    /// gives the full set if you need it.)
    pub const DEFAULT_PRIORITY: &'static [ProviderId] = &[
        ProviderId::Apkcombo,
        ProviderId::Apkpure,
        ProviderId::Apkmirror,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            ProviderId::Apkmirror => "apkmirror",
            ProviderId::Apkpure => "apkpure",
            ProviderId::Apkcombo => "apkcombo",
        }
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A search hit for an app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppResult {
    pub package: String,
    pub title: String,
    /// Latest/only version the search row advertised, if any (APKMirror shows it;
    /// the others don't).
    pub version: Option<String>,
    pub developer: Option<String>,
    /// Which provider produced this result.
    pub provider: ProviderId,
}

/// One published version of an app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub version_code: Option<String>,
    pub uploaded: Option<String>,
    pub provider: ProviderId,
}

/// A concrete, fetchable APK: URL plus any headers the host requires (referer,
/// cookies, UA) to serve the file rather than a challenge page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadTarget {
    pub url: String,
    pub filename: String,
    pub version: Option<String>,
    /// Architecture of the resolved variant (e.g. `arm64-v8a`, `universal`).
    pub arch: Option<String>,
    pub provider: ProviderId,
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
    /// The string says what was missing (e.g. `no app page for jp.naver.line.android`)
    /// — a bare "not found" is useless in a failover log. The provider name is
    /// added by [`ProviderFailure`], so the message must not repeat it.
    #[error("{0}")]
    NotFound(String),
    #[error("parse error: {0}")]
    ParseError(String),
    #[error("network error: {0}")]
    Network(#[from] std::io::Error),
    #[error("rate limited")]
    RateLimited,
}

/// A [`ProviderError`] tagged with which provider produced it. This is what leaves
/// the [`ProviderRegistry`] — a bare `ProviderError` never reaches the CLI.
#[derive(Debug, thiserror::Error)]
#[error("{provider}: {source}")]
pub struct ProviderFailure {
    pub provider: ProviderId,
    #[source]
    pub source: ProviderError,
}

/// `{pkg}-{version}-{arch}.{ext}`, sanitised for a filesystem. `arch` is dropped
/// from the name when unknown. `ext` is `"apk"` or `"xapk"`.
pub fn download_filename(pkg: &str, version: &str, arch: Option<&str>, ext: &str) -> String {
    let stem = match arch {
        Some(a) if !a.is_empty() => format!("{pkg}-{version}-{a}"),
        _ => format!("{pkg}-{version}"),
    };
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{cleaned}.{ext}")
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> ProviderId;
    async fn search(&self, query: &str) -> Result<Vec<AppResult>, ProviderError>;
    async fn versions(&self, pkg: &str) -> Result<Vec<VersionInfo>, ProviderError>;
    /// Resolve a download. `arch` is an ABI preference (e.g. `arm64-v8a`); a
    /// provider falls back to a universal build if it has no exact match.
    async fn download_url(
        &self,
        pkg: &str,
        version: Option<&str>,
        arch: &str,
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
    pub attempts: Vec<ProviderFailure>,
}

impl ResolveError {
    /// True if every attempt was a block / rate-limit (distinct exit code).
    pub fn all_blocked(&self) -> bool {
        !self.attempts.is_empty()
            && self.attempts.iter().all(|f| {
                matches!(
                    f.source,
                    ProviderError::Blocked { .. } | ProviderError::RateLimited
                )
            })
    }

    /// True if every attempt was a clean "not found".
    pub fn all_not_found(&self) -> bool {
        !self.attempts.is_empty()
            && self
                .attempts
                .iter()
                .all(|f| matches!(f.source, ProviderError::NotFound(_)))
    }

    /// True if any attempt failed on the network (vs. parse / not-found).
    pub fn any_network(&self) -> bool {
        self.attempts
            .iter()
            .any(|f| matches!(f.source, ProviderError::Network(_)))
    }
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "all providers failed for '{}':", self.pkg)?;
        for attempt in &self.attempts {
            write!(f, "\n  - {attempt}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ResolveError {}

/// Holds configured providers and drives priority-ordered fallback. Every call
/// through the registry tags failures with the [`ProviderId`], so callers get a
/// [`ProviderFailure`], never a bare [`ProviderError`].
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

    /// The registered providers, in registration order.
    pub fn names(&self) -> Vec<ProviderId> {
        self.providers.iter().map(|p| p.name()).collect()
    }

    pub fn get(&self, id: ProviderId) -> Option<&dyn Provider> {
        self.providers
            .iter()
            .find(|p| p.name() == id)
            .map(|b| b.as_ref())
    }

    fn tag<T>(id: ProviderId, r: Result<T, ProviderError>) -> Result<T, ProviderFailure> {
        r.map_err(|source| ProviderFailure {
            provider: id,
            source,
        })
    }

    /// Every `ProviderId` is registered by `build_registry`, and the CLI only ever
    /// passes ids parsed from the enum — so a lookup miss is a programming error.
    fn require(&self, id: ProviderId) -> &dyn Provider {
        self.get(id)
            .unwrap_or_else(|| panic!("provider {id} not registered"))
    }

    pub async fn search(
        &self,
        id: ProviderId,
        query: &str,
    ) -> Result<Vec<AppResult>, ProviderFailure> {
        Self::tag(id, self.require(id).search(query).await)
    }

    pub async fn versions(
        &self,
        id: ProviderId,
        pkg: &str,
    ) -> Result<Vec<VersionInfo>, ProviderFailure> {
        Self::tag(id, self.require(id).versions(pkg).await)
    }

    pub async fn download_url(
        &self,
        id: ProviderId,
        pkg: &str,
        version: Option<&str>,
        arch: &str,
    ) -> Result<DownloadTarget, ProviderFailure> {
        Self::tag(id, self.require(id).download_url(pkg, version, arch).await)
    }

    pub async fn check(&self, id: ProviderId) -> Result<(), ProviderFailure> {
        Self::tag(id, self.require(id).check().await)
    }

    /// Try each provider in `order`; return the first `download_url` success.
    /// Any `ProviderError` (especially `Blocked`) moves on to the next provider.
    pub async fn resolve_with_fallback(
        &self,
        pkg: &str,
        version: Option<&str>,
        arch: &str,
        order: &[ProviderId],
    ) -> Result<DownloadTarget, ResolveError> {
        let mut attempts = Vec::new();
        for &id in order {
            match self.download_url(id, pkg, version, arch).await {
                Ok(target) => return Ok(target),
                Err(f) => attempts.push(f),
            }
        }
        Err(ResolveError {
            pkg: pkg.to_string(),
            attempts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::download_filename;

    #[test]
    fn filename_shape() {
        assert_eq!(
            download_filename("org.mozilla.firefox", "155.0.1", Some("arm64-v8a"), "apk"),
            "org.mozilla.firefox-155.0.1-arm64-v8a.apk"
        );
        assert_eq!(
            download_filename("org.mozilla.firefox", "155.0.1", None, "xapk"),
            "org.mozilla.firefox-155.0.1.xapk"
        );
        // path separators from a slug-style id get scrubbed
        assert_eq!(
            download_filename("mozilla/firefox", "1.0", Some("universal"), "apk"),
            "mozilla_firefox-1.0-universal.apk"
        );
    }
}
