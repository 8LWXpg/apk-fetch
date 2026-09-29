use std::sync::LazyLock;

use scraper::{ElementRef, Html, Selector};
use versions::Versioning;

use crate::common::contract::{Arch, ProviderError, VersionInfo};
use crate::providers::scrape::{Variant, parse_date, sel, text_of, version_token};

pub type Url = crate::common::contract::Url<super::ApkMirror>;

static APP_ROW: LazyLock<Selector> = LazyLock::new(|| sel("div.appRow"));

pub struct SearchHit {
	/// `"{App name} {version}"`, e.g. `"YouTube 21.36.45"`.
	pub title: String,
	pub release_url: Url,
	pub package_id: String,
}

impl SearchHit {
	pub fn version(&self) -> String {
		version_token(&self.title)
	}
}

fn parse_row(row: ElementRef) -> Result<SearchHit, ProviderError> {
	let a = row
		.select(&sel("a.fontBlack"))
		.next()
		.ok_or_else(|| ProviderError::parse_error("title link", row.html()))?;
	let href = a
		.attr("href")
		.ok_or_else(|| ProviderError::parse_error("title href", a.html()))?;
	let img = row
		.select(&sel("img"))
		.next()
		.ok_or_else(|| ProviderError::parse_error("icon", row.html()))?;
	Ok(SearchHit {
		title: text_of(a),
		release_url: href.into(),
		package_id: package_id(img)?,
	})
}

/// Parse the `/?post_type=app_release&s=...` results page for `query`, best
/// match first (see [`rank`]); ties keep the site's order.
///
/// # Returns
/// Nonzero length `Vec`
pub fn parse_search(html: &Html, query: &str) -> Result<Vec<SearchHit>, ProviderError> {
	// Select search result div.
	let results = html.select(&sel(".listWidget")).next().unwrap();
	if let Some(e) = results.select(&sel(".addPadding p")).next()
		&& text_of(e) == "No results found matching your query"
	{
		return Err(ProviderError::NoMatch("search returned nothing".into()));
	}
	let mut hits = results.select(&APP_ROW).map(parse_row).collect::<Result<Vec<_>, _>>()?; // first bad row aborts with its ParseError
	hits.sort_by_cached_key(|h| rank(h, query));
	Ok(hits)
}

/// Get Package ID from `<img>`. Package ID is somehow used in app icon URL.
fn package_id(img: ElementRef) -> Result<String, ProviderError> {
	let src = img
		.attr("src")
		.ok_or_else(|| ProviderError::parse_error("no src", img.html()))?;
	// `wp-content/themes/APKMirror/ap_resize/ap_resize.php?src=https%3A%2F%2Fdownloadr2.apkmirror.com%2Fwp-content%2Fuploads%2F2024%2F10%2F21%2F67189d60d72a1_com.google.android.youtube.png&w=32&h=32&q=100`
	let start = src
		.rfind('_')
		.ok_or_else(|| ProviderError::parse_error("malformed img src", src))?
		+ 1;
	let stop = src
		.rfind(".png")
		.ok_or_else(|| ProviderError::parse_error("malformed img src", src))?;
	Ok(src[start..stop].into())
}

/// Rank a **name** query's hits, because APKMirror's own search is a plain
/// substring check with alphabetic ordering. Two terms, in priority order:
///
/// - `coverage`: for each query token, its best [`token_match`] against the
///   version-stripped title, summed — so every word of `youtube music` counts
///   and an off-title hit sorts last.
/// - `extra`: the spin-off penalty. Naming the channel (`youtube beta`) lifts it.
pub fn rank(hit: &SearchHit, query: &str) -> (u8, u8) {
	fn tokens(s: &str) -> impl Iterator<Item = &str> {
		s.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty())
	}

	/// How well one query token is matched by one title token; 0 = exact.
	fn token_match(q: &str, t: &str) -> u8 {
		if t == q {
			0
		} else if t.starts_with(q) {
			1
		} else if t.contains(q) {
			2
		} else {
			3
		}
	}

	/// For each query token, its best [`token_match`] against `subject`, summed.
	fn coverage(q: &str, subject: &str) -> u8 {
		let subject = subject.to_lowercase();
		tokens(q)
			.map(|qt| tokens(&subject).map(|tt| token_match(qt, tt)).min().unwrap_or(3))
			.sum()
	}

	/// Repo-slug tokens no query token equals. The phone app is the bare slug and
	/// each spin-off appends to it, so `youtube` < `youtube-beta` < `youtube-wear-os`.
	fn extra_slug_tokens(hit: &SearchHit, q: &str) -> u8 {
		let repo = app_slug(&hit.release_url).map_or_default(|s| s.rsplit('/').next().unwrap_or(s));
		tokens(repo).filter(|st| !tokens(q).any(|qt| qt == *st)).count() as u8
	}

	let q = query.to_lowercase();
	(coverage(&q, &strip_version(&hit.title)), extra_slug_tokens(hit, &q))
}

