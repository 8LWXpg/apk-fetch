//! HTTP via the system `curl`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::common::contract::ProviderError;
use tokio::process::Command;
use tokio::sync::Mutex;

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

/// Extension from the last `Content-Disposition` filename in a header dump.
fn ext_from_disposition(headers: &str) -> Option<String> {
	let ext = headers
		.lines()
		.rev() // Walk from the last header backwards...
		.filter(|l| {
			l.get(..20)
				.is_some_and(|p| p.eq_ignore_ascii_case("content-disposition:"))
		})
		.find_map(|l| {
			// `filename="x.apk"`, or RFC 5987 `filename*=UTF-8\x.apk`.
			let v = l.split_once("filename")?.1.trim_start_matches(['*', '=']);
			let v = v.rsplit("''").next()?.trim().trim_matches('"');
			v.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())
		})?;
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

/// Served extension from curl's stdout (header dumps, then the `-w` line after
/// [`META_SENTINEL`]): `Content-Disposition`, then `Content-Type`, then final URL.
fn served_ext(curl_stdout: &str) -> Option<String> {
	let (headers, meta) = curl_stdout
		.rsplit_once(META_SENTINEL)
		.unwrap_or((curl_stdout, ""));
	let (ct, url) = meta.split_once('\t').unwrap_or((meta, ""));
	ext_from_disposition(headers).or_else(|| ext_from_type_or_url(ct, url))
}

