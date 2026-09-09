//! Pure HTML/-fragment parsers for APKCombo. Network-free for fixture tests.
//!
//! APKCombo download flow (no Cloudflare, no captcha on the download path):
//!   search              -> `/{slug}/{pkg}/`
//!   old-versions page   -> `/{slug}/{pkg}/old-versions`      (version list)
//!   download page       -> `/{slug}/{pkg}/download/phone-{v}-apk`  (carries `xid`)
//!   POST variant frag   -> `/{slug}/{pkg}/{xid}/dl`   (form: package_name, version)
//!   POST `/checkin`     -> `fp=...&ip=...` token
//!   final = `{BASE}{r2_href}&{checkin}&package_name={pkg}&lang=en`  -> 302 -> CDN

use provider::ProviderError;
use scraper::{Html, Selector};

pub const BASE_URL: &str = "https://apkcombo.com";
/// Fallback build tag if the download page's `var xid = "..."` can't be scraped.
pub const FALLBACK_XID: &str = "01a1200x20240308";

fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap_or_else(|e| panic!("invalid CSS selector {s:?}: {e}"))
}

fn text_of(el: scraper::ElementRef<'_>) -> String {
    el.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_err(m: impl Into<String>) -> ProviderError {
    ProviderError::ParseError(m.into())
}

pub fn abs(href: &str) -> String {
    if href.starts_with("http") {
        href.to_string()
    } else if let Some(r) = href.strip_prefix('/') {
        format!("{BASE_URL}/{r}")
    } else {
        format!("{BASE_URL}/{href}")
    }
}

/// Trailing release-number-ish token of a `.vername` like "Spotify 9.1.80.2221".
pub fn version_token(name: &str) -> String {
    name.split_whitespace()
        .rev()
        .find(|t| t.starts_with(|c: char| c.is_ascii_digit()))
        .unwrap_or(name)
        .to_string()
}

// --- search ------------------------------------------------------------------

pub struct SearchHit {
    pub package: String,
    pub title: String,
}

/// The `{slug}` URL segment for `pkg`, from any `href="/{slug}/{pkg}/"` link on
/// the bare app page `{BASE}/{pkg}/` (which self-links with the canonical slug).
pub fn slug_from_app_page(html: &str, pkg: &str) -> Option<String> {
    // Match `href="/{slug}/{pkg}/"`; skip the canonical `.../apkcombo.com/{pkg}/`
    // where the "segment" before {pkg} is the host.
    let needle = format!("/{pkg}/\"");
    let mut from = 0;
    while let Some(rel) = html[from..].find(&needle) {
        let at = from + rel;
        let slug = html[..at].rsplit('/').next().unwrap_or("");
        if !slug.is_empty()
            && slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && html[..at].ends_with(&format!("\"/{slug}"))
        {
            return Some(slug.to_string());
        }
        from = at + needle.len();
    }
    None
}

/// The package id a bare app page (`{BASE}/{pkg}/`) is for — its canonical URL's
/// last segment.
pub fn app_page_package(html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let href = doc
        .select(&sel(r#"link[rel="canonical"]"#))
        .find_map(|e| e.value().attr("href"))?;
    let seg = href.trim_end_matches('/').rsplit('/').next()?;
    seg.contains('.').then(|| seg.to_string())
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
        return Err(ProviderError::NotFound);
    }
    Ok(hits)
}

// --- versions --------------------------------------------------------------

pub struct VersionRow {
    pub name: String,
    pub uploaded: Option<String>,
    /// `/{slug}/{pkg}/download/phone-{version}-apk` (absolute).
    pub download_page_url: String,
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
            Some(VersionRow {
                name: a.select(&vername).next().map(text_of).unwrap_or_default(),
                uploaded: a
                    .select(&desc)
                    .next()
                    .map(text_of)
                    .and_then(|d| d.split('·').next().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty()),
                download_page_url: abs(href),
            })
        })
        .collect();
    if rows.is_empty() {
        return Err(ProviderError::NotFound);
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

// --- variants (POST /dl fragment) -----------------------------------------

pub struct Variant {
    pub version: String,
    /// "APK" or "XAPK".
    pub kind: String,
    pub arch: Vec<String>,
    /// `/r2?u=<encoded signed URL>` (absolute).
    pub r2_url: String,
}

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
        let arch: Vec<String> = g
            .select(&arch_sel)
            .next()
            .map(|c| {
                text_of(c)
                    .split(&[',', '+'][..])
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        for a in g.select(&item) {
            let Some(href) = a.value().attr("href") else { continue };
            if !seen.insert(href.to_string()) {
                continue;
            }
            variants.push(Variant {
                version: a.select(&vername).next().map(text_of).unwrap_or_default(),
                kind: a.select(&vtype).next().map(text_of).unwrap_or_default(),
                arch: arch.clone(),
                r2_url: abs(href),
            });
        }
    }
    if variants.is_empty() {
        return Err(parse_err("no variants in download fragment"));
    }
    Ok(variants)
}

/// Pick a variant. `bundle` selects XAPK, else APK. Then exact-arch, else the
/// widest arch list (universal-ish), else the first.
pub fn choose_variant<'a>(
    variants: &'a [Variant],
    arch: &str,
    bundle: bool,
) -> Option<&'a Variant> {
    let want_kind = if bundle { "xapk" } else { "apk" };
    let want = arch.to_ascii_lowercase();
    let kind_ok = |v: &&Variant| v.kind.eq_ignore_ascii_case(want_kind);
    let arch_ok = |v: &&Variant| v.arch.iter().any(|a| a.eq_ignore_ascii_case(&want));
    variants
        .iter()
        .find(|v| kind_ok(v) && arch_ok(v))
        .or_else(|| {
            variants
                .iter()
                .filter(kind_ok)
                .max_by_key(|v| v.arch.len())
        })
        .or_else(|| variants.iter().find(kind_ok))
        .or_else(|| variants.first())
}

