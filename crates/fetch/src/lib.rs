//! HTTP via the system `curl`. We shell out rather than use a Rust HTTP client
//! because mirror sites (APKMirror) sit behind Cloudflare rules that challenge
//! `reqwest`'s TLS fingerprint on content paths — `curl` (schannel / system
//! OpenSSL) passes with a browser UA where `rustls` and `native-tls` both get a
//! managed challenge. `curl` ships with Windows 10+, macOS, and virtually every
//! Linux.
//!
//! Responsibilities: per-provider throttle, transient-error retry, and mapping
//! Cloudflare/anti-bot responses to `ProviderError::Blocked`.

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use contract::ProviderError;
use async_trait::async_trait;
use tokio::process::Command;
use tokio::sync::Mutex;

/// Hardcoded knobs (spec: constants live here, not in config).
/// Whole-request cap for page fetches. Downloads only get `CONNECT_TIMEOUT_SECS`
/// (a big APK legitimately takes minutes).
pub const REQUEST_TIMEOUT_SECS: u64 = 30;
pub const CONNECT_TIMEOUT_SECS: u64 = 20;
/// Minimum gap between requests from one provider's fetcher.
pub const THROTTLE_DELAY: Duration = Duration::from_millis(1200);
pub const MAX_RETRIES: u32 = 3;
pub const RETRY_BASE_BACKOFF: Duration = Duration::from_millis(500);
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
    (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// Cloudflare / anti-bot challenge fingerprints in a response body.
const BLOCK_SIGNATURES: &[&str] = &[
    "Just a moment...",
    "cf-browser-verification",
    "cf_chl_opt",
    "Attention Required! | Cloudflare",
    "Checking if the site connection is secure",
    "_cf_chl_",
    "Enable JavaScript and cookies to continue",
];

fn network_err(e: impl std::fmt::Display) -> ProviderError {
    ProviderError::Network(std::io::Error::other(e.to_string()))
}

fn looks_blocked(body: &str) -> bool {
    BLOCK_SIGNATURES.iter().any(|sig| body.contains(sig))
}

/// First bytes of a local ZIP (APK/XAPK) — `PK\x03\x04`, or the empty-archive
/// `PK\x05\x06`. An empty or missing file fails the check.
/// APK-family extension implied by a content-type or final URL, if any.
fn package_ext_from(content_type: &str, url: &str) -> Option<&'static str> {
    let ct = content_type.to_ascii_lowercase();
    if ct.contains("xapk") {
        return Some("xapk");
    }
    if ct.contains("vnd.android.package-archive") {
        return Some("apk");
    }
    let hay = url.to_ascii_lowercase();
    ["xapk", "apkm", "apks", "apk"]
        .into_iter()
        .find(|ext| hay.contains(&format!(".{ext}?")) || hay.ends_with(&format!(".{ext}")))
}

/// If the server says the payload is a different package type than `dest`'s
/// extension, rename it. Returns the path actually on disk.
async fn rename_to_real_ext(
    dest: &Path,
    content_type: &str,
    final_url: &str,
) -> Result<std::path::PathBuf, ProviderError> {
    let Some(real) = package_ext_from(content_type, final_url) else {
        return Ok(dest.to_path_buf());
    };
    if dest.extension().and_then(|e| e.to_str()) == Some(real) {
        return Ok(dest.to_path_buf());
    }
    let target = dest.with_extension(real);
    tokio::fs::rename(dest, &target).await.map_err(network_err)?;
    Ok(target)
}

async fn starts_with_zip_magic(path: &std::path::Path) -> bool {
    use tokio::io::AsyncReadExt;
    let Ok(mut f) = tokio::fs::File::open(path).await else {
        return false;
    };
    let mut head = [0u8; 4];
    match f.read_exact(&mut head).await {
        Ok(_) => head == *b"PK\x03\x04" || head == *b"PK\x05\x06",
        Err(_) => false,
    }
}

/// curl exit codes worth retrying (connect / resolve / timeout / recv).
fn curl_exit_is_transient(code: Option<i32>) -> bool {
    matches!(code, Some(6 | 7 | 28 | 35 | 52 | 55 | 56))
}

