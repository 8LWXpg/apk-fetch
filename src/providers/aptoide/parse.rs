use chrono::NaiveDate;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::common::contract::{AppResult, Arch, ProviderError};
use crate::common::ui::error;
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
		let arch: Arch = self
			.file
			.hardware
			.cpus
			.iter()
			.filter_map(|c| {
				c.replace("x86-64", "x86_64")
					.parse::<Arch>()
					.inspect_err(|e| error!("aptoide: {}: {e}", self.package))
					.ok()
			})
			.collect();
		if arch.is_empty() { Arch::all() } else { arch }
	}

	pub fn uploaded(&self) -> Result<NaiveDate, ProviderError> {
		parse_date(&self.file.added, "%Y-%m-%d %H:%M:%S")
			.ok_or_else(|| ProviderError::parse_error("bad json field file.added", &self.file.added))
	}
}

fn json<T: DeserializeOwned>(body: &str) -> Result<T, ProviderError> {
	serde_json::from_str(body).map_err(|e| ProviderError::parse_error("aptoide json", e))
}

/// `apps/search`: one hit per package (the site lists one per uploader store).
pub fn parse_search(body: &str) -> Result<Vec<AppResult>, ProviderError> {
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

/// `listAppVersions`, newest first. At equal versions the build with the fewest ABIs first
/// (mirrors `choose_variant`). The same version from several stores stays: distinct candidates.
pub fn parse_versions(body: &str) -> Result<Vec<App>, ProviderError> {
	let r: VersionsResp = json(body)?;
	let apps = r.list;
	if apps.is_empty() {
		return Err(ProviderError::NoMatch("no versions listed".into()));
	}
	// Assume list is sorted by version
	Ok(apps)
}
