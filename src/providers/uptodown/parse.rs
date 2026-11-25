use std::cmp::Reverse;

use chrono::NaiveDate;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use versions::Versioning;

use crate::common::contract::{AppResult, ProviderError, Result, VersionInfo};
use crate::providers::scrape::{newest_first, parse_date, version_matches};

pub type Url = crate::common::contract::Url<super::Uptodown>;

/// One downloadable build from the versions list.
#[derive(Debug, Clone)]
pub struct Row {
	pub version: String,
	pub file_id: u64,
	/// Split bundle (XAPK / APKS / APKM) rather than a plain APK.
	pub bundle: bool,
	pub uploaded: NaiveDate,
}

fn json<T: DeserializeOwned>(body: &str) -> Result<T> {
	serde_json::from_str(body).map_err(|e| ProviderError::parse_error("uptodown json", e))
}

fn data<T: DeserializeOwned>(body: &str) -> Result<Option<T>> {
	/// `{"success":1,"data":...}`. `data` is absent on the error shape.
	#[derive(Deserialize)]
	struct Envelope<T> {
		data: Option<T>,
	}

	Ok(json::<Envelope<T>>(body)?.data)
}

pub fn parse_token(body: &str) -> Result<String> {
	#[derive(Deserialize)]
	struct TokenResp {
		token: String,
	}

	Ok(json::<TokenResp>(body)?.token)
}

pub fn parse_app_id(body: &str, pkg: &str) -> Result<String> {
	#[derive(Deserialize)]
	struct Lookup {
		#[serde(rename = "appID")]
		app_id: u64,
	}

	data::<Lookup>(body)?
		.map(|d| d.app_id.to_string())
		.ok_or_else(|| ProviderError::no_match("no such app", pkg))
}

pub fn parse_search(body: &str) -> Result<Vec<AppResult>> {
	#[derive(Deserialize)]
	struct SearchData {
		results: Vec<SearchItem>,
	}

	#[derive(Deserialize)]
	struct SearchItem {
		/// This somehow can be null when searching.
		#[serde(rename = "packageName")]
		package: Option<String>,
		name: String,
	}

	let hits: Vec<AppResult> = data::<SearchData>(body)?
		.map(|d| d.results)
		.unwrap_or_default()
		.into_iter()
		.filter_map(|h| {
			Some(AppResult {
				package: h.package?,
				title: h.name,
			})
		})
		.collect();
	if hits.is_empty() {
		return Err(ProviderError::NoMatch("search returned nothing".into()));
	}
	Ok(hits)
}

pub fn parse_download(body: &str) -> Result<String> {
	#[derive(Deserialize)]
	struct DownloadResp {
		#[serde(rename = "downloadURL")]
		download_url: String,
		// sha256: Option<String>,
	}

	let d = data::<DownloadResp>(body)?
		.ok_or_else(|| ProviderError::ParseError("no `data` in download response".into()))?;
	Ok(d.download_url)
}

/// One list page. Empty `Vec` = past the last page. A bad row fails the page: dropping it
/// could turn a pinned version into a false "not found".
pub fn parse_page(body: &str) -> Result<Vec<Row>> {
	#[derive(Deserialize)]
	struct Item {
		#[serde(rename = "fileID")]
		file_id: u64,
		version: String,
		#[serde(rename = "lastUpdate")]
		last_update: String,
		#[serde(rename = "kindFile", default)]
		kind: String,
	}

	data::<Vec<Item>>(body)?
		.unwrap_or_default()
		.into_iter()
		.map(|it| -> Result<Row> {
			// `Nov 20, 2026`, English only with `lang=en`.
			let uploaded = parse_date(&it.last_update, "%b %d, %Y").map_err(|_| {
				ProviderError::parse_error("bad uptodown date (is `lang=en` honored?)", &it.last_update)
			})?;
			Ok(Row {
				version: it.version.trim().to_string(),
				file_id: it.file_id,
				bundle: matches!(it.kind.as_str(), "xapk" | "apks" | "apkm"),
				uploaded,
			})
		})
		.collect()
}

/// Newest build matching `want`. Latest skips `-EXTENDED` / `-SECONDARY` style builds; a pinned
/// version uses `version_matches`. Among builds of the winning version, a plain APK wins.
pub fn pick<'a>(rows: &'a [Row], want: Option<&str>) -> Option<&'a Row> {
	rows.iter()
		.filter(|r| match want {
			Some(w) => version_matches(&r.version, w),
			None => true,
		})
		.max_by_key(|r| (Versioning::new(&r.version), Reverse(r.bundle)))
}

/// One entry per version (a version can list several builds), newest first.
pub fn into_versions(rows: Vec<Row>) -> Result<Vec<VersionInfo>> {
	let mut seen = std::collections::HashSet::new();
	let mut out: Vec<VersionInfo> = rows
		.into_iter()
		.filter(|r| seen.insert(r.version.clone()))
		.map(|r| VersionInfo {
			version: r.version,
			uploaded: r.uploaded,
		})
		.collect();
	if out.is_empty() {
		return Err(ProviderError::NoMatch("no versions listed".into()));
	}
	newest_first(&mut out, |v| &v.version);
	Ok(out)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn row(version: &str, bundle: bool) -> Row {
		Row {
			version: version.into(),
			file_id: 1,
			bundle,
			uploaded: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
		}
	}

	#[test]
	fn empty_or_missing_data_is_end_of_list() {
		assert!(parse_page(r#"{"success":1,"data":[]}"#).unwrap().is_empty());
		assert!(parse_page(r#"{"success":0,"errorCode":-84}"#).unwrap().is_empty());
	}

	#[test]
	fn pick_skips_suffixed_for_latest_but_not_when_pinned() {
		let rows = vec![
			row("21.41.526", false),
			row("21.41.525-EXTENDED", false),
			row("21.39.522-SECONDARY", false),
			row("21.39.522", false),
		];
		assert_eq!(pick(&rows, None).unwrap().version, "21.41.526");
		assert_eq!(pick(&rows, Some("21.39.522")).unwrap().version, "21.39.522");
		assert_eq!(
			pick(&rows, Some("21.41.525-EXTENDED")).unwrap().version,
			"21.41.525-EXTENDED"
		);
		assert!(pick(&rows, Some("21.41.525")).is_none());
	}
}