/// `{BASE}/apk/{org}/{repo}/{repo}-x-y-release/` -> `{org}/{repo}`.
pub fn app_slug(release_url: &Url) -> Option<&str> {
	let path = release_url.path().strip_prefix("apk/")?;
	let mut segs = path.split('/');
	let org = segs.next()?;
	let repo = segs.next()?;
	(!org.is_empty() && !repo.is_empty()).then(|| &path[..org.len() + 1 + repo.len()])
}

/// Get latest version from "All versions" widget on app page.
pub fn latest_version(html: &Html) -> Result<SearchHit, ProviderError> {
	let widget = html
		.select(&sel("div.listWidget.p-relative"))
		.next()
		.ok_or_else(|| ProviderError::ParseError("no 'All versions' widget on app page".into()))?;

	let hits = widget.select(&APP_ROW).map(parse_row).collect::<Result<Vec<_>, _>>()?;
	Ok(hits.into_iter().max_by_key(|h| Versioning::new(h.version())).unwrap())
}

/// Parse the "All versions" widget on app page.
pub fn parse_versions(html: &Html) -> Result<Vec<VersionInfo>, ProviderError> {
	let widget = html
		.select(&sel("div.listWidget.p-relative"))
		.next()
		.ok_or_else(|| ProviderError::ParseError("no 'All versions' widget on app page".into()))?;
	let title_sel = sel("h5.appRowTitle a.fontBlack");
	let date_sel = sel("span.dateyear_utc");

	let mut rows: Vec<VersionInfo> = widget
		.select(&APP_ROW)
		.filter_map(|row| {
			let a = row.select(&title_sel).next()?;
			a.attr("href")?; // Skip rows whose title isn't a real link
			let version = version_token(&text_of(a));
			// `09/7/2026 02:34 UTC`
			let date = row
				.select(&date_sel)
				.next()
				.and_then(|d| d.attr("data-utcdate"))
				.unwrap_or("");
			let uploaded = parse_date("apkmirror", &version, date, "%m/%d/%Y %H:%M UTC")?;
			Some(VersionInfo { version, uploaded })
		})
		.collect();
	if rows.is_empty() {
		return Err(ProviderError::NoMatch("no versions listed".into()));
	}
	rows.sort_by(|a, b| Versioning::new(&b.version).cmp(&Versioning::new(&a.version)));
	Ok(rows)
}

/// Drop the version number from a search-result title:
/// `"YouTube Music 9.35.54"` -> `"YouTube Music"`.
pub fn strip_version(title: &str) -> String {
	let tok = version_token(title);
	if tok == title || !(tok.contains('.') && tok.starts_with(|c: char| c.is_ascii_digit())) {
		return title.to_string();
	}
	// The version isn't always last ("YouTube 21.36.42 beta"), so drop the token
	// wherever it sits rather than stripping a suffix.
	title
		.split_whitespace()
		.filter(|t| *t != tok)
		.collect::<Vec<_>>()
		.join(" ")
}

