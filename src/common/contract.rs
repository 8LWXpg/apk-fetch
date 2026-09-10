use async_trait::async_trait;
use serde::{Deserialize, Serialize};

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
	pub version: Option<String>,
	/// Architecture of the resolved variant (e.g. `arm64-v8a`, `universal`).
	pub arch: Option<String>,
	pub provider: ProviderId,
	#[serde(default)]
	pub headers: Vec<(String, String)>,
}

/// Failure modes a provider can hit.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
	#[error("blocked by anti-bot / rate limit (challenge page, 403, or 429)")]
	Blocked,
	/// The string says what was missing. The provider name is added by
	/// [`ProviderFailure`], so the message must not repeat it.
	#[error("{0}")]
	NotFound(String),
	#[error("parse error: {0}")]
	ParseError(String),
	#[error("network error: {0}")]
	Network(#[from] std::io::Error),
	/// Ctrl+C
	#[error("cancelled")]
	Cancelled,
}

#[derive(Debug, thiserror::Error)]
#[error("{provider}: {source}")]
pub struct ProviderFailure {
	pub provider: ProviderId,
	#[source]
	pub source: ProviderError,
}

impl ProviderError {
	/// Attributes this failure to the provider that raised it.
	pub fn by(self, provider: ProviderId) -> ProviderFailure {
		ProviderFailure {
			provider,
			source: self,
		}
	}
}

/// `{pkg}-{version}-{arch}`, sanitised for a filesystem. `arch` is dropped when
/// unknown. No extension: the fetcher appends whatever the site actually serves.
pub fn download_filename(pkg: &str, version: &str, arch: Option<&str>) -> String {
	let stem = match arch {
		Some(a) if !a.is_empty() => format!("{pkg}-{version}-{a}"),
		_ => format!("{pkg}-{version}"),
	};
	stem.chars()
		.map(|c| {
			if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
				c
			} else {
				'_'
			}
		})
		.collect()
}

#[async_trait]
pub trait Provider: Send + Sync {
	fn id(&self) -> ProviderId;
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

#[derive(Debug)]
pub struct ResolveError {
	pub pkg: String,
	pub attempts: Vec<ProviderFailure>,
}

impl std::fmt::Display for ResolveError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		// Only one provider was asked; naming a list of one reads as noise.
		if let [only] = self.attempts.as_slice() {
			return write!(f, "{only}");
		}
		write!(f, "all providers failed for '{}':", self.pkg)?;
		for attempt in &self.attempts {
			write!(f, "\n  - {attempt}")?;
		}
		Ok(())
	}
}

impl std::error::Error for ResolveError {}

/// Holds configured providers **in priority order**
#[derive(Default)]
pub struct ProviderRegistry {
	providers: Vec<Box<dyn Provider>>,
}

impl ProviderRegistry {
	pub fn new() -> Self {
		Self::default()
	}

	/// Appends one provider.
	pub fn register(&mut self, provider: Box<dyn Provider>) {
		self.providers.push(provider);
	}

	/// The highest-priority provider. The registry is never empty: it is built
	/// from a non-empty selection.
	pub fn top(&self) -> ProviderId {
		self.names()[0]
	}

	pub fn names(&self) -> Vec<ProviderId> {
		self.providers.iter().map(|p| p.id()).collect()
	}

	#[track_caller]
	fn require(&self, id: ProviderId) -> &dyn Provider {
		self.providers
			.iter()
			.find(|p| p.id() == id)
			.unwrap_or_else(|| panic!("provider {id} not registered"))
			.as_ref()
	}

	pub async fn search(
		&self,
		id: ProviderId,
		query: &str,
	) -> Result<Vec<AppResult>, ProviderFailure> {
		self.require(id).search(query).await.map_err(|e| e.by(id))
	}

	pub async fn versions(
		&self,
		id: ProviderId,
		pkg: &str,
	) -> Result<Vec<VersionInfo>, ProviderFailure> {
		self.require(id).versions(pkg).await.map_err(|e| e.by(id))
	}

	pub async fn download_url(
		&self,
		id: ProviderId,
		pkg: &str,
		version: Option<&str>,
		arch: &str,
	) -> Result<DownloadTarget, ProviderFailure> {
		self.require(id)
			.download_url(pkg, version, arch)
			.await
			.map_err(|e| e.by(id))
	}

	pub async fn check(&self, id: ProviderId) -> Result<(), ProviderFailure> {
		self.require(id).check().await.map_err(|e| e.by(id))
	}

	/// Tries every provider in priority order, returning the first success.
	pub async fn resolve_with_fallback(
		&self,
		pkg: &str,
		version: Option<&str>,
		arch: &str,
	) -> Result<DownloadTarget, ResolveError> {
		let mut attempts = Vec::new();
		for id in self.names() {
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

impl From<Vec<Box<dyn Provider>>> for ProviderRegistry {
	fn from(value: Vec<Box<dyn Provider>>) -> Self {
		Self { providers: value }
	}
}

#[cfg(test)]
mod tests {
	use super::download_filename;

	#[test]
	fn filename_shape() {
		assert_eq!(
			download_filename("org.mozilla.firefox", "155.0.1", Some("arm64-v8a")),
			"org.mozilla.firefox-155.0.1-arm64-v8a"
		);
		assert_eq!(
			download_filename("org.mozilla.firefox", "155.0.1", None),
			"org.mozilla.firefox-155.0.1"
		);
		// path separators from a slug-style id get scrubbed
		assert_eq!(
			download_filename("mozilla/firefox", "1.0", Some("universal")),
			"mozilla_firefox-1.0-universal"
		);
	}
}
