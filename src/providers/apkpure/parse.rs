use chrono::NaiveDate;
use scraper::CaseSensitivity::CaseSensitive;
use scraper::{ElementRef, Html};
use versions::Versioning;

use crate::common::Arch;
use crate::common::contract::{AppResult, ProviderError, Result};
use crate::common::ui::error;
use crate::providers::scrape::{parse_date, sel, text_of};

pub type Url = crate::common::contract::Url<super::ApkPure>;

/// APKPure sometimes formats a version as `2.6.2(20653)`
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
	let ok = dots >= 2 && seg.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_'));
	ok.then(|| seg.to_string())
}

/// Parse `/search?q=...`. Covers both result shapes: the top "brand" match
/// (`div.sa-apps-div`) and the `a.dd` list rows.
pub fn parse_search(html: &Html) -> Result<Vec<AppResult>> {
	let anchor = sel("a[href*=\"apkpure.com/\"]");
	let p1 = sel("p.p1");

	let mut seen = std::collections::HashSet::new();
	let mut hits = Vec::new();
	for a in html.select(&anchor) {
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
		hits.push(AppResult { package, title });
	}
	if hits.is_empty() {
		return Err(ProviderError::NoMatch("search returned nothing".into()));
	}
	Ok(hits)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
	APK,
	XAPK,
}

// pub field is needed
pub struct Variant {
	pub url: Url,
	pub version: String,
	android: u8,
	pub arch: Arch,
	kind: Kind,
}

pub fn parse_variants(html: &Html, arch_want: Arch) -> Result<Variant> {
	let variants = sel("div#version-list div.show-more div.show-more-content");
	let var = html
		.select(&variants)
		.next()
		.ok_or_else(|| ProviderError::ParseError("no 'All Variants' element".into()))?;

	let dl_btn = sel("a.download-btn");
	let version = sel("div.info div.info-top span");
	let sdk = sel("span.sdk");
	let mut arch: Option<Arch> = None;
	var.children()
		.filter_map(ElementRef::wrap)
		.filter_map(|el| {
			if el.value().has_class("group-title", CaseSensitive) {
				arch = text_of(el).parse().ok();
				return None;
			}
			if !el.value().has_class("apk", CaseSensitive) {
				return None;
			}
			let href = el.select(&dl_btn).next()?.attr("href")?;
			// Text like `Android 11.0+`
			let android = text_of(el.select(&sdk).next()?)
				.split_once(' ')?
				.1
				.split_once('.')?
				.0
				.parse()
				.ok()?;
			Some(Variant {
				arch: arch?,
				version: text_of(el.select(&version).next()?),
				url: href.into(),
				android,
				kind: if href.contains("/b/APK") { Kind::APK } else { Kind::XAPK },
			})
		})
		.filter(|v| v.arch.contains(arch_want))
		// Return last element on tie for larger version code.
		.max_by_key(|v| (v.android, v.kind == Kind::APK))
		.ok_or_else(|| ProviderError::no_match("no matching variant", arch_want))
}

pub struct VersionRow {
	pub version: String,
	pub uploaded: NaiveDate,
}

/// Parse `/x/<pkg>/versions` — rows carry everything in `data-dt-*` attributes.
pub fn parse_versions(html: &Html) -> Result<Vec<VersionRow>> {
	let row = sel("div.ver_download_link[data-dt-version][data-dt-versioncode]");
	let date = sel("span.update-on");
	let mut seen = std::collections::HashSet::new();
	let mut rows: Vec<VersionRow> = html
		.select(&row)
		.filter_map(|el| {
			let version = clean_version(el.value().attr("data-dt-version")?);
			if version.is_empty() || !seen.insert(version.clone()) {
				return None;
			}
			// `Apr 10, 2025`
			let date = el.select(&date).next().map(text_of).unwrap_or_default();
			let uploaded = parse_date(&date, "%b %d, %Y")
				.inspect_err(|e| error!("failed to parse date {date}: {e}"))
				.ok()?;
			Some(VersionRow { version, uploaded })
		})
		.collect();
	if rows.is_empty() {
		return Err(ProviderError::NoMatch("no versions listed".into()));
	}
	rows.sort_by(|a, b| Versioning::new(&b.version).cmp(&Versioning::new(&a.version)));
	Ok(rows)
}
