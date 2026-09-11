use crate::common::contract::{Arch, ProviderError, VersionInfo};
use crate::providers::scrape::{Variant, abs, parse_date, parse_err, sel, text_of, version_token};
use scraper::Html;

pub const BASE_URL: &str = "https://www.apkmirror.com";

// --- search --------------------------------------------------------------------

pub struct SearchHit {
	/// `"{App name} {version}"`, e.g. `"YouTube 21.36.45"`.
	pub title: String,
	/// Release-page URL (absolute).
	pub release_url: String,
}

impl SearchHit {
	pub fn version(&self) -> String {
		version_token(&self.title)
	}
}

/// Parse the `/?post_type=app_release&s=...` results page for `query`, best
/// match first (see [`rank`]); ties keep the site's newest-first order.
pub fn parse_search(html: &str, query: &str) -> Result<Vec<SearchHit>, ProviderError> {
	// A no-results page still renders a "you might also like" grid of unrelated
	// apps, so an empty selector match isn't enough — check the marker.
	if html.contains("No results found matching your query") {
		return Err(ProviderError::NotFound("search returned nothing".into()));
	}
	// The results list ends where the "Popular / Latest Uploads" widgets begin;
	// those use `<h5 class="widgetHeader">` while the results header is a `<div>`.
	let html = match html.find(r#"<h5 class="widgetHeader">"#) {
		Some(cut) => &html[..cut],
		None => html,
	};
	let doc = Html::parse_document(html);
	let link = sel("h5.appRowTitle a.fontBlack");
	let mut hits: Vec<SearchHit> = doc
		.select(&link)
		.filter_map(|a| {
			let href = a.value().attr("href")?;
			Some(SearchHit {
				title: text_of(a),
				release_url: abs(BASE_URL, href),
			})
		})
		.collect();
	if hits.is_empty() {
		return Err(ProviderError::NotFound("search returned nothing".into()));
	}
	hits.sort_by_key(|h| rank(h, query));
	Ok(hits)
}