/// `{r2_url}&{checkin}&package_name={pkg}&lang=en` — the JS `octs()` decoration.
pub fn final_download_url(r2_url: &str, checkin: &str, pkg: &str) -> String {
    format!("{r2_url}&{}&package_name={pkg}&lang=en", checkin.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn app_dirs() -> Vec<(String, PathBuf)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
        let mut d: Vec<(String, PathBuf)> = fs::read_dir(&root)
            .expect("tests/ dir")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir() && p.join("search.html").metadata().is_ok_and(|m| m.len() > 0))
            .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), p))
            .collect();
        d.sort();
        assert!(!d.is_empty(), "no tests/<app>/ dirs in {root:?}");
        d
    }
    fn read(d: &Path, n: &str) -> String {
        fs::read_to_string(d.join(n)).unwrap_or_else(|e| panic!("{}: {e}", d.join(n).display()))
    }

    #[test]
    fn search_and_versions_and_variants_parse() {
        for (app, dir) in app_dirs() {
            let hits = parse_search(&read(&dir, "search.html"))
                .unwrap_or_else(|e| panic!("{app}: search: {e}"));
            assert!(hits.iter().all(|h| h.package.contains('.') && !h.title.is_empty()));

            // the bare app page self-links with its {slug}
            let app_html = read(&dir, "app.html");
            let pkg = app_page_package(&app_html)
                .unwrap_or_else(|| panic!("{app}: no canonical package on app page"));
            let slug = slug_from_app_page(&app_html, &pkg)
                .unwrap_or_else(|| panic!("{app}: no slug link for {pkg}"));
            assert!(!slug.is_empty());

            let vers = parse_versions(&read(&dir, "old-versions.html"))
                .unwrap_or_else(|e| panic!("{app}: versions: {e}"));
            assert!(!vers.is_empty());
            assert!(vers[0].download_page_url.contains("/download/phone-"));
            assert!(vers.iter().any(|v| {
                let t = version_token(&v.name);
                t.contains('.') && t.starts_with(|c: char| c.is_ascii_digit())
            }));

            assert!(!extract_xid(&read(&dir, "download-page.html")).is_empty());

            let variants = parse_variants(&read(&dir, "variants.html"))
                .unwrap_or_else(|e| panic!("{app}: variants: {e}"));
            for v in &variants {
                assert!(v.r2_url.contains("/r2?u="), "{app}: {}", v.r2_url);
                assert!(v.kind.eq_ignore_ascii_case("APK") || v.kind.eq_ignore_ascii_case("XAPK"));
            }
            let apk = choose_variant(&variants, "arm64-v8a", false).unwrap();
            assert!(apk.kind.eq_ignore_ascii_case("APK"));
        }
    }

    #[test]
    fn slug_from_link() {
        let html = r#"<a href="/apkcombo-installer/com.apkcombo.app/">x</a><a href="/spotify/com.spotify.music/" title="Spotify APK">"#;
        assert_eq!(slug_from_app_page(html, "com.spotify.music").as_deref(), Some("spotify"));
        assert_eq!(slug_from_app_page(html, "com.nope.nope"), None);
    }

    #[test]
    fn xid_extraction() {
        assert_eq!(extract_xid(r#"...<script>var xid = "abc123def"</script>..."#), "abc123def");
        assert_eq!(extract_xid("no xid here"), FALLBACK_XID);
    }

    #[test]
    fn final_url_shape() {
        assert_eq!(
            final_download_url("https://apkcombo.com/r2?u=X", "fp=a&ip=b", "com.x.y"),
            "https://apkcombo.com/r2?u=X&fp=a&ip=b&package_name=com.x.y&lang=en"
        );
    }
}

