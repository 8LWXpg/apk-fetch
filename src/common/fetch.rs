//! HTTP via the system `curl`, not a Rust client: Cloudflare challenges
//! `reqwest`'s TLS fingerprint on APKMirror content paths, where `curl` with a
//! browser UA passes.

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::common::contract::ProviderError;
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

/// Marks curl's `-w` line in stdout, past the header dump `-D -` writes there.
const META_SENTINEL: &str = "\u{1f}apk-fetch\u{1f}";

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

/// Extension from the last `Content-Disposition` filename in a header dump — the
/// name a browser would save this download under. A server (or a hijacked mirror)
/// chooses that string, so only a short alphanumeric suffix is accepted.
fn ext_from_disposition(headers: &str) -> Option<String> {
	let ext = headers
		.lines()
		.filter(|l| {
			l.get(..20)
				.is_some_and(|p| p.eq_ignore_ascii_case("content-disposition:"))
		})
		.filter_map(|l| {
			// `filename="x.apk"`, or RFC 5987 `filename*=UTF-8\x.apk`.
			let v = l.split_once("filename")?.1.trim_start_matches(['*', '=']);
			let v = v.rsplit("''").next()?.trim().trim_matches('"');
			v.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())
		})
		.next_back()?;
	(ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric())).then_some(ext)
}

/// The other two things the site tells us, in descending trustworthiness.
fn ext_from_type_or_url(content_type: &str, url: &str) -> Option<String> {
	let ct = content_type.to_ascii_lowercase();
	if ct.contains("xapk") {
		return Some("xapk".to_string());
	}
	if ct.contains("vnd.android.package-archive") {
		return Some("apk".to_string());
	}
	let hay = url.to_ascii_lowercase();
	["xapk", "apkm", "apks", "apk"]
		.into_iter()
		.find(|ext| hay.contains(&format!(".{ext}?")) || hay.ends_with(&format!(".{ext}")))
		.map(str::to_string)
}

/// Appends the extension the site served — `Content-Disposition` first, then
/// content-type, then the final URL. `dest` arrives without one, and the stem is
/// full of dots (version numbers), so this appends instead of replacing. Nothing
/// recognisable leaves the bare name. Returns the path on disk.
async fn add_served_ext(
	dest: &Path,
	headers: &str,
	content_type: &str,
	final_url: &str,
) -> Result<std::path::PathBuf, ProviderError> {
	let Some(ext) =
		ext_from_disposition(headers).or_else(|| ext_from_type_or_url(content_type, final_url))
	else {
		return Ok(dest.to_path_buf());
	};
	let target = std::path::PathBuf::from(format!("{}.{ext}", dest.display()));
	tokio::fs::rename(dest, &target)
		.await
		.map_err(network_err)?;
	Ok(target)
}

/// First bytes of a local ZIP (APK/XAPK) — `PK\x03\x04`, or the empty-archive
/// `PK\x05\x06`. An empty or missing file fails the check.
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

/// curl exit codes worth retrying: resolve (6), connect (7), timeout (28),
/// SSL connect (35), empty reply (52), send (55), recv (56).
fn curl_exit_is_transient(code: Option<i32>) -> bool {
	matches!(code, Some(6 | 7 | 28 | 35 | 52 | 55 | 56))
}

fn map_http_status(code: u16, url: &str) -> Option<ProviderError> {
	match code {
		404 | 410 => Some(ProviderError::NotFound(format!("got {code} for {url}"))),
		403 | 429 => Some(ProviderError::Blocked),
		s if s >= 500 => Some(network_err(format!("upstream returned {s}"))),
		_ => None,
	}
}

#[async_trait]
pub trait Fetcher: Send + Sync {
	async fn get_text(&self, url: &str) -> Result<String, ProviderError>;
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

