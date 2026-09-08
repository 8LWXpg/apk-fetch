//! Pure HTML -> data parsers for APKMirror pages. No network here: every function
//! takes an HTML string so it can be unit-tested against saved fixtures.
//!
//! Selectors are `const &str` on purpose (spec): externalise to config only after
//! a real breakage proves it's needed.

use apk_fetch_core::ProviderError;
use scraper::{Html, Selector};

pub const BASE_URL: &str = "https://www.apkmirror.com";

// --- selectors ------------------------------------------------------------------
const SEARCH_RESULT_LINK: &str = "h5.appRowTitle a.fontBlack";
const LIST_WIDGET: &str = "div.listWidget";
const ALL_VERSIONS_ANCHOR: &str = r#"a[name="all_versions"]"#;
const VERSION_ROW: &str = "div.appRow";
const ROW_TITLE_LINK: &str = "h5.appRowTitle a.fontBlack";
const ROW_DATE: &str = "span.dateyear_utc";
const VARIANTS_TABLE_ROW: &str = "div.variants-table div.table-row";
const CELL: &str = "div.table-cell";
const VARIANT_LINK: &str = "a.accent_color";
const VARIANT_BADGE: &str = "span.apkm-badge";
const DOWNLOAD_BUTTON: &str = "a.downloadButton";
const FINAL_LINK: &str = "a#download-link";
const FINAL_LINK_FALLBACK: &str = "div.card-with-tabs a[href]";

fn sel(s: &str) -> Selector {
    Selector::parse(s).expect("static selector is valid")
}

fn parse_err(msg: impl Into<String>) -> ProviderError {
    ProviderError::ParseError(msg.into())
}

/// Absolute-ise an APKMirror href.
pub fn abs(href: &str) -> String {
    if href.starts_with("http") {
        href.to_string()
    } else if let Some(rest) = href.strip_prefix('/') {
        format!("{BASE_URL}/{rest}")
    } else {
        format!("{BASE_URL}/{href}")
    }
}

