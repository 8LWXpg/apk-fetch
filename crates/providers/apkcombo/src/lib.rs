//! APKCombo provider. No Cloudflare / captcha on the download path. Resolves a
//! package id via APKCombo's own search to get the `{slug}` URL segment, then
//! walks the download flow documented in `parse`.

mod parse;

use apk_fetch_core::{AppResult, DownloadTarget, Provider, ProviderError, VersionInfo};
use apk_fetch_fetch::{Fetcher, HttpFetcher};
use async_trait::async_trait;

const NAME: &str = "apkcombo";

pub struct ApkCombo {
    fetcher: HttpFetcher,
}

impl ApkCombo {
    pub fn new() -> Self {
        Self { fetcher: HttpFetcher::new() }
    }

    fn search_url(query: &str) -> String {
        format!("{}/search?q={}", parse::BASE_URL, query.trim().replace(' ', "+"))
    }

    /// package id -> `{slug}` URL segment. The bare app page `{BASE}/{pkg}/`
    /// self-links with the canonical slug; APKCombo search is name-based and
    /// wouldn't take a package id.
    async fn slug_for(&self, pkg: &str) -> Result<String, ProviderError> {
        let html = self
            .fetcher
            .get_text(&format!("{}/{pkg}/", parse::BASE_URL))
            .await?;
        // A bare page for an unknown package doesn't carry it canonically.
        if parse::app_page_package(&html).as_deref() != Some(pkg) {
            return Err(ProviderError::NotFound);
        }
        parse::slug_from_app_page(&html, pkg).ok_or(ProviderError::NotFound)
    }
}

impl Default for ApkCombo {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for ApkCombo {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn search(&self, query: &str) -> Result<Vec<AppResult>, ProviderError> {
        let html = self.fetcher.get_text(&Self::search_url(query)).await?;
        Ok(parse::parse_search(&html)?
            .into_iter()
            .map(|h| AppResult {
                package: h.package,
                title: h.title,
                developer: None,
                provider: NAME.to_string(),
            })
            .collect())
    }

    async fn versions(&self, pkg: &str) -> Result<Vec<VersionInfo>, ProviderError> {
        let slug = self.slug_for(pkg).await?;
        let url = format!("{}/{slug}/{pkg}/old-versions", parse::BASE_URL);
        let html = self.fetcher.get_text(&url).await?;
        Ok(parse::parse_versions(&html)?
            .into_iter()
            .map(|r| VersionInfo {
                version: parse::version_token(&r.name),
                version_code: None,
                uploaded: r.uploaded,
                provider: NAME.to_string(),
            })
            .collect())
    }

    async fn download_url(
        &self,
        pkg: &str,
        version: Option<&str>,
        arch: &str,
    ) -> Result<DownloadTarget, ProviderError> {
        let slug = self.slug_for(pkg).await?;

        // Locate the download page (carries the `xid` build tag).
        let dl_page = match version {
            None => format!("{}/{slug}/{pkg}/download/phone-latest-apk", parse::BASE_URL),
            Some(want) => {
                let ov = self
                    .fetcher
                    .get_text(&format!("{}/{slug}/{pkg}/old-versions", parse::BASE_URL))
                    .await?;
                parse::parse_versions(&ov)?
                    .into_iter()
                    .find(|r| parse::version_token(&r.name) == want || r.name.contains(want))
                    .map(|r| r.download_page_url)
                    .ok_or(ProviderError::NotFound)?
            }
        };
        let xid = parse::extract_xid(&self.fetcher.get_text(&dl_page).await?);

        // POST the variant fragment.
        let frag = self
            .fetcher
            .post_form(
                &format!("{}/{slug}/{pkg}/{xid}/dl", parse::BASE_URL),
                &[("package_name", pkg), ("version", version.unwrap_or(""))],
            )
            .await?;
        let variants = parse::parse_variants(&frag)?;
        let variant = parse::choose_variant(&variants, arch, false)
            .ok_or(ProviderError::NotFound)?;

        // Checkin token, then decorate the r2 link.
        let checkin = self
            .fetcher
            .post_form(&format!("{}/checkin", parse::BASE_URL), &[])
            .await?;
        let url = parse::final_download_url(&variant.r2_url, &checkin, pkg);

        let resolved_version = if variant.version.is_empty() {
            version.unwrap_or("latest").to_string()
        } else {
            parse::version_token(&variant.version)
        };
        let ext = if variant.kind.eq_ignore_ascii_case("XAPK") { "xapk" } else { "apk" };
        let variant_arch = variant.arch.first().cloned();
        Ok(DownloadTarget {
            filename: apk_fetch_core::download_filename(
                pkg,
                &resolved_version,
                variant_arch.as_deref(),
                ext,
            ),
            url,
            version: Some(resolved_version),
            arch: variant_arch,
            provider: NAME.to_string(),
            headers: Vec::new(),
        })
    }
}