	/// `progress`: show curl's own meter on stderr (downloads). `-s` suppresses
	/// the meter, so showing it means dropping to a bare `-S`.
	fn base_cmd(url: &str, headers: &[(String, String)], progress: bool) -> Command {
		let mut cmd = Command::new("curl");
		cmd.arg(if progress { "-S" } else { "-sS" });
		cmd.args([
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
		// Nothing else ties curl's life to ours: an orphan keeps downloading after
		// we're gone. Covers every path whose future is dropped mid-flight.
		cmd.kill_on_drop(true);
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

	async fn request(
		&self,
		url: &str,
		headers: &[(String, String)],
		extra_args: &[&str],
	) -> Result<String, ProviderError> {
		let mut attempt = 0;
		loop {
			self.throttle().await;

			let output = Self::base_cmd(url, headers, false)
				.args(["--max-time", &REQUEST_TIMEOUT_SECS.to_string()])
				.args(extra_args)
				.arg("-w")
				.arg("\n%{http_code}")
				.output()
				.await
				.map_err(|e| {
					network_err(format!(
						"could not run `curl` (is it installed and on PATH?): {e}"
					))
				})?;

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
				return Err(ProviderError::Blocked);
			}
			return Ok(body.to_string());
		}
	}

	/// The URL a `HEAD` lands on after redirects, body never downloaded. Lets a
	/// provider read something the server encodes in a redirect instead of
	/// scraping it back out of the page (APKCombo's `/en/{pkg}/` -> canonical
	/// `/{slug}/{pkg}/`). No redirect means the URL is returned unchanged.
	pub async fn resolve_url(&self, url: &str) -> Result<String, ProviderError> {
		self.throttle().await;

		let output = Self::base_cmd(url, &[], false)
			.args(["-I", "--max-time", &REQUEST_TIMEOUT_SECS.to_string()])
			.arg("-w")
			.arg("\n%{url_effective}")
			.output()
			.await
			.map_err(|e| {
				network_err(format!(
					"could not run `curl` (is it installed and on PATH?): {e}"
				))
			})?;

		if !output.status.success() {
			return Err(network_err(format!(
				"curl exited {:?}: {}",
				output.status.code(),
				String::from_utf8_lossy(&output.stderr).trim()
			)));
		}
		let stdout = String::from_utf8_lossy(&output.stdout);
		Ok(stdout
			.rsplit('\n')
			.next()
			.unwrap_or_default()
			.trim()
			.to_string())
	}

	/// Stream a URL to `dest`. curl draws its own progress bar on the inherited
	/// stderr (it knows the content length; we did not).
	///
	/// Returns the path actually written: if the server's content-type / final URL
	/// says the payload is a different package type than `dest`'s extension (e.g.
	/// an `.xapk` when we guessed `.apk`), the file is renamed to match.
	pub async fn download_to_file(
		&self,
		url: &str,
		headers: &[(String, String)],
		dest: &Path,
	) -> Result<std::path::PathBuf, ProviderError> {
		self.throttle().await;

		// `.output()` would force stderr to a pipe and hide curl's progress bar;
		// spawn instead so stderr stays on the inherited terminal.
		let mut child = Self::base_cmd(url, headers, true)
			.args(["--fail", "--retry", &MAX_RETRIES.to_string()])
			.arg("-o")
			.arg(dest)
			.arg("-D")
			.arg("-")
			.arg("-w")
			.arg(format!(
				"\n{META_SENTINEL}%{{content_type}}\t%{{url_effective}}"
			))
			.stdout(Stdio::piped())
			.stderr(Stdio::inherit())
			.spawn()
			.map_err(|e| {
				network_err(format!(
					"could not run `curl` (is it installed and on PATH?): {e}"
				))
			})?;

		// Downloads are the long-lived curl, so Ctrl+C lands here. Handling the
		// signal replaces the default handler, which aborts the process outright —
		// no unwinding, no `kill_on_drop`, so curl would keep downloading orphaned.
		//
		// Wait before reading stdout, not after: `read_to_string` only returns once
		// curl closes the pipe, so reading first parks us here for the whole
		// download with no `ctrl_c` branch armed. `wait()` leaves stdout open, and
		// the `-w` line is far too small to fill the pipe buffer.
		let status = tokio::select! {
			status = child.wait() => status.map_err(network_err)?,
			_ = tokio::signal::ctrl_c() => {
				let _ = child.kill().await;
				let _ = tokio::fs::remove_file(dest).await;
				eprintln!();
				return Err(ProviderError::Cancelled);
			}
		};

		let mut stdout_buf = String::new();
		if let Some(mut s) = child.stdout.take() {
			use tokio::io::AsyncReadExt;
			let _ = s.read_to_string(&mut stdout_buf).await;
		}

		if !status.success() {
			let _ = tokio::fs::remove_file(dest).await;
			// curl already printed the reason to the inherited stderr.
			return Err(network_err(format!("curl download failed ({status})")));
		}
		// APK/XAPK are ZIP: a non-`PK` payload is an error/landing page
		// that came back 200 (some mirror download endpoints do this).
		if !starts_with_zip_magic(dest).await {
			let _ = tokio::fs::remove_file(dest).await;
			return Err(network_err(
				"downloaded file is not an APK (server returned a non-package response)",
			));
		}
		// stdout holds every redirect's headers, then the `-w` line last.
		let (headers, meta) = stdout_buf
			.rsplit_once(META_SENTINEL)
			.unwrap_or((stdout_buf.as_str(), ""));
		let (ct, final_url) = meta.split_once('\t').unwrap_or((meta, ""));
		add_served_ext(dest, headers, ct, final_url).await
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
		assert!(!looks_blocked(
			"<html><body>normal apkmirror page</body></html>"
		));
	}

	#[test]
	fn status_mapping() {
		let u = "https://x/y";
		assert!(matches!(
			map_http_status(403, u),
			Some(ProviderError::Blocked)
		));
		assert!(matches!(
			map_http_status(429, u),
			Some(ProviderError::Blocked)
		));
		assert!(matches!(
			map_http_status(404, u),
			Some(ProviderError::NotFound(_))
		));
		assert!(map_http_status(200, u).is_none());
	}

	#[test]
	fn disposition_wins_and_takes_the_last_redirect() {
		// What `curl -D -` writes: one header block per hop, CRLF-terminated.
		let headers = [
			"HTTP/1.1 302 Found",
			"Content-Disposition: attachment; filename=\"first\".apk\"",
			"",
			"HTTP/1.1 200 OK",
			"Content-Type: application/octet-stream",
			"Content-Disposition: attachment; filename=\"yt_21.35.448_apkmirror.com\".apkm\"",
			"",
		]
		.join("\r\n");
		assert_eq!(ext_from_disposition(&headers).as_deref(), Some("apkm"));
	}

	#[test]
	fn disposition_rfc5987_and_junk() {
		assert_eq!(
			ext_from_disposition("Content-Disposition: attachment; filename*=UTF-8''app.xapk")
				.as_deref(),
			Some("xapk")
		);
		// A hostile name can't smuggle a path or a long suffix through.
		assert_eq!(
			ext_from_disposition("content-disposition: attachment; filename=\"x./../etc/passwd\""),
			None
		);
		assert_eq!(ext_from_disposition("Content-Type: text/html"), None);
	}

	#[test]
	fn falls_back_to_type_then_url() {
		assert_eq!(
			ext_from_type_or_url("application/vnd.android.package-archive", "").as_deref(),
			Some("apk")
		);
		assert_eq!(
			ext_from_type_or_url("application/octet-stream", "https://cdn/x.apkm?token=1")
				.as_deref(),
			Some("apkm")
		);
		// download.php?id=&key= tells us nothing — the file keeps its bare name.
		assert_eq!(
			ext_from_type_or_url(
				"application/octet-stream",
				"https://a/download.php?id=1&key=z"
			),
			None
		);
	}
}