fn text_of(el: scraper::ElementRef<'_>) -> String {
    el.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

// --- search --------------------------------------------------------------------

pub struct SearchHit {
    pub title: String,
    /// Release-page URL (absolute).
    pub release_url: String,
}

/// Parse the `/?post_type=app_release&s=...` results page, in document order.
pub fn parse_search(html: &str) -> Result<Vec<SearchHit>, ProviderError> {
    // A no-results page still renders a "you might also like" grid of unrelated
    // apps, so an empty selector match isn't enough — check the marker.
    if html.contains("No results found matching your query") {
        return Err(ProviderError::NotFound);
    }
    // The results list ends where the "Popular / Latest Uploads" widgets begin;
    // those use `<h5 class="widgetHeader">` while the results header is a `<div>`.
    let html = match html.find(r#"<h5 class="widgetHeader">"#) {
        Some(cut) => &html[..cut],
        None => html,
    };
    let doc = Html::parse_document(html);
    let link = sel(SEARCH_RESULT_LINK);
    let hits: Vec<SearchHit> = doc
        .select(&link)
        .filter_map(|a| {
            let href = a.value().attr("href")?;
            Some(SearchHit {
                title: text_of(a),
                release_url: abs(href),
            })
        })
        .collect();
    if hits.is_empty() {
        return Err(ProviderError::NotFound);
    }
    Ok(hits)
}

/// `/apk/{org}/{repo}/{repo}-x-y-release/` -> `/apk/{org}/{repo}/` (absolute).
pub fn app_page_from_release(release_url: &str) -> Option<String> {
    let path = release_url.strip_prefix(BASE_URL)?;
    let mut segs = path.split('/').filter(|s| !s.is_empty());
    let apk = segs.next()?; // "apk"
    let org = segs.next()?;
    let repo = segs.next()?;
    if apk != "apk" {
        return None;
    }
    Some(format!("{BASE_URL}/{apk}/{org}/{repo}/"))
}

// --- versions -----------------------------------------------------------------

pub struct VersionRow {
    pub title: String,
    pub version_page_url: String,
    /// Raw `data-utcdate` string, e.g. `09/7/2026 02:34 UTC`.
    pub uploaded: Option<String>,
}

/// Parse the "All versions" widget on an app page.
pub fn parse_versions(html: &str) -> Result<Vec<VersionRow>, ProviderError> {
    let doc = Html::parse_document(html);
    let widget_sel = sel(LIST_WIDGET);
    let anchor_sel = sel(ALL_VERSIONS_ANCHOR);
    let row_sel = sel(VERSION_ROW);
    let title_sel = sel(ROW_TITLE_LINK);
    let date_sel = sel(ROW_DATE);

    // scraper has no :has(), so find the listWidget that contains the anchor.
    let widget = doc
        .select(&widget_sel)
        .find(|w| w.select(&anchor_sel).next().is_some())
        .ok_or_else(|| parse_err("no 'all versions' widget on app page"))?;

    let rows: Vec<VersionRow> = widget
        .select(&row_sel)
        .filter_map(|row| {
            let a = row.select(&title_sel).next()?;
            let href = a.value().attr("href")?;
            let uploaded = row
                .select(&date_sel)
                .next()
                .and_then(|d| d.value().attr("data-utcdate"))
                .map(|s| s.trim().to_string());
            Some(VersionRow {
                title: text_of(a),
                version_page_url: abs(href),
                uploaded,
            })
        })
        .collect();
    if rows.is_empty() {
        return Err(ProviderError::NotFound);
    }
    Ok(rows)
}

/// Best-effort: last whitespace token that looks like a version number.
pub fn version_token(title: &str) -> String {
    title
        .split_whitespace()
        .rev()
        .find(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .unwrap_or(title)
        .to_string()
}

// --- variants -----------------------------------------------------------------

pub struct Variant {
    pub version: String,
    pub arch: String,
    /// "APK" or "BUNDLE".
    pub kind: String,
    pub download_page_url: String,
}

/// Parse the `.variants-table` on a version page. Empty vec (not an error) if the
/// table is absent — caller may have landed straight on a download page.
pub fn parse_variants(html: &str) -> Vec<Variant> {
    let doc = Html::parse_document(html);
    let row_sel = sel(VARIANTS_TABLE_ROW);
    let cell_sel = sel(CELL);
    let link_sel = sel(VARIANT_LINK);
    let badge_sel = sel(VARIANT_BADGE);

    doc.select(&row_sel)
        .skip(1) // header row
        .filter_map(|row| {
            let cells: Vec<_> = row.select(&cell_sel).collect();
            let link = cells.first()?.select(&link_sel).next()?;
            let href = link.value().attr("href")?;
            let kind = cells
                .first()?
                .select(&badge_sel)
                .next()
                .map(text_of)
                .unwrap_or_default();
            Some(Variant {
                version: text_of(link),
                kind,
                arch: cells.get(1).map(|c| text_of(*c)).unwrap_or_default(),
                download_page_url: abs(href),
            })
        })
        .collect()
}

fn arch_tokens(v: &Variant) -> Vec<String> {
    v.arch
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

fn is_universal(v: &Variant) -> bool {
    let a = v.arch.to_ascii_lowercase();
    a.is_empty() || a.contains("universal") || a.contains("noarch") || a == "all"
}

/// Pick a variant for `arch` (e.g. `arm64-v8a`): exact-arch APK, else a universal
/// APK, else any APK, else an exact-arch bundle, else the first row.
// ponytail: no `--dpi`; add it here + in the trait if screen-density builds matter.
pub fn choose_variant<'a>(variants: &'a [Variant], arch: &str) -> Option<&'a Variant> {
    let want = arch.to_ascii_lowercase();
    let is_apk = |v: &&Variant| v.kind.eq_ignore_ascii_case("APK");
    let arch_match = |v: &&Variant| arch_tokens(v).contains(&want);
    variants
        .iter()
        .find(|v| is_apk(v) && arch_match(v))
        .or_else(|| variants.iter().find(|v| is_apk(v) && is_universal(v)))
        .or_else(|| variants.iter().find(is_apk))
        .or_else(|| variants.iter().find(arch_match))
        .or_else(|| variants.first())
}

// --- download chain ----------------------------------------------------------

/// Download page -> the keyed `a.downloadButton` href (absolute).
pub fn parse_download_button(html: &str) -> Result<String, ProviderError> {
    let doc = Html::parse_document(html);
    doc.select(&sel(DOWNLOAD_BUTTON))
        .find_map(|a| a.value().attr("href"))
        .map(abs)
        .ok_or_else(|| parse_err("no download button on download page"))
}

/// "Your download is starting..." page -> the actual APK URL (absolute).
pub fn parse_final_link(html: &str) -> Result<String, ProviderError> {
    let doc = Html::parse_document(html);
    doc.select(&sel(FINAL_LINK))
        .chain(doc.select(&sel(FINAL_LINK_FALLBACK)))
        .find_map(|a| a.value().attr("href"))
        .map(abs)
        .ok_or_else(|| parse_err("no final download link on 'starting' page"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = include_str!("../tests/fixtures/search-firefox.html");
    const SEARCH_NONE: &str = include_str!("../tests/fixtures/search-no-results.html");
    const APP: &str = include_str!("../tests/fixtures/app-firefox.html");
    const VERSION: &str = include_str!("../tests/fixtures/version-firefox.html");
    const DL_PAGE: &str = include_str!("../tests/fixtures/download-page-firefox.html");
    const STARTING: &str = include_str!("../tests/fixtures/download-starting-firefox.html");

    #[test]
    fn search_finds_firefox_release() {
        let hits = parse_search(SEARCH).unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].release_url.contains("/apk/mozilla/firefox"));
        assert!(hits[0].release_url.ends_with("-release/"));
        // "Popular / Latest Uploads" widgets must be excluded: every hit is firefox.
        assert!(
            hits.iter().all(|h| h.title.to_lowercase().contains("firefox")),
            "unrelated apps leaked into results: {:?}",
            hits.iter().map(|h| &h.title).collect::<Vec<_>>()
        );
    }

    #[test]
    fn no_results_page_is_not_found() {
        assert!(matches!(
            parse_search(SEARCH_NONE),
            Err(ProviderError::NotFound)
        ));
    }

    #[test]
    fn derives_app_page() {
        assert_eq!(
            app_page_from_release(
                "https://www.apkmirror.com/apk/mozilla/firefox/firefox-x-y-release/"
            )
            .unwrap(),
            "https://www.apkmirror.com/apk/mozilla/firefox/"
        );
    }

    #[test]
    fn parses_version_list() {
        let rows = parse_versions(APP).unwrap();
        assert!(!rows.is_empty());
        assert!(rows[0].version_page_url.contains("-release/"));
        assert!(rows.iter().any(|r| r.title.to_lowercase().contains("firefox")));
        // version tokens look like release numbers
        assert!(rows.iter().any(|r| {
            let t = version_token(&r.title);
            t.contains('.') && t.chars().next().is_some_and(|c| c.is_ascii_digit())
        }));
        assert!(
            rows[0].uploaded.as_deref().is_some_and(|d| d.contains("UTC")),
            "expected an upload date: {:?}",
            rows[0].uploaded
        );
    }

    #[test]
    fn version_token_extracts_number() {
        assert_eq!(version_token("Firefox Fast & Private Browser 155.0.1"), "155.0.1");
    }

    #[test]
    fn parses_variants_and_picks_by_arch() {
        let variants = parse_variants(VERSION);
        assert!(!variants.is_empty(), "expected variant rows");
        for v in &variants {
            assert!(v.download_page_url.contains("-download/"));
        }
        // Firefox 155 lists an arm64-v8a APK plus universal bundles/APK.
        let arm = choose_variant(&variants, "arm64-v8a").unwrap();
        assert!(arm.kind.eq_ignore_ascii_case("APK"));
        assert!(arm.arch.to_lowercase().contains("arm64-v8a"));

        // An arch with no exact match falls back to a universal APK, never a bundle.
        let x86 = choose_variant(&variants, "x86").unwrap();
        assert!(x86.kind.eq_ignore_ascii_case("APK"));
    }

    #[test]
    fn walks_download_chain() {
        let btn = parse_download_button(DL_PAGE).unwrap();
        assert!(btn.contains("/download/?key="));
        let final_url = parse_final_link(STARTING).unwrap();
        assert!(final_url.contains("download.php?id=") || final_url.contains("downloadr"));
    }
}
