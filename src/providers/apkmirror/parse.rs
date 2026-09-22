use crate::common::contract::{Arch, ProviderError, VersionInfo};
use crate::providers::scrape::{Variant, parse_date, parse_err, sel, text_of, version_token};
use scraper::Html;

pub type Url = crate::common::contract::Url<super::ApkMirror>;

pub struct SearchHit {
	/// `"{App name} {version}"`, e.g. `"YouTube 21.36.45"`.
	pub title: String,
	pub release_url: Url,
}

impl SearchHit {
	pub fn version(&self) -> String {
		version_token(&self.title)
	}
}

/// Parse the `/?post_type=app_release&s=...` results page for `query`, best
/// match first (see [`rank`]); ties keep the site's newest-first order.
///
/// # Returns
/// Nonzero length `Vec`
pub fn parse_search(html: &str, query: &str) -> Result<Vec<SearchHit>, ProviderError> {
	if html.contains("No results found matching your query") {
		return Err(ProviderError::NotFound("search returned nothing".into()));
	}
	// Strip "Popular / Latest Uploads" widgets in `<h5 class="widgetHeader">`.
	let html = match html.find(r#"<h5 class="widgetHeader">"#) {
		Some(cut) => &html[..cut],
		None => html,
	};
	let doc = Html::parse_document(html);
	let link = sel("h5.appRowTitle a.fontBlack");
	let mut hits: Vec<SearchHit> = doc
		.select(&link)
		.map(|a| SearchHit {
			title: text_of(a),
			release_url: a.attr("href").unwrap().into(),
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

/// Custom ranking because APKMirror search algorithm is a simple substring
/// check with alphabetic ordering.
///
/// Two terms, in priority order:
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
	let repo = app_slug(&hit.release_url).map_or_default(|s| s.rsplit('/').next().unwrap_or(s));
	let extra = tokens(repo)
		.filter(|st| !tokens(&q).any(|qt| qt == *st))
		.count() as u8;
	(coverage, extra)
}

/// `{BASE}/apk/{org}/{repo}/{repo}-x-y-release/` -> `{org}/{repo}`.
pub fn app_slug(release_url: &Url) -> Option<&str> {
	let path = release_url.path()?.strip_prefix("apk/")?;
	let mut segs = path.split('/');
	let org = segs.next()?;
	let repo = segs.next()?;
	(!org.is_empty() && !repo.is_empty()).then(|| &path[..org.len() + 1 + repo.len()])
}

/// Get latest version from "All versions" widget on app page.
pub fn latest_version(html: &str) -> Result<SearchHit, ProviderError> {
	let doc = Html::parse_document(html);
	let widget_sel = sel("div.listWidget.p-relative");
	let title_sel = sel("h5.appRowTitle a.fontBlack");

	let widget = doc
		.select(&widget_sel)
		.next()
		.ok_or_else(|| parse_err("no 'All versions' widget on app page"))?;

	let a = widget
		.select(&title_sel)
		.next()
		.ok_or_else(|| parse_err("no item in 'All versions' widget"))?;

	Ok(SearchHit {
		title: text_of(a),
		release_url: a.attr("href").unwrap().into(),
	})
}

/// Parse the "All versions" widget on app page.
pub fn parse_versions(html: &str) -> Result<Vec<VersionInfo>, ProviderError> {
	let doc = Html::parse_document(html);
	let widget_sel = sel("div.listWidget.p-relative");
	let row_sel = sel("div.appRow");
	let title_sel = sel("h5.appRowTitle a.fontBlack");
	let date_sel = sel("span.dateyear_utc");

	let widget = doc
		.select(&widget_sel)
		.next()
		.ok_or_else(|| parse_err("no 'All versions' widget on app page"))?;

	let rows: Vec<VersionInfo> = widget
		.select(&row_sel)
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
		return Err(ProviderError::NotFound("no versions listed".into()));
	}
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
				url: Url::from(href).into_string(),
			})
		})
		.collect()
}

/// Download page -> the keyed `a.downloadButton` href (absolute).
pub fn parse_download_button(html: &str) -> Result<Url, ProviderError> {
	let doc = Html::parse_document(html);
	doc.select(&sel("a.downloadButton"))
		.find_map(|a| a.attr("href"))
		.map(Into::into)
		.ok_or_else(|| parse_err("no download button on download page"))
}

/// "Your download is starting..." page -> the actual APK URL (absolute).
pub fn parse_final_link(html: &str) -> Result<Url, ProviderError> {
	let doc = Html::parse_document(html);
	doc.select(&sel("a#download-link"))
		.chain(doc.select(&sel("div.card-with-tabs a[href]")))
		.find_map(|a| a.attr("href"))
		.map(Into::into)
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

			let hits = parse_search(&html, &app).unwrap_or_else(|e| panic!("{app}: {e}"));
			// First hit is the phone app, not the automotive/wear spin-off that uploaded last.
			assert!(
				hits[0]
					.release_url
					.as_str()
					.contains(&format!("/{app}/{app}-")),
				"{app}: picked {}",
				hits[0].release_url
			);
			for h in &hits {
				assert!(
					h.release_url.as_str().contains("/apk/"),
					"{app}: {}",
					h.release_url
				);
				assert!(
					h.release_url.as_str().ends_with("-release/"),
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
			assert!(btn.as_str().contains("/download/?key="), "{app}: {btn}");
			let final_url = parse_final_link(&read(&dir, "download-starting.html"))
				.unwrap_or_else(|e| panic!("{app}: parse_final_link: {e}"));
			assert!(
				final_url.as_str().contains("download.php")
					|| final_url.as_str().contains("downloadr"),
				"{app}: {final_url}"
			);
		}
	}

	#[test]
	fn derives_app_slug() {
		assert_eq!(
			app_slug(&"https://www.apkmirror.com/apk/mozilla/firefox/firefox-x-y-release/".into()),
			Some("mozilla/firefox")
		);
		assert_eq!(
			app_slug(&"https://www.apkmirror.com/apk/mozilla/".into()),
			None
		);
		assert_eq!(app_slug(&"https://elsewhere/apk/a/b/".into()), None);
	}

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
		// Asking for the channel lifts its penalty; `dev` in a package id is a
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
