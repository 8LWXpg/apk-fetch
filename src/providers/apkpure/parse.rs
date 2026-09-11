use crate::common::contract::ProviderError;
use crate::providers::scrape::{parse_date, sel, text_of};
use chrono::NaiveDate;
use scraper::Html;

pub const BASE_URL: &str = "https://apkpure.com";
/// Direct-download host: `{DL_URL}/b/{APK|XAPK}/{pkg}?versionCode={c}` 302s to the file.
pub const DL_URL: &str = "https://d.apkpure.com";

/// apkpure sometimes formats a version as `2.6.2(20653)`
fn clean_version(v: &str) -> String {
	match v.trim().split_once('(') {
		Some((name, _)) if !name.trim().is_empty() => name.trim().to_string(),
		_ => v.trim().to_string(),
	}
}

/// A package id is the last path segment when it looks like `a.b.c`.
fn package_from_href(href: &str) -> Option<String> {
	let seg = href.trim_end_matches('/').rsplit('/').next()?;
	let dots = seg.matches('.').count();
	let ok = dots >= 2
		&& seg
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_'));
	ok.then(|| seg.to_string())
}

// --- search -------------------------------------------------------------------

pub struct SearchHit {
	pub package: String,
	pub title: String,
}

/// Parse `/search?q=...`. Covers both result shapes: the top "brand" match
/// (`div.sa-apps-div`) and the `a.dd` list rows.
pub fn parse_search(html: &str) -> Result<Vec<SearchHit>, ProviderError> {
	let doc = Html::parse_document(html);
	let anchor = sel("a[href*=\"apkpure.com/\"]");
	let p1 = sel("p.p1");

	let mut seen = std::collections::HashSet::new();
	let mut hits = Vec::new();
	for a in doc.select(&anchor) {
		let Some(href) = a.value().attr("href") else {
			continue;
		};
		let Some(package) = package_from_href(href) else {
			continue;
		};
		let Some(title_el) = a.select(&p1).next() else {
			continue;
		};
		let title = text_of(title_el);
		if title.is_empty() || !seen.insert(package.clone()) {
			continue;
		}
		hits.push(SearchHit { package, title });
	}
	if hits.is_empty() {
		return Err(ProviderError::NotFound("search returned nothing".into()));
	}
	Ok(hits)
}

// --- versions ----------------------------------------------------------------

pub struct VersionRow {
	pub version: String,
	/// Version code for download.
	pub code: String,
	pub uploaded: NaiveDate,
}

/// Parse `/x/<pkg>/versions` — rows carry everything in `data-dt-*` attrs.
pub fn parse_versions(html: &str) -> Result<Vec<VersionRow>, ProviderError> {
	let doc = Html::parse_document(html);
	let row = sel("div.ver_download_link[data-dt-version][data-dt-versioncode]");
	let date = sel("span.update-on");
	let mut seen = std::collections::HashSet::new();
	let rows: Vec<VersionRow> = doc
		.select(&row)
		.filter_map(|el| {
			let version = clean_version(el.value().attr("data-dt-version")?);
			let code = el.value().attr("data-dt-versioncode")?.trim().to_string();
			if version.is_empty() || code.is_empty() || !seen.insert(version.clone()) {
				return None;
			}
			// `Apr 10, 2025`
			let date = el.select(&date).next().map(text_of).unwrap_or_default();
			let uploaded = parse_date("apkpure", &version, &date, "%b %d, %Y")?;
			Some(VersionRow {
				version,
				code,
				uploaded,
			})
		})
		.collect();
	if rows.is_empty() {
		return Err(ProviderError::NotFound("no versions listed".into()));
	}
	Ok(rows)
}

/// The latest version string from an app page main download button, else first
/// `data-dt-version`. `None` if the page has no such marker.
pub fn latest_version(html: &str) -> Option<String> {
	let doc = Html::parse_document(html);
	let main = sel(".dt-main-download-btn[data-dt-version]");
	let any = sel("[data-dt-version]");
	doc.select(&main)
		.chain(doc.select(&any))
		.find_map(|el| el.value().attr("data-dt-version"))
		.map(clean_version)
		.filter(|s| !s.is_empty())
}

/// `{DL_URL}/b/APK/{pkg}?versionCode={code}`, or `?version=latest` when `code` is `None`.
pub fn download_url(pkg: &str, code: Option<&str>) -> String {
	match code {
		Some(code) => format!("{DL_URL}/b/APK/{pkg}?versionCode={code}"),
		None => format!("{DL_URL}/b/APK/{pkg}?version=latest"),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::providers::fixtures::{app_dirs, read};

	/// The package id an app page is for — the last segment of its canonical URL.
	fn app_package(app_html: &str) -> Option<String> {
		let doc = Html::parse_document(app_html);
		let href = doc
			.select(&sel(r#"link[rel="canonical"]"#))
			.find_map(|el| el.value().attr("href"))?;
		let seg = href.trim_end_matches('/').rsplit('/').next()?;
		seg.contains('.').then(|| seg.to_string())
	}

	#[test]
	fn search_pages_parse() {
		for (app, dir) in app_dirs("apkpure") {
			let hits = parse_search(&read(&dir, "search.html"))
				.unwrap_or_else(|e| panic!("{app}: parse_search: {e}"));
			assert!(!hits.is_empty(), "{app}: empty hit list");
			for h in &hits {
				assert!(
					h.package.contains('.'),
					"{app}: bad package {:?}",
					h.package
				);
				assert!(!h.title.trim().is_empty(), "{app}: blank title");
			}
			// no duplicate packages
			let mut pkgs: Vec<_> = hits.iter().map(|h| &h.package).collect();
			let n = pkgs.len();
			pkgs.sort();
			pkgs.dedup();
			assert_eq!(pkgs.len(), n, "{app}: duplicate packages in results");

			// the app this dir is named for shows up in its own search results
			if let Some(pkg) = app_package(&read(&dir, "app.html")) {
				assert!(
					hits.iter().any(|h| h.package == pkg),
					"{app}: {pkg} missing from results"
				);
			}
		}
	}

	#[test]
	fn app_and_versions_pages_parse() {
		for (app, dir) in app_dirs("apkpure") {
			let v = latest_version(&read(&dir, "app.html"))
				.unwrap_or_else(|| panic!("{app}: no version marker on app page"));
			assert!(!v.contains('('), "{app}: (code) suffix not stripped: {v}");
			assert!(
				v.contains('.') && v.starts_with(|c: char| c.is_ascii_digit()),
				"{app}: odd version {v}"
			);

			let rows = parse_versions(&read(&dir, "versions.html"))
				.unwrap_or_else(|e| panic!("{app}: parse_versions: {e}"));
			assert!(rows.len() > 3, "{app}: only {} versions", rows.len());
			assert!(
				rows.iter()
					.all(|r| !r.version.contains('(') && r.version.contains('.')),
				"{app}: a version string is malformed"
			);
			assert!(
				rows.iter()
					.all(|r| r.code.chars().all(|c| c.is_ascii_digit())),
				"{app}: a versionCode is not numeric"
			);
		}
	}
}
