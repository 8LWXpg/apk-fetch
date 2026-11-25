//! Uptodown: the Android app's `eapi` JSON API, no scraping.
//!
//!   POST api.uptodown.app/auth/token                 -> JWT (body carries HMAC-SHA256 of unix time)
//!   POST t.uptodown.app/eapi/v2/tracker/device       -> registers this run's device id
//!   GET  /v2/apps/search/{q}?lang=en                 -> hits by name (`packageName`, `name`)
//!   GET  /apps/byPackagename/{pkg}                   -> numeric `appID`
//!   GET  /v3/app/{id}/device/{device}/compatible/versions?page[limit]&page[offset]
//!                                                    -> builds: `fileID`, `version`, `lastUpdate`,
//!                                                       `abis`, `fileType`, `sha256`
//!   GET  /apps/{id}/file/{fileID}/downloadUrl        -> signed CDN URL + `sha256`
mod parse;
use std::cell::RefCell;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, KeyInit, Mac};
use parse::{Row, Url};
use sha2::Sha256;

use crate::common::contract::{
	AppResult, Arch, DownloadTarget, Provider, ProviderConst, ProviderError, ProviderId, Result, VersionInfo,
};
use crate::common::fetch::HttpFetcher;
use crate::providers::scrape::query;

const HMAC_KEY: &str = "MDGMXUMdvHJBG/vjdFgmqX6LUdy7ecfwvYNd0gyfOCs=";
const CLIENT_VERSION: &str = "745";
const WEB_PAGES: &str = "https://en.uptodown.com/android/apps";
const PAGE_LIMIT: usize = 10;

pub struct Uptodown {
	fetcher: HttpFetcher,
	/// Identifier for `/auth/token`
	device: String,
	token: RefCell<Option<String>>,
}

impl Default for Uptodown {
	fn default() -> Self {
		Self::new(HttpFetcher::with_delay(Duration::from_millis(100)))
	}
}

fn app_headers(token: Option<&str>) -> Vec<(String, String)> {
	let mut h = vec![
		("Identificador".to_string(), "Uptodown_Android".to_string()),
		("Identificador-Version".to_string(), CLIENT_VERSION.to_string()),
		("Client-Version".to_string(), CLIENT_VERSION.to_string()),
		("Accept".to_string(), "application/json".to_string()),
		("Accept-Language".to_string(), "en".to_string()),
	];
	if let Some(t) = token {
		h.push(("Authorization".to_string(), format!("Bearer {t}")));
	}
	h
}

impl Uptodown {
	fn new(fetcher: HttpFetcher) -> Self {
		Self {
			fetcher,
			device: format!("{:016x}", RandomState::new().build_hasher().finish()),
			token: RefCell::default(),
		}
	}

	fn authorize(&self) -> Result<String> {
		let ts = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.map_or(0, |d| d.as_secs())
			.to_string();

		let timestamp: &str = &ts;
		let mut mac = Hmac::<Sha256>::new_from_slice(HMAC_KEY.as_bytes()).unwrap();
		mac.update(timestamp.as_bytes());
		let sig = hex::encode(mac.finalize().into_bytes());

		let body = self.fetcher.post_urlencoded(
			Url::from(format!("/auth/token?identifier={}", self.device)).as_str(),
			&app_headers(None),
			&[
				("hmac", sig.as_str()),
				("unixtime", ts.as_str()),
				("lang", "en"),
				("id_plataforma", "13"),
				("identifier", self.device.as_str()),
			],
		)?;
		parse::parse_token(&body)
	}

	/// Request headers with a valid bearer token (fetched on first use).
	fn headers(&self) -> Result<Vec<(String, String)>> {
		if self.token.borrow().is_none() {
			let t = self.authorize()?;
			*self.token.borrow_mut() = Some(t);
		}
		Ok(app_headers(self.token.borrow().as_deref()))
	}

	/// Runs `f` with auth headers; a 401 drops the token and retries once.
	fn authed<T>(&self, f: impl Fn(&[(String, String)]) -> Result<T>) -> Result<T> {
		match f(&self.headers()?) {
			Err(ProviderError::Unauthorized) => {
				self.token.take();
				f(&self.headers()?)
			}
			r => r,
		}
	}