/// First bytes of a local ZIP (APK/XAPK) — `PK\x03\x04`, or the empty-archive
/// `PK\x05\x06`. An empty or missing file fails the check.
async fn starts_with_zip_magic(path: &Path) -> bool {
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

/// A curl-backed fetcher with a per-instance throttle. Construct one per provider
/// so each site gets its own request cadence.
/// Maps a fetched URL to its fixture file stem, or `None` to skip it.
#[cfg(test)]
pub type FixtureName = fn(&str) -> Option<&'static str>;

pub struct HttpFetcher {
	min_gap: Duration,
	last_request: Mutex<Option<Instant>>,
	/// Fixture recorder: every 2xx body lands in `dir/<name(url)>.html`.
	#[cfg(test)]
	record: Option<(PathBuf, FixtureName)>,
}

impl HttpFetcher {
	pub fn new() -> Self {
		Self::with_delay(THROTTLE_DELAY)
	}

	pub fn with_delay(min_gap: Duration) -> Self {
		Self {
			min_gap,
			last_request: Mutex::new(None),
			#[cfg(test)]
			record: None,
		}
	}

	/// A fetcher that also saves what it fetches, so fixtures are captured via
	/// the provider's own URL building instead of a second copy of it.
	#[cfg(test)]
	pub fn recording(dir: PathBuf, name: FixtureName) -> Self {
		Self {
			record: Some((dir, name)),
			..Self::new()
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
		cmd.kill_on_drop(true);
		cmd
	}

	pub async fn get_text(&self, url: &str) -> Result<String, ProviderError> {
		self.request(url, &[], &[]).await
	}

	/// POST `url` with a `multipart/form-data` body (curl `-F`).
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
				.args(["-w", "\n%{http_code}"])
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
			#[cfg(test)]
			if let Some((dir, name)) = &self.record
				&& let Some(name) = name(url)
			{
				std::fs::create_dir_all(dir).expect("fixture dir");
				std::fs::write(dir.join(format!("{name}.html")), trim_html(body))
					.expect("write fixture");
			}
			return Ok(body.to_string());
		}
	}

	/// Resolve URL redirection. No redirect means the URL is returned unchanged.
	pub async fn resolve_url(&self, url: &str) -> Result<String, ProviderError> {
		self.throttle().await;

		let output = Self::base_cmd(url, &[], false)
			.args([
				"-I",
				"--max-time",
				&REQUEST_TIMEOUT_SECS.to_string(),
				"-w",
				"\n%{url_effective}",
			])
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

	/// Stream a URL to `dest` (extension-less; the response decides `.apk` vs
	/// `.xapk`/`.apkm`). Returns the path written. Failure leaves no file.
	pub async fn download_to_file(
		&self,
		url: &str,
		headers: &[(String, String)],
		dest: &Path,
	) -> Result<PathBuf, ProviderError> {
		self.throttle().await;
		let result = Self::curl_to_file(url, headers, dest).await;
		if result.is_err() {
			let _ = tokio::fs::remove_file(dest).await;
		}
		result
	}

	async fn curl_to_file(
		url: &str,
		headers: &[(String, String)],
		dest: &Path,
	) -> Result<PathBuf, ProviderError> {
		// `.output()` would force stderr to a pipe and hide curl's progress bar;
		// spawn instead so stderr stays on the inherited terminal.
		let mut child = Self::base_cmd(url, headers, true)
			.args(["--fail", "--retry", &MAX_RETRIES.to_string(), "-o"])
			.arg(dest)
			.args([
				"-D",
				"-",
				"-w",
				&format!("\n{META_SENTINEL}%{{content_type}}\t%{{url_effective}}"),
			])
			.stdout(Stdio::piped())
			.stderr(Stdio::inherit())
			.spawn()
			.map_err(|e| {
				network_err(format!(
					"could not run `curl` (is it installed and on PATH?): {e}"
				))
			})?;

		// Waits curl and handle Ctrl+C
		let status = tokio::select! {
			status = child.wait() => status.map_err(network_err)?,
			_ = tokio::signal::ctrl_c() => {
				let _ = child.kill().await;
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
			// curl already printed the reason to stderr.
			return Err(network_err(format!("curl download failed ({status})")));
		}
		// Non-ZIP payload = error/landing page that came back 200.
		if !starts_with_zip_magic(dest).await {
			return Err(network_err(
				"downloaded file is not an APK (server returned a non-package response)",
			));
		}
		let Some(ext) = served_ext(&stdout_buf) else {
			return Ok(dest.to_path_buf());
		};
		// Appended, not `set_extension`: the stem holds dotted version numbers.
		let target = PathBuf::from(format!("{}.{ext}", dest.display()));
		tokio::fs::rename(dest, &target)
			.await
			.map_err(network_err)?;
		Ok(target)
	}
}

impl Default for HttpFetcher {
	fn default() -> Self {
		Self::new()
	}
}

/// Drop `<script>`/`<style>`/`<svg>`/`<noscript>` elements: fixtures shrink
/// several-fold and nothing the parsers read lives there.
#[cfg(test)]
fn trim_html(html: &str) -> String {
	let lower = html.to_ascii_lowercase();
	let mut out = String::with_capacity(html.len());
	let mut pos = 0;
	while let Some((start, tag)) = ["script", "style", "svg", "noscript"]
		.iter()
		.filter_map(|t| lower[pos..].find(&format!("<{t}")).map(|i| (pos + i, *t)))
		.min()
	{
		out.push_str(&html[pos..start]);
		let close = format!("</{tag}>");
		let Some(end) = lower[start..].find(&close) else {
			pos = start; // unterminated: keep the tail as-is
			break;
		};
		pos = start + end + close.len();
	}
	out.push_str(&html[pos..]);
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn trim_html_strips_noise_only() {
		assert_eq!(
			trim_html("<a>x</a><SCRIPT src=1>var y</script><b>z</b><style>.c{}</style>"),
			"<a>x</a><b>z</b>"
		);
		assert_eq!(
			trim_html("<p>ok</p><script>never closed"),
			"<p>ok</p><script>never closed"
		);
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
	fn served_ext_splits_curl_stdout() {
		let out = format!(
			"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\r\n\n{META_SENTINEL}application/octet-stream\thttps://cdn/x.xapk?t=1"
		);
		assert_eq!(served_ext(&out).as_deref(), Some("xapk"));
		assert_eq!(served_ext("no sentinel at all"), None);
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
