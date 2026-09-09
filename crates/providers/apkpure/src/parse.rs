//! Pure HTML -> data parsers for APKPure pages. Network-free so they can be
//! unit-tested against saved fixtures.

use contract::ProviderError;
use scraper::{Html, Selector};

pub const BASE_URL: &str = "https://apkpure.com";
/// Direct-download host: `{DL_URL}/b/{APK|XAPK}/{pkg}?version={v}` 302s to the file.
pub const DL_URL: &str = "https://d.apkpure.com";

fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap_or_else(|e| panic!("invalid CSS selector {s:?}: {e}"))
}

fn text_of(el: scraper::ElementRef<'_>) -> String {
    el.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// apkpure sometimes formats a version as `2.6.2(20653)` — the trailing
/// `(versionCode)` is noise for our purposes.
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
    let ok = dots >= 2
        && seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_'));
    ok.then(|| seg.to_string())
}

// --- search -------------------------------------------------------------------

pub struct SearchHit {
    pub package: String,
    pub title: String,
    pub developer: Option<String>,
}

/// Parse `/search?q=...`. Covers both result shapes: the top "brand" match
/// (`div.sa-apps-div`) and the `a.dd` list rows.
pub fn parse_search(html: &str) -> Result<Vec<SearchHit>, ProviderError> {
    let doc = Html::parse_document(html);
    let anchor = sel("a[href*=\"apkpure.com/\"]");
    let p1 = sel("p.p1");
    let p2 = sel("p.p2");

    let mut seen = std::collections::HashSet::new();
    let mut hits = Vec::new();
    for a in doc.select(&anchor) {
        let Some(href) = a.value().attr("href") else { continue };
        let Some(package) = package_from_href(href) else { continue };
        let Some(title_el) = a.select(&p1).next() else { continue };
        let title = text_of(title_el);
        if title.is_empty() || !seen.insert(package.clone()) {
            continue;
        }
        hits.push(SearchHit {
            package,
            title,
            developer: a.select(&p2).next().map(text_of).filter(|s| !s.is_empty()),
        });
    }
    if hits.is_empty() {
        return Err(ProviderError::NotFound);
    }
    Ok(hits)
}

// --- versions ----------------------------------------------------------------

pub struct VersionRow {
    pub version: String,
    pub version_code: Option<String>,
}

/// Parse `/<slug>/<pkg>/versions` — rows carry everything in `data-dt-*` attrs.
pub fn parse_versions(html: &str) -> Result<Vec<VersionRow>, ProviderError> {
    let doc = Html::parse_document(html);
    let row = sel("div.ver_download_link[data-dt-version]");
    let mut seen = std::collections::HashSet::new();
    let rows: Vec<VersionRow> = doc
        .select(&row)
        .filter_map(|el| {
            let version = clean_version(el.value().attr("data-dt-version")?);
            if version.is_empty() || !seen.insert(version.clone()) {
                return None;
            }
            Some(VersionRow {
                version,
                version_code: el
                    .value()
                    .attr("data-dt-versioncode")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            })
        })
        .collect();
    if rows.is_empty() {
        return Err(ProviderError::NotFound);
    }
    Ok(rows)
}

/// The latest version string from an app page — the main download button, else
/// any `data-dt-version`. `None` if the page has no such marker (treat as
/// not-found). May include a `(code)` suffix; that's apkpure's own formatting.
pub fn latest_version(html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let main = sel(".dt-main-download-btn[data-dt-version]");
    let any = sel("[data-dt-version]");
    doc.select(&main)
        .chain(doc.select(&any))
        .find_map(|el| el.value().attr("data-dt-version"))
        .map(clean_version)
        .filter(|s| !s.is_empty())
}

fn pct(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '-' | '_' | '~' => c.to_string(),
            _ => format!("%{:02X}", c as u32),
        })
        .collect()
}

/// `{DL_URL}/b/APK/{pkg}?version={version}`. Pass `"latest"` when unknown.
pub fn download_url(pkg: &str, version: &str) -> String {
    format!("{DL_URL}/b/APK/{pkg}?version={}", pct(version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Every `tests/<app>/` directory with a `search.html`. Adding a dir (via
    /// `refresh-fixtures.sh <app> <pkg>`) extends coverage with no code change.
    fn app_dirs() -> Vec<(String, PathBuf)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
        let mut dirs: Vec<(String, PathBuf)> = fs::read_dir(&root)
            .expect("tests/ dir")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir() && p.join("search.html").metadata().is_ok_and(|m| m.len() > 0))
            .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), p))
            .collect();
        dirs.sort();
        assert!(!dirs.is_empty(), "no tests/<app>/ fixture dirs in {root:?}");
        dirs
    }

    fn read(dir: &Path, name: &str) -> String {
        fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("{}: {e}", dir.join(name).display()))
    }

    /// The package id an app page is for — the last segment of its canonical URL.
    fn app_package(app_html: &str) -> Option<String> {
        let doc = Html::parse_document(app_html);
        let href = doc
            .select(&sel(r#"link[rel="canonical"]"#))
            .find_map(|el| el.value().attr("href"))?;
        let seg = href.trim_end_matches('/').rsplit('/').next()?;
        seg.contains('.').then(|| seg.to_string())
    }

    #[test]
    fn search_pages_parse() {
        for (app, dir) in app_dirs() {
            let hits = parse_search(&read(&dir, "search.html"))
                .unwrap_or_else(|e| panic!("{app}: parse_search: {e}"));
            assert!(!hits.is_empty(), "{app}: empty hit list");
            for h in &hits {
                assert!(h.package.contains('.'), "{app}: bad package {:?}", h.package);
                assert!(!h.title.trim().is_empty(), "{app}: blank title");
            }
            // no duplicate packages
            let mut pkgs: Vec<_> = hits.iter().map(|h| &h.package).collect();
            let n = pkgs.len();
            pkgs.sort();
            pkgs.dedup();
            assert_eq!(pkgs.len(), n, "{app}: duplicate packages in results");

            // the app this dir is named for shows up in its own search results
            if let Some(pkg) = app_package(&read(&dir, "app.html")) {
                assert!(hits.iter().any(|h| h.package == pkg), "{app}: {pkg} missing from results");
            }
        }
    }

    #[test]
    fn app_and_versions_pages_parse() {
        for (app, dir) in app_dirs() {
            let v = latest_version(&read(&dir, "app.html"))
                .unwrap_or_else(|| panic!("{app}: no version marker on app page"));
            assert!(!v.contains('('), "{app}: (code) suffix not stripped: {v}");
            assert!(
                v.contains('.') && v.starts_with(|c: char| c.is_ascii_digit()),
                "{app}: odd version {v}"
            );

            let rows = parse_versions(&read(&dir, "versions.html"))
                .unwrap_or_else(|e| panic!("{app}: parse_versions: {e}"));
            assert!(rows.len() > 3, "{app}: only {} versions", rows.len());
            assert!(rows.iter().all(|r| r.version_code.is_some()), "{app}: a row has no version code");
            assert!(
                rows.iter().all(|r| !r.version.contains('(') && r.version.contains('.')),
                "{app}: a version string is malformed"
            );
        }
    }

    #[test]
    fn download_url_shape() {
        assert_eq!(
            download_url("org.mozilla.firefox", "155.0.1"),
            "https://d.apkpure.com/b/APK/org.mozilla.firefox?version=155.0.1"
        );
    }
}
