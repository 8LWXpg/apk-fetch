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
use std::cell::{Cell, RefCell};
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

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
const TRACKER_URL: &str = "https://t.uptodown.app/eapi/v2/tracker/device";
/// Builds per list request (what the app asks for).
const PAGE_SIZE: usize = 50;
const PAGE_LIMIT: usize = 100;

pub struct Uptodown {
	fetcher: HttpFetcher,
	/// Client id the server knows this run by.
	device: String,
	token: RefCell<Option<String>>,
	registered: Cell<bool>,
}

impl Default for Uptodown {
	fn default() -> Self {
		Self::new(HttpFetcher::new())
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
			registered: Cell::new(false),
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
		let cached = self.token.borrow().clone();
		let token = match cached {
			Some(t) => t,
			None => {
				let t = self.authorize()?;
				*self.token.borrow_mut() = Some(t.clone());
				t
			}
		};
		Ok(app_headers(Some(&token)))
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

	fn get(&self, url: &Url) -> Result<String> {
		self.authed(|h| self.fetcher.get(url.as_str(), h))
	}

	fn register(&self) -> Result<()> {
		if self.registered.get() {
			return Ok(());
		}
		let device = parse::device_json(&self.device);
		let body = self.authed(|h| {
			self.fetcher
				.post_urlencoded(TRACKER_URL, h, &[("device", device.as_str())])
		})?;
		parse::parse_ack(&body)?;
		self.registered.set(true);
		Ok(())
	}

	fn resolve(&self, pkg: &str) -> Result<String> {
		parse::parse_app_id(
			&self.get(&Url::from(format!("/apps/byPackagename/{}", query(pkg))))?,
			pkg,
		)
	}

	/// One list page (`n` from 0). A 404 is an empty list.
	fn page(&self, id: &str, n: usize) -> Result<Vec<Row>> {
		self.register()?;
		let url = Url::from(format!(
			"/v3/app/{id}/device/{}/compatible/versions?page[limit]={PAGE_SIZE}&page[offset]={}\
			 &lang=en&id_plataforma=13&identifier={}&countryIsoCode=US",
			self.device,
			n * PAGE_SIZE,
			self.device,
		));
		match self.get(&url) {
			Ok(body) => parse::parse_page(&body),
			Err(ProviderError::NotFound(_)) => Ok(Vec::new()),
			Err(e) => Err(e),
		}
	}

	/// Feeds pages to `visit` until it returns `true`, a page is empty, or `pages` are read.
	fn walk(&self, id: &str, pages: usize, mut visit: impl FnMut(Vec<Row>) -> bool) -> Result<()> {
		for n in 0..pages {
			let rows = self.page(id, n)?;
			if rows.is_empty() || visit(rows) {
				break;
			}
		}
		Ok(())
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
		parse::parse_search(&self.get(&Url::from(format!(
			"/v2/apps/search/{}?page[limit]=30&page[offset]=0&lang=en",
			query(q)
		)))?)
	}

	fn versions(&self, pkg: &str) -> Result<Vec<VersionInfo>> {
		let id = self.resolve(pkg)?;
		let rows = self.page(&id, 0)?;
		parse::into_versions(rows)
	}

	fn download_url(&self, pkg: &str, version: Option<&str>, arch: Arch) -> Result<DownloadTarget> {
		let id = self.resolve(pkg)?;
		// Latest is on page 1; a pinned version may be anywhere.
		let mut found = None;
		self.walk(&id, if version.is_some() { PAGE_LIMIT } else { 1 }, |page| {
			found = parse::pick(&page, version, arch).cloned();
			found.is_some()
		})?;
		let row = found.ok_or_else(|| {
			ProviderError::no_match(
				format!("no {arch} build {} for {pkg}", version.unwrap_or("latest")),
				"uptodown",
			)
		})?;

		let dl = parse::parse_download(&self.get(&Url::from(format!(
			"/apps/{id}/file/{}/downloadUrl?update=0",
			row.file_id
		)))?)?;
		Ok(DownloadTarget {
			url: dl.url,
			version: row.version,
			arch: row.arch,
			provider: Self::ID,
			headers: Vec::new(),
		})
	}

	fn check(&self) -> Result<()> {
		self.resolve("org.mozilla.firefox").map(|_| ())
	}
}

/// Fixture file for each URL the provider fetches. Only list pages 1-2 are recorded.
#[cfg(test)]
fn fixture_name(url: &str) -> Option<&'static str> {
	let page = |n: usize| url.contains(&format!("page[offset]={}&", n * PAGE_SIZE));
	Some(if url.contains("/v2/apps/search/") {
		"search.json"
	} else if url.contains("/compatible/versions") {
		if page(0) {
			"versions-1.json"
		} else if page(1) {
			"versions-2.json"
		} else {
			return None;
		}
	} else if url.contains("/downloadUrl") {
		"download.json"
	} else if url.contains("/byPackagename/") {
		"lookup.json"
	} else if url.contains("/tracker/device") {
		"register.json"
	} else if url.contains("/auth/token") {
		"auth.json"
	} else {
		return None;
	})
}

/// `cargo test refresh_fixtures -- --ignored`
/// Scrub the JWT in `auth.html` and the signed URL in `download.html` before committing.
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