	fn get(&self, path: String) -> Result<String> {
		let url = Url::from(path);
		self.authed(|h| self.fetcher.get(url.as_str(), h))
	}

	fn resolve(&self, pkg: &str) -> Result<String> {
		parse::parse_app_id(&self.get(format!("/apps/byPackagename/{pkg}"))?, pkg)
	}

	/// One list page (`n` starts from 0), 20 items each. No auth needed.
	fn page(&self, id: &str, n: usize) -> Result<Vec<Row>> {
		parse::parse_page(&self.fetcher.get_text(&format!("{WEB_PAGES}/{id}/versions/{n}"))?)
	}
}

impl ProviderConst for Uptodown {
	const ID: ProviderId = ProviderId::Uptodown;
	const BASE_URL: &'static str = "https://www.uptodown.app/eapi";
}

impl Provider for Uptodown {
	#[cfg(test)]
	fn with_fetcher(fetcher: HttpFetcher) -> Self
	where
		Self: Sized,
	{
		Self::new(fetcher)
	}

	fn id(&self) -> ProviderId {
		Self::ID
	}

	fn search(&self, q: &str) -> Result<Vec<AppResult>> {
		parse::parse_search(&self.get(format!(
			"/v2/apps/search/{}?page[limit]=30&page[offset]=0&lang=en",
			query(q)
		))?)
	}

	fn versions(&self, pkg: &str) -> Result<Vec<VersionInfo>> {
		let id = self.resolve(pkg)?;
		// Only check for first page
		parse::into_versions(self.page(&id, 0)?)
	}

	fn download_url(&self, pkg: &str, version: Option<&str>, _arch: Arch) -> Result<DownloadTarget> {
		let id = self.resolve(pkg)?;
		// Latest is on page 1; a pinned version may be anywhere.
		let pages = if version.is_some() { PAGE_LIMIT } else { 1 };
		let mut found = None;
		for n in 0..=pages {
			let rows = self.page(&id, n)?;
			if rows.is_empty() {
				break;
			}
			if let Some(r) = parse::pick(&rows, version) {
				found = Some(r.clone());
				break;
			}
		}
		let row = found
			.ok_or_else(|| ProviderError::NoMatch(format!("no build {} for {pkg}", version.unwrap_or("latest"))))?;

		let url = parse::parse_download(&self.get(format!("/apps/{id}/file/{}/downloadUrl?update=0", row.file_id))?)?;
		Ok(DownloadTarget {
			url,
			version: row.version,
			arch: Arch::all(), // Uptodown serves one universal build per version
			provider: Self::ID,
			headers: Vec::new(),
		})
	}

	fn check(&self) -> Result<()> {
		self.resolve("org.mozilla.firefox").map(|_| ())
	}
}

/// Fixture file for each URL the provider fetches.
#[cfg(test)]
fn fixture_name(url: &str) -> Option<&'static str> {
	Some(if url.contains("/v2/apps/search/") {
		"search.json"
	} else if url.contains("/android/apps/") && url.contains("/versions/") {
		"versions.json"
	} else if url.contains("/downloadUrl") {
		"download.json"
	} else if url.contains("/byPackagename/") {
		"lookup.json"
	} else if url.contains("/auth/token") {
		"auth.json"
	} else {
		return None;
	})
}

/// `cargo test refresh_fixtures -- --ignored --nocapture`
#[cfg(test)]
mod refresh {
	use super::*;
	use crate::providers::fixtures;

	#[test]
	#[ignore = "network"]
	fn refresh_fixtures() {
		fixtures::refresh_fixtures::<Uptodown>(fixture_name);
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::providers::fixtures::{APPS, assert_invariants, assert_record, root, run_flow};

	#[test]
	fn resolves_from_fixtures() {
		for (app, pkg) in APPS {
			let dir = root("uptodown").join(app);
			let p = Uptodown::with_fetcher(HttpFetcher::playback(dir.clone(), fixture_name));

			let record = run_flow(&p, app, pkg);
			assert_record(&dir, &record, app);
			assert_invariants(&record, pkg, app);
			assert!(record.target.headers.is_empty(), "{app}: unexpected headers");
		}
	}
}
