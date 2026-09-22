use crate::common::contract::{Arch, ProviderError};
use crate::providers::scrape::{Variant, parse_date, parse_err, sel, text_of, version_token};

use chrono::NaiveDate;
use scraper::Html;

pub type Url = crate::common::contract::Url<super::ApkCombo>;

/// Fallback build tag if the download page's `var xid = "..."` can't be scraped.
pub const FALLBACK_XID: &str = "01a1200x20240308";
pub const LOOKUP_LOCALE: &str = "en";

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
	let path = url.path()?;
	match path.split('/').collect::<Vec<_>>()[..] {
		[slug, p] if p == pkg && slug != LOOKUP_LOCALE => Some(slug.to_string()),
		_ => None,
	}
}

pub fn parse_search(html: &str) -> Result<Vec<SearchHit>, ProviderError> {
	let doc = Html::parse_document(html);
	let link = sel("a.l_item");
	let mut seen = std::collections::HashSet::new();
	let hits: Vec<SearchHit> = doc
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
		return Err(ProviderError::NotFound("search returned nothing".into()));
	}
	Ok(hits)
}

pub struct VersionRow {
	pub version: String,
	pub uploaded: NaiveDate,
	/// `/{slug}/{pkg}/download/phone-{version}-apk`
	pub download_page_url: Url,
}

pub fn parse_versions(html: &str) -> Result<Vec<VersionRow>, ProviderError> {
	let doc = Html::parse_document(html);
	let row = sel("ul.list-versions li a.ver-item");
	let vername = sel(".vername");
	let desc = sel(".description");
	let rows: Vec<VersionRow> = doc
		.select(&row)
		.filter_map(|a| {
			let href = a.value().attr("href")?;
			let version = a
				.select(&vername)
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
		return Err(ProviderError::NotFound("no versions listed".into()));
	}
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
pub fn parse_variants(fragment: &str) -> Result<Vec<Variant>, ProviderError> {
	let doc = Html::parse_fragment(fragment);
	// The recommended pick lives in `#best-variant-tab`; the rest in
	// `#variants-tab`. Both are `.content-tab` with the same row shape.
	let group = sel(".content-tab .tree > ul > li");
	let arch_sel = sel("span.blur code, code.blur, span.blur");
	let item = sel("ul.file-list li a.variant");
	let vername = sel(".vername");
	let vtype = sel(".vtype");

	let mut variants = Vec::new();
	let mut seen = std::collections::HashSet::new();
	for g in doc.select(&group) {
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
				version: a
					.select(&vername)
					.next()
					.map(|v| version_token(&text_of(v)))
					.unwrap_or_default(),
				// `.vtype` is "APK" or "XAPK".
				bundle: a
					.select(&vtype)
					.next()
					.is_some_and(|t| text_of(t).eq_ignore_ascii_case("xapk")),
				arch,
				url: Url::from(href).into_string(),
			});
		}
	}
	if variants.is_empty() {
		return Err(parse_err("no variants in download fragment"));
	}
	Ok(variants)
}

/// `{r2_url}&{checkin}&package_name={pkg}&lang=en` — the JS `octs()` decoration.
pub fn final_download_url(r2_url: &str, checkin: &str, pkg: &str) -> String {
	format!("{r2_url}&{}&package_name={pkg}&lang=en", checkin.trim())
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::providers::fixtures::{app_dirs, read};
	use crate::providers::scrape::choose_variant;

	/// `{pkg}` out of a `/{slug}/{pkg}/download/phone-{v}-apk` URL.
	fn pkg_from_download_url(url: &Url) -> Option<String> {
		let path = url.path()?;
		let pkg = path.split('/').nth(1)?;
		pkg.contains('.').then(|| pkg.to_string())
	}

	#[test]
	fn search_and_versions_and_variants_parse() {
		for (app, dir) in app_dirs("apkcombo") {
			let hits = parse_search(&read(&dir, "search.html"))
				.unwrap_or_else(|e| panic!("{app}: search: {e}"));
			assert!(!hits.is_empty(), "{app}: empty hit list");
			for h in &hits {
				assert!(
					h.package.contains('.'),
					"{app}: bad package {:?}",
					h.package
				);
				assert!(!h.title.trim().is_empty(), "{app}: blank title");
			}

			let vers = parse_versions(&read(&dir, "old-versions.html"))
				.unwrap_or_else(|e| panic!("{app}: versions: {e}"));
			assert!(!vers.is_empty(), "{app}: no version rows");

			// Cross-check the fixtures against each other: the package the
			// version pages are for must be one the search page actually found.
			// Catches a dir whose files were captured for different apps, and the
			// "we fetched a generic page" class that a per-file assert sails past.
			let pkg = pkg_from_download_url(&vers[0].download_page_url)
				.unwrap_or_else(|| panic!("{app}: no package in {}", vers[0].download_page_url));
			assert!(
				hits.iter().any(|h| h.package == pkg),
				"{app}: {pkg} missing from search"
			);
			assert!(
				vers[0]
					.download_page_url
					.as_str()
					.contains("/download/phone-")
			);
			assert!(vers.iter().any(|v| {
				v.version.contains('.') && v.version.starts_with(|c: char| c.is_ascii_digit())
			}));

			assert!(!extract_xid(&read(&dir, "download-page.html")).is_empty());

			let variants = parse_variants(&read(&dir, "variants.html"))
				.unwrap_or_else(|e| panic!("{app}: variants: {e}"));
			for v in &variants {
				assert!(v.url.contains("/r2?u="), "{app}: {}", v.url);
			}
			// Every fixture app publishes a plain APK; the pick must land on one.
			assert!(variants.iter().any(|v| !v.bundle), "{app}: no plain APK");
			assert!(!choose_variant(&variants, Arch::ARM64_V8A).unwrap().bundle);
		}
	}

	#[test]
	fn slug_off_the_redirect() {
		let yt = "com.google.android.youtube";
		// `/en/{pkg}/` redirected to the canonical page: first segment is the slug.
		assert_eq!(
			slug_from_canonical_url(&format!("/youtube/{yt}/").into(), yt).as_deref(),
			Some("youtube")
		);
		// Never redirected — APKCombo has no page for it.
		assert_eq!(
			slug_from_canonical_url(&format!("{LOOKUP_LOCALE}/{yt}/").into(), yt),
			None
		);
		// Landed on a different app's page.
		assert_eq!(
			slug_from_canonical_url(&format!("/spotify/com.spotify.music/").into(), yt),
			None
		);
	}

	#[test]
	fn xid_extraction() {
		assert_eq!(
			extract_xid(r#"...<script>var xid = "abc123def"</script>..."#),
			"abc123def"
		);
		assert_eq!(extract_xid("no xid here"), FALLBACK_XID);
	}
}
