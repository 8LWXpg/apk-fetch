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
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Every `tests/<app>/` directory holding a `search.html`. The dir name is the
    /// app; adding a dir (via `refresh-fixtures.sh <app> <pkg>`) extends coverage
    /// with no code change.
    fn app_dirs() -> Vec<(String, PathBuf)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
        let mut dirs: Vec<(String, PathBuf)> = fs::read_dir(&root)
            .expect("tests/ dir")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir() && p.join("search.html").is_file())
            .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), p))
            .collect();
        dirs.sort();
        assert!(!dirs.is_empty(), "no tests/<app>/ fixture dirs in {root:?}");
        dirs
    }

    fn read(dir: &Path, name: &str) -> String {
        fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("{}: {e}", dir.join(name).display()))
    }

    #[test]
    fn search_pages_parse() {
        for (app, dir) in app_dirs() {
            let html = read(&dir, "search.html");

            // A dir whose search page shows the "no results" marker (e.g.
            // `nonexistent/`) must parse as NotFound — not junk hits from the
            // "Popular / Latest Uploads" widgets further down the page.
            if html.contains("No results found matching your query") {
                assert!(
                    matches!(parse_search(&html), Err(ProviderError::NotFound)),
                    "{app}: no-results page didn't parse as NotFound"
                );
                continue;
            }

            let hits = parse_search(&html).unwrap_or_else(|e| panic!("{app}: {e}"));
            assert!(!hits.is_empty(), "{app}: empty hit list");
            for h in &hits {
                assert!(h.release_url.contains("/apk/"), "{app}: {}", h.release_url);
                assert!(h.release_url.ends_with("-release/"), "{app}: {}", h.release_url);
                assert!(!h.title.trim().is_empty(), "{app}: blank title");
            }
        }
    }

    #[test]
    fn download_chains_parse() {
        for (app, dir) in app_dirs() {
            if !dir.join("app.html").is_file() {
                continue; // search-only dir (e.g. `nonexistent/`)
            }

            let rows = parse_versions(&read(&dir, "app.html"))
                .unwrap_or_else(|e| panic!("{app}: parse_versions: {e}"));
            assert!(!rows.is_empty(), "{app}: no version rows");
            assert!(rows[0].version_page_url.contains("-release/"), "{app}");
            assert!(
                rows.iter().any(|r| {
                    let t = version_token(&r.title);
                    t.contains('.') && t.starts_with(|c: char| c.is_ascii_digit())
                }),
                "{app}: no release-number-shaped versions"
            );
            assert!(
                rows.iter().any(|r| r.uploaded.as_deref().is_some_and(|d| d.contains("UTC"))),
                "{app}: no upload dates parsed"
            );

            let variants = parse_variants(&read(&dir, "version.html"));
            assert!(!variants.is_empty(), "{app}: no variant rows");
            for v in &variants {
                assert!(v.download_page_url.contains("-download/"), "{app}: {}", v.download_page_url);
            }
            let picked = choose_variant(&variants, "arm64-v8a").expect("a variant");
            // If the app publishes any plain APK, arch selection must land on one.
            if variants.iter().any(|v| v.kind.eq_ignore_ascii_case("APK")) {
                assert!(picked.kind.eq_ignore_ascii_case("APK"), "{app}: picked {:?}", picked.kind);
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

    fn variant(kind: &str, arch: &str) -> Variant {
        Variant {
            version: "1.0".into(),
            arch: arch.into(),
            kind: kind.into(),
            download_page_url: "https://x/-download/".into(),
        }
    }

    #[test]
    fn choose_variant_prefers_apk_then_exact_arch() {
        let vs = vec![
            variant("BUNDLE", "arm64-v8a"),
            variant("APK", "armeabi-v7a"),
            variant("APK", "arm64-v8a"),
            variant("APK", "universal"),
        ];
        // exact-arch APK wins over an arch-matching bundle and other APKs
        assert_eq!(choose_variant(&vs, "arm64-v8a").unwrap().arch, "arm64-v8a");
        assert_eq!(choose_variant(&vs, "armeabi-v7a").unwrap().arch, "armeabi-v7a");
        // no exact match -> universal APK, never the bundle
        let picked = choose_variant(&vs, "x86").unwrap();
        assert_eq!(picked.kind, "APK");
        assert_eq!(picked.arch, "universal");

        // bundle-only app: arch match on the bundle beats an off-arch bundle
        let bundles = vec![variant("BUNDLE", "universal"), variant("BUNDLE", "arm64-v8a")];
        assert_eq!(choose_variant(&bundles, "arm64-v8a").unwrap().arch, "arm64-v8a");
    }

    #[test]
    fn derives_app_page() {
        assert_eq!(
            app_page_from_release("https://www.apkmirror.com/apk/mozilla/firefox/firefox-x-y-release/")
                .unwrap(),
            "https://www.apkmirror.com/apk/mozilla/firefox/"
        );
    }

    #[test]
    fn version_token_extracts_number() {
        assert_eq!(version_token("Firefox Fast & Private Browser 155.0.1"), "155.0.1");
    }
}
