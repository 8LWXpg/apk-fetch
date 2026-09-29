use std::sync::LazyLock;

use chrono::NaiveDate;
use scraper::{Html, Selector};
use versions::Versioning;

use crate::common::contract::{Arch, ProviderError};
use crate::providers::scrape::{Variant, parse_date, sel, text_of, version_token};

pub type Url = crate::common::contract::Url<super::ApkCombo>;

/// Fallback build tag if the download page's `var xid = "..."` can't be scraped.
pub const FALLBACK_XID: &str = "01a1200x20240308";
pub const LOOKUP_LOCALE: &str = "en";

// Shared by `parse_versions` and `parse_variants`.
static VERNAME: LazyLock<Selector> = LazyLock::new(|| sel(".vername"));

/// `{BASE}/{slug}/{pkg}/{tail}`.
pub fn app_url(slug: &str, pkg: &str, tail: &str) -> Url {
	format!("{slug}/{pkg}/{tail}").into()
}
pub struct SearchHit {
	pub package: String,
	pub title: String,
}

/// The `{slug}` from a canonical app URL `{BASE}/{slug}/{pkg}/`. `None` when the
/// URL isn't that shape, or is still the `{LOOKUP_LOCALE}` one we asked for.
pub fn slug_from_canonical_url(url: &Url, pkg: &str) -> Option<String> {
	let path = url.path();
	match path.split('/').collect::<Vec<_>>()[..] {
		[slug, p] if p == pkg && slug != LOOKUP_LOCALE => Some(slug.to_string()),
		_ => None,
	}
}

pub fn parse_search(html: &Html) -> Result<Vec<SearchHit>, ProviderError> {
	let link = sel("a.l_item");
	let mut seen = std::collections::HashSet::new();
	let hits: Vec<SearchHit> = html
		.select(&link)
		.filter_map(|a| {
			let href = a.value().attr("href")?;
			let mut segs = href.trim_matches('/').split('/');
			segs.next()?; // {slug}
			let package = segs.next()?.to_string();
			if !package.contains('.') || segs.next().is_some() || !seen.insert(package.clone()) {
				return None;
			}
			let title = a
				.value()
				.attr("title")
				.map(|t| t.trim_end_matches(" APK").trim().to_string())
				.filter(|s| !s.is_empty())
				.unwrap_or_else(|| text_of(a));
			Some(SearchHit { package, title })
		})
		.collect();
	if hits.is_empty() {
		return Err(ProviderError::NoMatch("search returned nothing".into()));
	}
	Ok(hits)
}

pub struct VersionRow {
	pub version: String,
	pub uploaded: NaiveDate,
	/// `/{slug}/{pkg}/download/phone-{version}-apk`
	pub download_page_url: Url,
}

/// Sorted by version.
pub fn parse_versions(html: &Html) -> Result<Vec<VersionRow>, ProviderError> {
	let row = sel("ul.list-versions li a.ver-item");
	let desc = sel(".description");
	let mut rows: Vec<VersionRow> = html
		.select(&row)
		.filter_map(|a| {
			let href = a.value().attr("href")?;
			let version = a
				.select(&VERNAME)
				.next()
				.map(|v| version_token(&text_of(v)))
				.unwrap_or_default();
			// `Sep 4, 2026 · Android 10+`
			let desc = a.select(&desc).next().map(text_of).unwrap_or_default();
			let date = desc.split('·').next().unwrap_or("");
			let uploaded = parse_date("apkcombo", &version, date, "%b %d, %Y")?;
			Some(VersionRow {
				version,
				uploaded,
				download_page_url: href.into(),
			})
		})
		.collect();
	if rows.is_empty() {
		return Err(ProviderError::NoMatch("no versions listed".into()));
	}
	rows.sort_by(|a, b| Versioning::new(&b.version).cmp(&Versioning::new(&a.version)));
	Ok(rows)
}

/// `var xid = "..."` on a download page. Falls back to [`FALLBACK_XID`].
pub fn extract_xid(html: &str) -> String {
	html.find("var xid")
		.and_then(|i| html[i..].find('"').map(|j| i + j + 1))
		.and_then(|start| html[start..].find('"').map(|end| &html[start..start + end]))
		.filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric()))
		.unwrap_or(FALLBACK_XID)
		.to_string()
}

/// [`Variant::url`] is the `/r2?u=<encoded signed URL>` link (absolute).
/// The caller parses the fragment with [`Html::parse_fragment`].
pub fn parse_variants(html: &Html) -> Result<Vec<Variant>, ProviderError> {
	// The recommended pick lives in `#best-variant-tab`; the rest in
	// `#variants-tab`. Both are `.content-tab` with the same row shape.
	let group = sel(".content-tab .tree > ul > li");
	let arch_sel = sel("span.blur code, code.blur, span.blur");
	let item = sel("ul.file-list li a.variant");
	let vtype = sel(".vtype");

	let mut variants = Vec::new();
	let mut seen = std::collections::HashSet::new();
	for g in html.select(&group) {
		// One ABI, a comma list for a split bundle, or none (= runs anywhere).
		let arch = g
			.select(&arch_sel)
			.next()
			.and_then(|c| text_of(c).parse().ok())
			.unwrap_or(Arch::all());
		for a in g.select(&item) {
			let Some(href) = a.value().attr("href") else {
				continue;
			};
			if !seen.insert(href.to_string()) {
				continue;
			}
			variants.push(Variant {
				version: version_token(&text_of(
					a.select(&VERNAME)
						.next()
						.ok_or_else(|| ProviderError::parse_error("no version found", a.html()))?,
				)),
				// `.vtype` is "APK" or "XAPK".
				bundle: a
					.select(&vtype)
					.next()
					.is_some_and(|t| text_of(t).eq_ignore_ascii_case("xapk")),
				arch,
				url: Url::from(href).to_string(),
			});
		}
	}
	if variants.is_empty() {
		return Err(ProviderError::ParseError("no variants in download fragment".into()));
	}
	Ok(variants)
}

/// `{r2_url}&{checkin}&package_name={pkg}&lang=en` — the JS `octs()` decoration.
pub fn final_download_url(r2_url: &str, checkin: &str, pkg: &str) -> String {
	format!("{r2_url}&{}&package_name={pkg}&lang=en", checkin.trim())
}