fn map_http_status(code: u16, url: &str) -> Option<ProviderError> {
    match code {
        404 | 410 => Some(ProviderError::NotFound(format!("got {code} for {url}"))),
        403 | 429 => Some(ProviderError::Blocked { retry_after: None }),
        s if s >= 500 => Some(network_err(format!("upstream returned {s}"))),
        _ => None,
    }
}

#[async_trait]
pub trait Fetcher: Send + Sync {
    /// GET a URL and return the body as text (throttle + retry + block detection).
    async fn get_text(&self, url: &str) -> Result<String, ProviderError>;
    /// GET a URL and return the raw body bytes.
    async fn get_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError>;
}

/// A curl-backed fetcher with a per-instance throttle. Construct one per provider
/// so each site gets its own request cadence.
pub struct HttpFetcher {
    min_gap: Duration,
    last_request: Mutex<Option<Instant>>,
}

impl HttpFetcher {
    pub fn new() -> Self {
        Self::with_delay(THROTTLE_DELAY)
    }

    pub fn with_delay(min_gap: Duration) -> Self {
        Self {
            min_gap,
            last_request: Mutex::new(None),
        }
    }

    /// Sleep so at least `min_gap` has elapsed since the previous request.
    async fn throttle(&self) {
        let mut last = self.last_request.lock().await;
        if let Some(prev) = *last {
            let elapsed = prev.elapsed();
            if elapsed < self.min_gap {
                tokio::time::sleep(self.min_gap - elapsed).await;
            }
        }
        *last = Some(Instant::now());
    }

    fn base_cmd(url: &str, headers: &[(String, String)]) -> Command {
        let mut cmd = Command::new("curl");
        cmd.args([
            "-sS",
            "-L",
            "--compressed",
            "--connect-timeout",
            &CONNECT_TIMEOUT_SECS.to_string(),
            "-A",
            USER_AGENT,
        ]);
        // Just the UA: mirror-site WAFs (APKPure's especially) flag a lone
        // `Accept-Language` on top of curl's fingerprint, and neither site needs
        // more than the UA to serve pages.
        for (k, v) in headers {
            cmd.arg("-H").arg(format!("{k}: {v}"));
        }
        cmd.arg(url);
        cmd
    }

    async fn get(&self, url: &str, headers: &[(String, String)]) -> Result<String, ProviderError> {
        self.request(url, headers, &[]).await
    }

    /// POST `url` with a `multipart/form-data` body (curl `-F`). An empty `form`
    /// still issues a POST (some endpoints want a bodyless POST).
    pub async fn post_form(
        &self,
        url: &str,
        form: &[(&str, &str)],
    ) -> Result<String, ProviderError> {
        let mut extra: Vec<String> = vec!["-X".into(), "POST".into()];
        for (k, v) in form {
            extra.push("-F".into());
            extra.push(format!("{k}={v}"));
        }
        let extra_ref: Vec<&str> = extra.iter().map(String::as_str).collect();
        self.request(url, &[], &extra_ref).await
    }

