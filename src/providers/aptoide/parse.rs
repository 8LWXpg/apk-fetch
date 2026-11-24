use chrono::NaiveDate;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::common::contract::{AppResult, Arch, ProviderError, Result};
use crate::providers::scrape::parse_date;

pub type Url = crate::common::contract::Url<super::Aptoide>;

#[derive(Deserialize)]
struct SearchResp {
	datalist: Option<DataList>,
}

#[derive(Deserialize)]
struct DataList {
	list: Vec<SearchHit>,
}

#[derive(Deserialize)]
struct VersionsResp {
	list: Vec<App>,
}

#[derive(Deserialize)]
pub struct SearchHit {
	pub name: String,
	pub package: String,
}

/// One build from `listAppVersions`. Every entry is a standalone APK uploaded by some store.
#[derive(Deserialize)]
pub struct App {
	pub id: u64,
	pub package: String,
	pub store: Store,
	pub file: File,
}

#[derive(Deserialize)]
pub struct Store {
	pub name: String,
}

#[derive(Deserialize)]
pub struct File {
	pub vername: String,
	pub vercode: u64,
	pub md5sum: String,
	/// `2026-10-24 13:10:42`
	pub added: String,
	pub hardware: Hardware,
}

#[derive(Deserialize)]
pub struct Hardware {
	pub cpus: Vec<String>,
}

impl App {
	/// `listAppVersions` carries no download link; it's
	/// `pool.apk.aptoide.com/{store}/{pkg-with-dashes}-{vercode}-{id}-{md5}.apk`.
	pub fn apk_url(&self) -> Url {
		format!(
			"https://pool.apk.aptoide.com/{}/{}-{}-{}-{}.apk",
			self.store.name,
			self.package.replace('.', "-"),
			self.file.vercode,
			self.id,
			self.file.md5sum
		)
		.into()
	}

	/// ABIs the build ships. Empty `cpus` = runs anywhere.
	pub fn arch(&self) -> Arch {
		let arch = Arch::from_abis(self.file.hardware.cpus.iter());
		if arch.is_empty() { Arch::all() } else { arch }
	}

	pub fn uploaded(&self) -> Result<NaiveDate> {
		parse_date(&self.file.added, "%Y-%m-%d %H:%M:%S")
	}
}

fn json<T: DeserializeOwned>(body: &str) -> Result<T> {
	serde_json::from_str(body).map_err(|e| ProviderError::parse_error("aptoide json", e))
}

/// `apps/search`: one hit per package (the site lists one per uploader store).
pub fn parse_search(body: &str) -> Result<Vec<AppResult>> {
	let r: SearchResp = json(body)?;
	let hits: Vec<AppResult> = r
		.datalist
		.ok_or_else(|| ProviderError::ParseError("no `datalist` in search".into()))?
		.list
		.into_iter()
		.map(|h| AppResult {
			title: h.name,
			package: h.package,
		})
		.collect();
	if hits.is_empty() {
		return Err(ProviderError::NoMatch("search returned nothing".into()));
	}
	Ok(hits)
}

/// `listAppVersions`, returned value is version sorted.
pub fn parse_versions(body: &str) -> Result<Vec<App>> {
	let r: VersionsResp = json(body)?;
	let apps = r.list;
	if apps.is_empty() {
		return Err(ProviderError::NoMatch("no versions listed".into()));
	}
	// Assume list is sorted by version
	Ok(apps)
}
