//! Pure parsers for Uptodown. Search and version listing work over plain HTTP;
//! the actual APK download is gated behind Cloudflare Turnstile, so `download_url`
//! in `lib.rs` reports `Blocked` and the resolver fails over.

use apk_fetch_core::ProviderError;
use scraper::{Html, Selector};

pub const SEARCH_URL: &str = "https://en.uptodown.com/android/search";

fn sel(s: &str) -> Selector {
    Selector::parse(s).expect("static selector is valid")
}

fn text_of(el: scraper::ElementRef<'_>) -> String {
    el.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

pub struct SearchHit {
    /// e.g. `https://spotify.en.uptodown.com` (no path).
    pub base: String,
    /// Numeric app id used by the versions API.
    pub code: String,
    pub name: String,
}

pub fn parse_search(html: &str) -> Result<Vec<SearchHit>, ProviderError> {
    let doc = Html::parse_document(html);
    let item = sel("#content-list div.item[data-code]");
    let img = sel("img[alt]");
    let hits: Vec<SearchHit> = doc
        .select(&item)
        .filter_map(|el| {
            let code = el.value().attr("data-code")?.to_string();
            let onclick = el.value().attr("onclick")?;
            // location.href='https://<sub>.en.uptodown.com/android'
            let start = onclick.find("location.href='")? + "location.href='".len();
            let rest = &onclick[start..];
            let end = rest.find('\'')?;
            let base = rest[..end]
                .trim_end_matches("/android")
                .trim_end_matches('/')
                .to_string();
            if !base.contains("uptodown.com") {
                return None;
            }
            let name = el
                .select(&img)
                .next()
                .and_then(|i| i.value().attr("alt"))
                .map(|a| a.trim_end_matches(" icon").trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| text_of(el));
            Some(SearchHit { base, code, name })
        })
        .collect();
    if hits.is_empty() {
        return Err(ProviderError::NotFound);
    }
    Ok(hits)
}

/// The `Package Name` table row on an app page.
pub fn app_package(html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let rows = sel("tr");
    let th = sel("th");
    let td = sel("td");
    doc.select(&rows).find_map(|tr| {
        let is_pkg = tr.select(&th).any(|h| text_of(h).eq_ignore_ascii_case("Package Name"));
        if !is_pkg {
            return None;
        }
        // the row also holds an icon <td>; the package is the one that looks like one
        tr.select(&td).map(text_of).find(|s| s.contains('.') && !s.contains(' '))
    })
}

/// `data-code` anywhere on an app page (fallback when search didn't supply it).
pub fn app_code(html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    doc.select(&sel("[data-code]"))
        .find_map(|e| e.value().attr("data-code"))
        .filter(|c| c.chars().all(|ch| ch.is_ascii_digit()) && !c.is_empty())
        .map(str::to_string)
}

pub struct VersionRow {
    pub version: String,
    pub uploaded: Option<String>,
}

/// Parse `{base}/android/apps/{code}/versions/{n}` JSON.
pub fn parse_versions_json(json: &str) -> Result<Vec<VersionRow>, ProviderError> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| ProviderError::ParseError(format!("versions json: {e}")))?;
    let arr = v
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| ProviderError::ParseError("versions json: no data array".into()))?;
    let rows: Vec<VersionRow> = arr
        .iter()
        .filter_map(|row| {
            Some(VersionRow {
                version: row.get("version")?.as_str()?.to_string(),
                uploaded: row.get("lastUpdate").and_then(|u| u.as_str()).map(str::to_string),
            })
        })
        .collect();
    if rows.is_empty() {
        return Err(ProviderError::NotFound);
    }
    Ok(rows)
}

/// Candidate search terms for a package id: the non-generic segments, longest
/// first, plus their join. `com.spotify.music` -> ["spotify music", "spotify", "music"].
pub fn search_terms(pkg: &str) -> Vec<String> {
    const GENERIC: &[&str] = &[
        "com", "org", "net", "io", "app", "me", "co", "de", "tv", "fm", "android",
        "mobile", "apps", "free", "the",
    ];
    let mut segs: Vec<&str> = pkg
        .split('.')
        .filter(|s| !GENERIC.contains(&s.to_ascii_lowercase().as_str()) && s.len() > 1)
        .collect();
    let mut out = Vec::new();
    if segs.len() > 1 {
        out.push(segs.join(" "));
    }
    segs.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for s in segs {
        if !out.contains(&s.to_string()) {
            out.push(s.to_string());
        }
    }
    if out.is_empty() {
        out.push(pkg.to_string());
    }
    out
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
    fn search_versions_and_app_page_parse() {
        for (app, dir) in app_dirs() {
            let hits = parse_search(&read(&dir, "search.html"))
                .unwrap_or_else(|e| panic!("{app}: search: {e}"));
            for h in &hits {
                assert!(h.base.starts_with("https://") && h.base.ends_with("uptodown.com"));
                assert!(h.code.chars().all(|c| c.is_ascii_digit()) && !h.code.is_empty());
                assert!(!h.name.is_empty());
            }

            let pkg = app_package(&read(&dir, "app.html"))
                .unwrap_or_else(|| panic!("{app}: no Package Name row"));
            assert!(pkg.contains('.'), "{app}: {pkg}");
            assert!(app_code(&read(&dir, "app.html")).is_some(), "{app}: no data-code");

            let rows = parse_versions_json(&read(&dir, "versions.json"))
                .unwrap_or_else(|e| panic!("{app}: versions: {e}"));
            assert!(rows.len() > 2);
            assert!(rows.iter().all(|r| r.version.contains('.')));
        }
    }

    #[test]
    fn search_terms_from_pkg() {
        assert_eq!(search_terms("com.spotify.music"), vec!["spotify music", "spotify", "music"]);
        assert_eq!(search_terms("com.whatsapp"), vec!["whatsapp"]);
    }
}