    /// One request with retry-on-transient. `Blocked` / `NotFound` never retry.
    async fn request(
        &self,
        url: &str,
        headers: &[(String, String)],
        extra_args: &[&str],
    ) -> Result<String, ProviderError> {
        let mut attempt = 0;
        loop {
            self.throttle().await;

            let output = Self::base_cmd(url, headers)
                .args(["--max-time", &REQUEST_TIMEOUT_SECS.to_string()])
                .args(extra_args)
                .arg("-w")
                .arg("\n%{http_code}")
                .output()
                .await
                .map_err(|e| network_err(format!("could not run `curl` (is it installed and on PATH?): {e}")))?;

            if !output.status.success() {
                let code = output.status.code();
                if curl_exit_is_transient(code) && attempt < MAX_RETRIES {
                    attempt += 1;
                    tokio::time::sleep(RETRY_BASE_BACKOFF * attempt).await;
                    continue;
                }
                return Err(network_err(format!(
                    "curl exited {code:?}: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let (body, status_line) = stdout.rsplit_once('\n').unwrap_or((&stdout, "0"));
            let status: u16 = status_line.trim().parse().unwrap_or(0);

            if let Some(err) = map_http_status(status, url) {
                if matches!(err, ProviderError::Network(_)) && attempt < MAX_RETRIES {
                    attempt += 1;
                    tokio::time::sleep(RETRY_BASE_BACKOFF * attempt).await;
                    continue;
                }
                return Err(err);
            }
            if looks_blocked(body) {
                return Err(ProviderError::Blocked { retry_after: None });
            }
            return Ok(body.to_string());
        }
    }

    /// Stream a URL to `dest`, calling `progress(downloaded, total)` while it runs.
    /// `total` is `None` — curl owns the transfer, we just poll the partial file.
    ///
    /// Returns the path actually written: if the server's content-type / final URL
    /// says the payload is a different package type than `dest`'s extension (e.g.
    /// an `.xapk` when we guessed `.apk`), the file is renamed to match.
    pub async fn download_to_file<F>(
        &self,
        url: &str,
        headers: &[(String, String)],
        dest: &Path,
        mut progress: F,
    ) -> Result<std::path::PathBuf, ProviderError>
    where
        F: FnMut(u64, Option<u64>) + Send,
    {
        self.throttle().await;

        let mut child = Self::base_cmd(url, headers)
            .args(["--fail", "--retry", &MAX_RETRIES.to_string()])
            .arg("-o")
            .arg(dest)
            .arg("-w")
            .arg("%{content_type}\t%{url_effective}")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| network_err(format!("could not run `curl` (is it installed and on PATH?): {e}")))?;

        loop {
            tokio::select! {
                status = child.wait() => {
                    let status = status.map_err(network_err)?;
                    if let Ok(m) = tokio::fs::metadata(dest).await {
                        progress(m.len(), None);
                    }
                    if !status.success() {
                        let mut err = String::new();
                        if let Some(mut s) = child.stderr.take() {
                            use tokio::io::AsyncReadExt;
                            let _ = s.read_to_string(&mut err).await;
                        }
                        let _ = tokio::fs::remove_file(dest).await;
                        return Err(network_err(format!(
                            "curl download failed ({status}): {}",
                            err.trim()
                        )));
                    }
                    // APK/XAPK are ZIP: a non-`PK` payload is an error/landing page
                    // that came back 200 (some mirror download endpoints do this).
                    if !starts_with_zip_magic(dest).await {
                        let _ = tokio::fs::remove_file(dest).await;
                        return Err(network_err(
                            "downloaded file is not an APK (server returned a non-package response)",
                        ));
                    }
                    let mut w = String::new();
                    if let Some(mut s) = child.stdout.take() {
                        use tokio::io::AsyncReadExt;
                        let _ = s.read_to_string(&mut w).await;
                    }
                    let (ct, final_url) = w.split_once('\t').unwrap_or((w.as_str(), ""));
                    return rename_to_real_ext(dest, ct, final_url).await;
                }
                _ = tokio::time::sleep(Duration::from_millis(200)) => {
                    if let Ok(m) = tokio::fs::metadata(dest).await {
                        progress(m.len(), None);
                    }
                }
            }
        }
    }
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Fetcher for HttpFetcher {
    async fn get_text(&self, url: &str) -> Result<String, ProviderError> {
        self.get(url, &[]).await
    }

    async fn get_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        self.get(url, &[]).await.map(String::into_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_cloudflare_challenge() {
        assert!(looks_blocked(
            "<html><head><title>Just a moment...</title></head></html>"
        ));
        assert!(looks_blocked("window._cf_chl_opt={};"));
        assert!(!looks_blocked("<html><body>normal apkmirror page</body></html>"));
    }

    #[test]
    fn ext_from_content_type_and_url() {
        assert_eq!(
            package_ext_from("application/vnd.android.package-archive", ""),
            Some("apk")
        );
        assert_eq!(package_ext_from("application/xapk-package-archive", ""), Some("xapk"));
        // content-type unhelpful -> fall back to the URL
        assert_eq!(
            package_ext_from("application/octet-stream", "https://x/y_APKPure.xapk?k=1"),
            Some("xapk")
        );
        assert_eq!(package_ext_from("text/html", "https://x/y"), None);
    }

    #[test]
    fn status_mapping() {
        let u = "https://x/y";
        assert!(matches!(
            map_http_status(403, u),
            Some(ProviderError::Blocked { .. })
        ));
        assert!(matches!(
            map_http_status(429, u),
            Some(ProviderError::Blocked { .. })
        ));
        assert!(matches!(
            map_http_status(404, u),
            Some(ProviderError::NotFound(_))
        ));
        assert!(map_http_status(200, u).is_none());
    }
}