fn tokens(s: &str) -> impl Iterator<Item = &str> {
	s.split(|c: char| !c.is_alphanumeric())
		.filter(|t| !t.is_empty())
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

/// Sort key, lower is better. APKMirror matches the query as a blind substring
/// of title / developer / description and lists one row per release, newest
/// first, across every listing that shares a package id (phone, `-wear-os`,
/// `-beta`, ...). Two terms, in priority order:
///
/// - `coverage`: for each query token, its best [`token_match`] against the
///   version-stripped title, summed — so every word of `youtube music` counts
///   and an off-title hit sorts last.
/// - `extra`: repo-slug tokens no query token equals. The phone app is the bare
///   slug and each spin-off appends to it, so `youtube` < `youtube-beta` <
///   `youtube-wear-os`; naming the channel (`youtube beta`) lifts its penalty.
///   A package-id query ties on `coverage` and this term alone picks the app.
fn rank(hit: &SearchHit, query: &str) -> (u8, u8) {
	let q = query.to_lowercase();
	let title = strip_version(&hit.title).to_lowercase();
	let coverage = tokens(&q)
		.map(|qt| {
			tokens(&title)
				.map(|tt| token_match(qt, tt))
				.min()
				.unwrap_or(3)
		})
		.sum();
	let repo = app_slug(&hit.release_url).map_or("", |s| s.rsplit('/').next().unwrap_or(s));
	let extra = tokens(repo)
		.filter(|st| !tokens(&q).any(|qt| qt == *st))
		.count() as u8;
	(coverage, extra)
}

/// `{BASE}/apk/{org}/{repo}/{repo}-x-y-release/` -> `{org}/{repo}`.
pub fn app_slug(release_url: &str) -> Option<&str> {
	let path = release_url.strip_prefix(BASE_URL)?.strip_prefix("/apk/")?;
	let mut segs = path.split('/');
	let org = segs.next()?;
	let repo = segs.next()?;
	(!org.is_empty() && !repo.is_empty()).then(|| &path[..org.len() + 1 + repo.len()])
}

// --- versions -----------------------------------------------------------------

/// Parse the "All versions" widget on an app page.
pub fn parse_versions(html: &str) -> Result<Vec<VersionInfo>, ProviderError> {
	let doc = Html::parse_document(html);
	let widget_sel = sel("div.listWidget");
	let anchor_sel = sel(r#"a[name="all_versions"]"#);
	let row_sel = sel("div.appRow");
	let title_sel = sel("h5.appRowTitle a.fontBlack");
	let date_sel = sel("span.dateyear_utc");

	// scraper has no :has(), so find the listWidget that contains the anchor.
	let widget = doc
		.select(&widget_sel)
		.find(|w| w.select(&anchor_sel).next().is_some())
		.ok_or_else(|| parse_err("no 'all versions' widget on app page"))?;

	let rows: Vec<VersionInfo> = widget
		.select(&row_sel)
		.filter_map(|row| {
			let a = row.select(&title_sel).next()?;
			a.value().attr("href")?; // skip rows whose title isn't a real link
			let version = version_token(&text_of(a));
			// `09/7/2026 02:34 UTC`
			let date = row
				.select(&date_sel)
				.next()
				.and_then(|d| d.value().attr("data-utcdate"))
				.unwrap_or("");
			let uploaded = parse_date("apkmirror", &version, date, "%m/%d/%Y %H:%M UTC")?;
			Some(VersionInfo { version, uploaded })
		})
		.collect();
	if rows.is_empty() {
		return Err(ProviderError::NotFound("no versions listed".into()));
	}
	Ok(rows)
}

/// Drop the version number from a search-result title:
/// `"LINE: Calls & Messages 26.14.0"` -> `"LINE: Calls & Messages"`.
/// Leaves the title whole when no token is a dotted number.
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

// --- variants -----------------------------------------------------------------

/// Parse the `.variants-table` on a version page. Empty vec (not an error) if the
/// table is absent — caller may have landed straight on a download page.
/// [`Variant::url`] is the variant's download page.
pub fn parse_variants(html: &str) -> Vec<Variant> {
	let doc = Html::parse_document(html);
	let row_sel = sel("div.variants-table div.table-row");
	let cell_sel = sel("div.table-cell");
	let link_sel = sel("a.accent_color");
	let badge_sel = sel("span.apkm-badge");

	doc.select(&row_sel)
		.skip(1) // header row
		.filter_map(|row| {
			let cells: Vec<_> = row.select(&cell_sel).collect();
			let link = cells.first()?.select(&link_sel).next()?;
			let href = link.value().attr("href")?;
			// Badge is "APK" or "BUNDLE"; a missing badge is treated as a bundle
			// so it never outranks a labelled APK.
			let bundle = !cells
				.first()?
				.select(&badge_sel)
				.next()
				.is_some_and(|b| text_of(b).eq_ignore_ascii_case("apk"));
			Some(Variant {
				version: text_of(link),
				bundle,
				// No ABI cell means the build runs anywhere.
				arch: cells
					.get(1)
					.and_then(|c| text_of(*c).parse().ok())
					.unwrap_or(Arch::all()),
				url: abs(BASE_URL, href),
			})
		})
		.collect()
}

// --- download chain ----------------------------------------------------------

/// Download page -> the keyed `a.downloadButton` href (absolute).
pub fn parse_download_button(html: &str) -> Result<String, ProviderError> {
	let doc = Html::parse_document(html);
	doc.select(&sel("a.downloadButton"))
		.find_map(|a| a.value().attr("href"))
		.map(|h| abs(BASE_URL, h))
		.ok_or_else(|| parse_err("no download button on download page"))
}

/// "Your download is starting..." page -> the actual APK URL (absolute).
pub fn parse_final_link(html: &str) -> Result<String, ProviderError> {
	let doc = Html::parse_document(html);
	doc.select(&sel("a#download-link"))
		.chain(doc.select(&sel("div.card-with-tabs a[href]")))
		.find_map(|a| a.value().attr("href"))
		.map(|h| abs(BASE_URL, h))
		.ok_or_else(|| parse_err("no final download link on 'starting' page"))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::providers::fixtures::{app_dirs, read};
	use crate::providers::scrape::choose_variant;

	#[test]
	fn search_pages_parse() {
		for (app, dir) in app_dirs("apkmirror") {
			let html = read(&dir, "search.html");

			// A dir whose search page shows the "no results" marker (e.g.
			// `nonexistent/`) must parse as NotFound — not junk hits from the
			// "Popular / Latest Uploads" widgets further down the page.
			if html.contains("No results found matching your query") {
				assert!(
					matches!(parse_search(&html, &app), Err(ProviderError::NotFound(_))),
					"{app}: no-results page didn't parse as NotFound"
				);
				continue;
			}

			let hits = parse_search(&html, &app).unwrap_or_else(|e| panic!("{app}: {e}"));
			// First hit is the phone app, not the automotive/wear spin-off that
			// uploaded last.
			assert!(
				hits[0].release_url.contains(&format!("/{app}/{app}-")),
				"{app}: picked {}",
				hits[0].release_url
			);
			for h in &hits {
				assert!(h.release_url.contains("/apk/"), "{app}: {}", h.release_url);
				assert!(
					h.release_url.ends_with("-release/"),
					"{app}: {}",
					h.release_url
				);
				assert!(!h.title.trim().is_empty(), "{app}: blank title");
			}
		}
	}

	#[test]
	fn download_chains_parse() {
		for (app, dir) in app_dirs("apkmirror") {
			if !dir.join("app.html").is_file() {
				continue; // search-only dir (e.g. `nonexistent/`)
			}

			let rows = parse_versions(&read(&dir, "app.html"))
				.unwrap_or_else(|e| panic!("{app}: parse_versions: {e}"));
			assert!(!rows.is_empty(), "{app}: no version rows");
			assert!(
				rows.iter().any(|r| {
					r.version.contains('.') && r.version.starts_with(|c: char| c.is_ascii_digit())
				}),
				"{app}: no release-number-shaped versions"
			);

			let variants = parse_variants(&read(&dir, "version.html"));
			assert!(!variants.is_empty(), "{app}: no variant rows");
			for v in &variants {
				assert!(v.url.contains("-download/"), "{app}: {}", v.url);
			}
			// If the app publishes any plain APK, arch selection must land on one.
			let picked = choose_variant(&variants, Arch::ARM64_V8A).expect("a variant");
			if variants.iter().any(|v| !v.bundle) {
				assert!(!picked.bundle, "{app}: picked a bundle");
			}

			let btn = parse_download_button(&read(&dir, "download-page.html"))
				.unwrap_or_else(|e| panic!("{app}: parse_download_button: {e}"));
			assert!(btn.contains("/download/?key="), "{app}: {btn}");
			let final_url = parse_final_link(&read(&dir, "download-starting.html"))
				.unwrap_or_else(|e| panic!("{app}: parse_final_link: {e}"));
			assert!(
				final_url.contains("download.php") || final_url.contains("downloadr"),
				"{app}: {final_url}"
			);
		}
	}

	#[test]
	fn derives_app_slug() {
		assert_eq!(
			app_slug("https://www.apkmirror.com/apk/mozilla/firefox/firefox-x-y-release/"),
			Some("mozilla/firefox")
		);
		assert_eq!(app_slug("https://www.apkmirror.com/apk/mozilla/"), None);
		assert_eq!(app_slug("https://elsewhere/apk/a/b/"), None);
	}

	#[test]
	fn strips_version_from_any_position() {
		assert_eq!(
			strip_version("LINE: Calls & Messages 26.14.0"),
			"LINE: Calls & Messages"
		);
		// version is not the last token
		assert_eq!(strip_version("YouTube 21.36.42 beta"), "YouTube beta");
		// no dotted number -> title kept whole
		assert_eq!(strip_version("Some App"), "Some App");
		assert_eq!(strip_version("2nd Line"), "2nd Line");
	}

	fn hit(title: &str, repo: &str) -> SearchHit {
		SearchHit {
			title: title.into(),
			release_url: format!("{BASE_URL}/apk/org/{repo}/{repo}-1-release/"),
		}
	}

	#[test]
	fn rank_covers_query_tokens_then_penalises_slug_extras() {
		let r = |title: &str, repo: &str, q: &str| rank(&hit(title, repo), q);
		// coverage rungs: whole word < prefix < substring < absent
		assert!(
			r("LINE Camera 1.0", "line-camera", "line") < r("Lineage2M 1.0", "lineage2m", "line")
		);
		assert!(
			r("Lineage2M 1.0", "lineage2m", "line")
				< r("Airline Manager", "airline-manager", "line")
		);
		assert!(
			r("Airline Manager", "airline-manager", "line")
				< r("Korean Air My", "korean-air-my", "line")
		);
		// every query token counts, order-free; the version token never does
		assert_eq!(
			r("YouTube Music 9.3", "youtube-music", "youtube music"),
			(0, 0)
		);
		assert_eq!(r("Music - YouTube", "youtube-music", "youtube music").0, 0);
		assert_eq!(r("YouTube 21.3", "youtube", "youtube music").0, 3);
		// package-id query: coverage ties, uncovered slug tokens pick the phone app
		let pkg = "com.google.android.youtube";
		let phone = r("YouTube 1.0", "youtube", pkg);
		let beta = r("YouTube 1.2 beta", "youtube-beta", pkg);
		let wear = r("YouTube 1.1", "youtube-wear-os", pkg);
		assert!(phone < beta && beta < wear);
		// asking for the channel lifts its penalty; `dev` in a package id is a
		// token, not a substring, so `com.devhd.x` doesn't pick `-dev`
		assert!(
			r("YouTube beta", "youtube-beta", "youtube beta")
				< r("YouTube", "youtube", "youtube beta")
		);
		assert!(
			r("Feedly", "feedly", "com.devhd.feedly")
				< r("Feedly", "feedly-dev", "com.devhd.feedly")
		);
	}
}
