use std::cmp::Reverse;

use chrono::NaiveDate;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use versions::Versioning;

use crate::common::contract::{AppResult, Arch, ProviderError, Result, VersionInfo};
use crate::providers::scrape::{newest_first, parse_date, version_matches};

pub type Url = crate::common::contract::Url<super::Uptodown>;

/// One downloadable build from the versions list.
#[derive(Debug, Clone)]
pub struct Row {
	pub version: String,
	pub file_id: u64,
	/// Split bundle (XAPK / APKS / APKM) rather than a plain APK.
	pub bundle: bool,
	pub arch: Arch,
	pub uploaded: NaiveDate,
}

pub struct Download {
	pub url: String,
	#[allow(dead_code)]
	pub sha256: Option<String>,
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

/// Reply to the device registration.
pub fn parse_ack(body: &str) -> Result<()> {
	#[derive(Deserialize)]
	struct Ack {
		success: u8,
		#[serde(rename = "errorMsg")]
		error_msg: Option<String>,
	}

	let ack: Ack = json(body)?;
	if ack.success == 1 {
		return Ok(());
	}
	Err(ProviderError::parse_error(
		"device registration rejected",
		ack.error_msg.unwrap_or_default(),
	))
}

/// The device the app registers. Claims a recent SDK and every ABI so the server's
/// "compatible" filter hides nothing; `--arch` is applied by [`pick`].
pub fn device_json(id: &str) -> String {
	serde_json::json!({
		"identifier": id,
		"userID": null,
		"country": "US",
		"language": "en",
		"brand": "google",
		"model": "Pixel 8 Pro",
		"manufacturer": "Google",
		"sdk": 36,
		"displayWidth": 1080,
		"displayHeight": 2400,
		"displayDensity": 2.75,
		"density": "xxhdpi",
		"ram": 8_000_000_000u64,
		"operador": "",
		"supportedABIs": ["arm64-v8a", "armeabi-v7a", "x86_64", "x86"],
		"supported32BitsABIs": ["armeabi-v7a", "x86"],
		"supported64BitsABIs": ["arm64-v8a", "x86_64"],
		"appVersion": "Uptodown App Store v.7.45 (745)",
		"rooted": 0,
		"isTV": 0,
	})
	.to_string()
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

pub fn parse_download(body: &str) -> Result<Download> {
	#[derive(Deserialize)]
	struct DownloadResp {
		#[serde(rename = "downloadURL")]
		download_url: String,
		sha256: Option<String>,
	}

	let d = data::<DownloadResp>(body)?
		.ok_or_else(|| ProviderError::ParseError("no `data` in download response".into()))?;
	Ok(Download {
		url: d.download_url,
		sha256: d.sha256.filter(|s| !s.is_empty()),
	})
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
		abi: Option<String>,
		#[serde(default)]
		abis: Vec<String>,
		#[serde(rename = "fileType", default)]
		file_type: String,
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
				bundle: matches!(it.file_type.to_ascii_lowercase().as_str(), "xapk" | "apks" | "apkm"),
				arch: Arch::from_abis(it.abi.iter().chain(&it.abis)),
				uploaded,
			})
		})
		.collect()
}

/// Newest build that runs on `arch`. Latest skips `-EXTENDED` / `-SECONDARY` style builds; a
/// pinned version uses `version_matches`, which never matches a suffixed build by prefix.
/// Among builds of the winning version: plain APK first, then the fewest ABIs.
pub fn pick<'a>(rows: &'a [Row], want: Option<&str>, arch: Arch) -> Option<&'a Row> {
	rows.iter()
		.filter(|r| {
			r.arch.contains(arch)
				&& match want {
					Some(w) => version_matches(&r.version, w),
					None => !r.version.contains('-'),
				}
		})
		.max_by_key(|r| {
			(
				Versioning::new(&r.version),
				Reverse(r.bundle),
				Reverse(r.arch.bits().count_ones()),
			)
		})
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

	fn row(version: &str, bundle: bool, arch: Arch) -> Row {
		Row {
			version: version.into(),
			file_id: 1,
			bundle,
			arch,
			uploaded: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
		}
	}

	#[test]
	fn page_row_fields() {
		let body = r#"{"success":1,"data":[{"fileID":1241932605,"version":"21.46.528",
			"lastUpdate":"Nov 20, 2026","abi":"arm64-v8a",
			"abis":["arm64-v8a","armeabi-v7a","x86","x86_64"],"fileType":"apk","sha256":"x"}]}"#;
		let rows = parse_page(body).unwrap();
		assert_eq!(rows.len(), 1);
		assert_eq!(rows[0].file_id, 1241932605);
		assert_eq!(rows[0].arch, Arch::all());
		assert!(!rows[0].bundle);
		assert_eq!(rows[0].uploaded, NaiveDate::from_ymd_opt(2026, 11, 20).unwrap());
	}

	#[test]
	fn non_english_date_fails_the_page() {
		let body = r#"{"data":[{"fileID":1,"version":"1.0","lastUpdate":"20 nov. 2026"}]}"#;
		assert!(parse_page(body).is_err());
	}

	#[test]
	fn empty_or_missing_data_is_end_of_list() {
		assert!(parse_page(r#"{"success":1,"data":[]}"#).unwrap().is_empty());
		assert!(parse_page(r#"{"success":0,"errorCode":-84}"#).unwrap().is_empty());
	}

	#[test]
	fn ack() {
		assert!(parse_ack(r#"{"success":1}"#).is_ok());
		assert!(parse_ack(r#"{"success":0,"errorMsg":"nope"}"#).is_err());
	}

	#[test]
	fn pick_skips_suffixed_for_latest_but_not_when_pinned() {
		let rows = vec![
			row("21.41.526", false, Arch::all()),
			row("21.41.525-EXTENDED", false, Arch::all()),
			row("21.39.522-SECONDARY", false, Arch::all()),
			row("21.39.522", false, Arch::all()),
		];
		let a = Arch::ARM64_V8A;
		assert_eq!(pick(&rows, None, a).unwrap().version, "21.41.526");
		assert_eq!(pick(&rows, Some("21.39.522"), a).unwrap().version, "21.39.522");
		assert_eq!(
			pick(&rows, Some("21.41.525-EXTENDED"), a).unwrap().version,
			"21.41.525-EXTENDED"
		);
		assert!(pick(&rows, Some("21.41.525"), a).is_none());
	}

	#[test]
	fn pick_filters_by_arch_then_prefers_apk_and_fewest_abis() {
		let rows = vec![
			row("2.0", true, Arch::ARM64_V8A),
			row("2.0", false, Arch::all()),
			row("2.0", false, Arch::ARM64_V8A),
			row("1.0", false, Arch::X86),
		];
		let p = pick(&rows, None, Arch::ARM64_V8A).unwrap();
		assert!(!p.bundle);
		assert_eq!(p.arch, Arch::ARM64_V8A);
		// x86 only exists in an older build, the newest 2.0 builds don't run on it
		assert_eq!(pick(&rows, None, Arch::X86).unwrap().arch, Arch::all());
		assert!(pick(&rows, None, Arch::X86_64).unwrap().arch == Arch::all());
		assert!(pick(&[row("1.0", false, Arch::X86)], None, Arch::ARM64_V8A).is_none());
	}
}