/// Parse the `.variants-table` on a version page. Empty `Vec` if the table is absent.
pub fn parse_variants(html: &Html) -> Vec<Variant> {
	let row_sel = sel("div.variants-table div.table-row");
	let cell_sel = sel("div.table-cell");
	let link_sel = sel("a.accent_color");
	let badge_sel = sel("span.apkm-badge");

	html.select(&row_sel)
		.skip(1) // header row
		.filter_map(|row| {
			let cells: Vec<_> = row.select(&cell_sel).collect();
			let link = cells.first()?.select(&link_sel).next()?;
			let href = link.attr("href")?;
			// Badge is "APK" or "BUNDLE"; a missing badge is treated as a bundle.
			let bundle = !cells
				.first()?
				.select(&badge_sel)
				.next()
				.is_some_and(|b| text_of(b).eq_ignore_ascii_case("apk"));
			Some(Variant {
				version: text_of(link),
				bundle,
				// Treat no ABI cell as universal build.
				arch: cells
					.get(1)
					.and_then(|c| text_of(*c).parse().ok())
					.unwrap_or(Arch::all()),
				url: Url::from(href).to_string(),
			})
		})
		.collect()
}

/// Download page -> the keyed `a.downloadButton` href (absolute).
pub fn parse_download_button(html: &Html) -> Result<Url, ProviderError> {
	html.select(&sel("a.downloadButton"))
		.find_map(|a| a.attr("href"))
		.map(Into::into)
		.ok_or_else(|| ProviderError::ParseError("no download button on download page".into()))
}

/// "Your download is starting..." page -> the actual AAPK URL (absolute).
pub fn parse_final_link(html: &Html) -> Result<Url, ProviderError> {
	html.select(&sel("a#download-link"))
		.chain(html.select(&sel("div.card-with-tabs a[href]")))
		.find_map(|a| a.attr("href"))
		.map(Into::into)
		.ok_or_else(|| ProviderError::ParseError("no final download link on 'starting' page".into()))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn strips_version_from_any_position() {
		assert_eq!(
			strip_version("LINE: Calls & Messages 26.14.0"),
			"LINE: Calls & Messages"
		);
		// Version is not the last token
		assert_eq!(strip_version("YouTube 21.36.42 beta"), "YouTube beta");
		// No dotted number -> title kept whole
		assert_eq!(strip_version("Some App"), "Some App");
		assert_eq!(strip_version("2nd Line"), "2nd Line");
	}

	fn hit(title: &str, repo: &str) -> SearchHit {
		SearchHit {
			title: title.into(),
			release_url: format!("/apk/org/{repo}/{repo}-1-release/").into(),
			package_id: String::new(),
		}
	}

	#[test]
	fn rank_covers_query_tokens_then_penalises_slug_extras() {
		let r = |title: &str, repo: &str, q: &str| rank(&hit(title, repo), q);
		// coverage rungs: whole word < prefix < substring < absent
		assert!(r("LINE Camera 1.0", "line-camera", "line") < r("Lineage2M 1.0", "lineage2m", "line"));
		assert!(r("Lineage2M 1.0", "lineage2m", "line") < r("Airline Manager", "airline-manager", "line"));
		assert!(r("Airline Manager", "airline-manager", "line") < r("Korean Air My", "korean-air-my", "line"));
		// every query token counts, order-free; the version token never does
		assert_eq!(r("YouTube Music 9.3", "youtube-music", "youtube music"), (0, 0));
		assert_eq!(r("Music - YouTube", "youtube-music", "youtube music").0, 0);
		assert_eq!(r("YouTube 21.3", "youtube", "youtube music").0, 3);
		// `package-id` query: coverage ties, uncovered slug tokens pick the phone app
		let pkg = "com.google.android.youtube";
		let phone = r("YouTube 1.0", "youtube", pkg);
		let beta = r("YouTube 1.2 beta", "youtube-beta", pkg);
		let wear = r("YouTube 1.1", "youtube-wear-os", pkg);
		assert!(phone < beta && beta < wear);
		// Asking for the channel lifts its penalty; `dev` in a package id is a
		// token, not a substring, so `com.devhd.x` doesn't pick `-dev`
		assert!(r("YouTube beta", "youtube-beta", "youtube beta") < r("YouTube", "youtube", "youtube beta"));
		assert!(r("Feedly", "feedly", "com.devhd.feedly") < r("Feedly", "feedly-dev", "com.devhd.feedly"));
	}
}
