use std::marker::PhantomData;

use async_trait::async_trait;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// The known providers.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, clap::ValueEnum)]
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

/// Provider constants that can be used by generics.
pub trait ProviderConst {
	const ID: ProviderId;
	const BASE_URL: &'static str;
}

/// Minimal URL struct, type of `url` field can be replaced if more complex handling needed.
pub struct Url<C> {
	url: String,
	_marker: PhantomData<C>,
}

impl<C: ProviderConst> From<String> for Url<C> {
	fn from(s: String) -> Self {
		let url = if s.starts_with("http://") || s.starts_with("https://") {
			s
		} else {
			format!("{}/{}", C::BASE_URL, s.trim_start_matches('/'))
		};
		Self {
			url,
			_marker: PhantomData,
		}
	}
}

impl<C: ProviderConst> From<&str> for Url<C> {
	fn from(s: &str) -> Self {
		s.to_string().into()
	}
}

impl<C> Clone for Url<C> {
	fn clone(&self) -> Self {
		Self {
			url: self.url.clone(),
			_marker: PhantomData,
		}
	}
}

impl<C> std::fmt::Debug for Url<C> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_tuple("AbsUrl").field(&self.url).finish()
	}
}

impl<C> std::fmt::Display for Url<C> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{}", self.url)
	}
}

impl<C: ProviderConst> Url<C> {
	pub fn as_str(&self) -> &str {
		&self.url
	}

	pub fn into_string(self) -> String {
		self.url
	}

	/// Path without `BASE_URL` and leading/ending `/`.
	pub fn path(&self) -> Option<&str> {
		Some(self.url.strip_prefix(C::BASE_URL)?.trim_matches('/'))
	}
}

bitflags::bitflags! {
	/// A split bundle carrying every ABI
	#[derive(Debug, Clone, Copy, PartialEq, Eq)]
	pub struct Arch: u8 {
		const ARM64_V8A = 1;
		const ARMEABI_V7A = 2;
		const X86 = 4;
		const X86_64 = 8;
	}
}

impl Arch {
	const NAMES: [(Arch, &'static str); 4] = [
		(Arch::ARM64_V8A, "arm64-v8a"),
		(Arch::ARMEABI_V7A, "armeabi-v7a"),
		(Arch::X86, "x86"),
		(Arch::X86_64, "x86_64"),
	];

	/// One ABI label; Apkmirror says `universal` (variants table) or `noarch` (app page).
	fn single(tok: &str) -> Option<Arch> {
		let t = tok.to_ascii_lowercase();
		Arch::NAMES
			.iter()
			.find_map(|(bit, n)| (*n == t).then_some(*bit))
			.or_else(|| matches!(t.as_str(), "universal" | "noarch").then(Arch::all))
	}
}

/// Parse from `,`/`+`-separated list
impl std::str::FromStr for Arch {
	type Err = String;
	fn from_str(s: &str) -> Result<Self, String> {
		let set: Arch = s
			.split([',', '+'])
			.flat_map(str::split_whitespace)
			.map(|tok| Arch::single(tok).ok_or_else(|| format!("unknown arch {tok:?}")))
			.collect::<Result<_, _>>()?;
		if set.is_empty() {
			Err("empty arch".into())
		} else {
			Ok(set)
		}
	}
}

/// `universal`, one ABI, or `a+b` for a partial split bundle.
impl std::fmt::Display for Arch {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		if *self == Arch::all() {
			return f.write_str("universal");
		}
		let mut first = true;
		for (bit, name) in Arch::NAMES {
			if self.contains(bit) {
				if !first {
					f.write_str("+")?;
				}
				f.write_str(name)?;
				first = false;
			}
		}
		Ok(())
	}
}

impl Serialize for Arch {
	fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
		s.collect_str(self)
	}
}

/// A search hit for an app. Versions are not included as most search results don't have them.
#[derive(Debug, Serialize)]
pub struct AppResult {
	pub package: String,
	pub title: String,
}

/// One published version of an app.
#[derive(Debug, Serialize)]
pub struct VersionInfo {
	pub version: String,
	pub uploaded: NaiveDate,
}

/// A concrete, downloadable APK: URL plus any headers the host requires.
#[derive(Debug)]
pub struct DownloadTarget {
	pub url: String,
	pub version: String,
	pub arch: Arch,
	pub provider: ProviderId,
	pub headers: Vec<(String, String)>,
}

/// Failure modes a provider can hit.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
	/// The site won't serve this client right now.
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

/// `{pkg}-{version}-{arch}`, sanitized for a filesystem.
pub fn download_filename(pkg: &str, version: &str, arch: Arch) -> String {
	format!("{pkg}-{version}-{arch}")
		.chars()
		.map(|c| {
			if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+') {
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
	/// User facing search. Should not used by `download_url`.
	async fn search(&self, query: &str) -> Result<Vec<AppResult>, ProviderError>;
	/// List available versions.
	async fn versions(&self, pkg: &str) -> Result<Vec<VersionInfo>, ProviderError>;
	/// Resolve a download.
	async fn download_url(
		&self,
		pkg: &str,
		version: Option<&str>,
		arch: Arch,
	) -> Result<DownloadTarget, ProviderError>;
	/// Used for `providers check`. Default: a canned search.
	/// Override if a provider has a cheaper health endpoint.
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
		arch: Arch,
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
		arch: Arch,
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
	use super::{Arch, download_filename};

	#[test]
	fn filename_shape() {
		assert_eq!(
			download_filename("org.mozilla.firefox", "155.0.1", Arch::ARM64_V8A),
			"org.mozilla.firefox-155.0.1-arm64-v8a"
		);
		assert_eq!(
			download_filename("mozilla/firefox", "1.0", Arch::all()),
			"mozilla_firefox-1.0-universal"
		);
	}

	#[test]
	fn arch_round_trip() {
		assert_eq!(" arm64-v8a ".parse(), Ok(Arch::ARM64_V8A));
		assert_eq!("x86_64".parse(), Ok(Arch::X86_64));
		assert_eq!("X86".parse(), Ok(Arch::X86));
		assert_eq!("universal".parse(), Ok(Arch::all()));
		assert_eq!("noarch".parse(), Ok(Arch::all()));
		// split bundles: the listed set, every ABI == universal
		assert_eq!(
			"arm64-v8a, armeabi-v7a, x86, x86_64".parse(),
			Ok(Arch::all())
		);
		assert_eq!("arm64-v8a + x86".parse(), Ok(Arch::ARM64_V8A | Arch::X86));
		// absent or unknown: resolves as universal
		assert!("".parse::<Arch>().is_err());
		assert!("mips".parse::<Arch>().is_err());

		assert_eq!(Arch::all().to_string(), "universal");
		assert_eq!(Arch::ARMEABI_V7A.to_string(), "armeabi-v7a");
		assert_eq!((Arch::ARM64_V8A | Arch::X86).to_string(), "arm64-v8a+x86");
	}
}
