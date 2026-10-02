//! HTTP via the system `curl`.

use std::cell::Cell;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use std::{fs, thread};

use crate::common::contract::ProviderError;

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

fn looks_blocked(body: &str) -> bool {
	BLOCK_SIGNATURES.iter().any(|sig| body.contains(sig))
}

/// Set by the Ctrl+C handler.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn install_ctrlc() {
	let _ = ctrlc::set_handler(move || INTERRUPTED.store(true, Ordering::SeqCst));
}

fn interrupted() -> bool {
	INTERRUPTED.load(Ordering::SeqCst)
}

/// Bail if cancelled.
macro_rules! cancelled {
	() => {
		if interrupted() {
			return Err(ProviderError::Cancelled);
		}
	};
	(clear) => {
		if interrupted() {
			eprintln!();
			return Err(ProviderError::Cancelled);
		}
	};
}
struct Scratch(PathBuf);

impl Scratch {
	/// Fresh, empty dir next to `dest`.
	fn new(dest: &Path) -> Result<Self, ProviderError> {
		let name = dest.file_name().unwrap_or_default().to_string_lossy();
		let dir = dest.with_file_name(format!(".{name}.part"));
		let _ = fs::remove_dir_all(&dir);
		fs::create_dir_all(&dir)?;
		Ok(Self(dir))
	}

	fn path(&self) -> &Path {
		&self.0
	}

	/// The single file curl wrote.
	fn only_file(&self) -> Result<PathBuf, ProviderError> {
		let mut entries = fs::read_dir(&self.0)?;
		let first = entries
			.next()
			.ok_or_else(|| ProviderError::network("curl wrote no file"))??
			.path();
		if entries.next().is_some() {
			return Err(ProviderError::network("curl wrote more than one file"));
		}
		Ok(first)
	}
}

impl Drop for Scratch {
	fn drop(&mut self) {
		let _ = fs::remove_dir_all(&self.0);
	}
}

/// First bytes of a local ZIP (APK/XAPK) — `PK\x03\x04`, or the empty-archive
/// `PK\x05\x06`. An empty or missing file fails the check.
fn starts_with_zip_magic(path: &Path) -> bool {
	let Ok(mut f) = fs::File::open(path) else {
		return false;
	};
	let mut head = [0u8; 4];
	match f.read_exact(&mut head) {
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
		s if s >= 500 => Some(ProviderError::network(format!("upstream returned {s}"))),
		_ => None,
	}
}

/// Maps a fetched URL to its fixture file stem, or `None` to skip it.
#[cfg(test)]
pub type FixtureName = fn(&str) -> Option<&'static str>;

/// A curl-backed fetcher with a per-instance throttle. Construct one per provider
/// so each site gets its own request cadence.
pub struct HttpFetcher {
	min_gap: Duration,
	last_request: Cell<Option<Instant>>,
	/// Fixture recorder: every 2xx body lands in `dir/<name(url)>.html`.
	#[cfg(test)]
	record: Option<(PathBuf, FixtureName)>,
	/// Fixture player: serves `dir/<name(url)>.html` instead of the network.
	#[cfg(test)]
	play: Option<(PathBuf, FixtureName)>,
}

impl HttpFetcher {
	pub fn new() -> Self {
		Self::with_delay(THROTTLE_DELAY)
	}

	pub fn with_delay(min_gap: Duration) -> Self {
		Self {
			min_gap,
			last_request: Cell::new(None),
			#[cfg(test)]
			record: None,
			#[cfg(test)]
			play: None,
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

	/// [`recording`]'s counterpart: replays the saved fixtures, so provider
	/// flows run offline against fixed pages.
	#[cfg(test)]
	pub fn playback(dir: PathBuf, name: FixtureName) -> Self {
		Self {
			play: Some((dir, name)),
			..Self::with_delay(Duration::ZERO)
		}
	}

	/// Read `dir/<name(url)>.html`, mapping the URL exactly as recording did.
	#[cfg(test)]
	fn play_body(dir: &Path, name: &FixtureName, url: &str) -> String {
		let name = name(url).unwrap_or_else(|| panic!("playback: no fixture mapping for {url}"));
		let file = dir.join(format!("{name}.html"));
		fs::read_to_string(&file).unwrap_or_else(|e| panic!("playback: {}: {e}", file.display()))
	}

	fn throttle(&self) {
		let prev = self.last_request.get();
		if let Some(p) = prev
			&& p.elapsed() < self.min_gap
		{
			thread::sleep(self.min_gap - p.elapsed());
		}
		self.last_request.set(Some(Instant::now()));
	}

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
		cmd
	}

	pub fn get_text(&self, url: &str) -> Result<String, ProviderError> {
		self.request(url, &[], &[])
	}

	/// POST `url` with a `multipart/form-data` body (curl `-F`).
	pub fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<String, ProviderError> {
		let mut extra: Vec<String> = vec!["-X".into(), "POST".into()];
		for (k, v) in form {
			extra.push("-F".into());
			extra.push(format!("{k}={v}"));
		}
		let extra_ref: Vec<&str> = extra.iter().map(String::as_str).collect();
		self.request(url, &[], &extra_ref)
	}

	fn request(&self, url: &str, headers: &[(String, String)], extra_args: &[&str]) -> Result<String, ProviderError> {
		#[cfg(test)]
		if let Some((dir, name)) = &self.play {
			return Ok(Self::play_body(dir, name, url));
		}

		let mut attempt = 0;
		loop {
			cancelled!();
			self.throttle();

			let output = Self::base_cmd(url, headers, false)
				.args(["--max-time", &REQUEST_TIMEOUT_SECS.to_string()])
				.args(extra_args)
				.args(["-w", "\n%{http_code}"])
				.output()
				.map_err(ProviderError::CurlSpawn)?;

			cancelled!();

			if !output.status.success() {
				let code = output.status.code();
				if curl_exit_is_transient(code) && attempt < MAX_RETRIES {
					attempt += 1;
					thread::sleep(RETRY_BASE_BACKOFF * attempt);
					continue;
				}
				return Err(ProviderError::network(format!(
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
					thread::sleep(RETRY_BASE_BACKOFF * attempt);
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
				fs::create_dir_all(dir).unwrap();
				fs::write(dir.join(format!("{name}.html")), trim_html(body)).unwrap();
			}
			return Ok(body.to_string());
		}
	}

	/// Resolve URL redirection. No redirect means the URL is returned unchanged.
	pub fn resolve_url(&self, url: &str) -> Result<String, ProviderError> {
		#[cfg(test)]
		if let Some((dir, name)) = &self.play {
			return Ok(Self::play_body(dir, name, url));
		}

		cancelled!();
		self.throttle();

		let output = Self::base_cmd(url, &[], false)
			.args([
				"-I",
				"--max-time",
				&REQUEST_TIMEOUT_SECS.to_string(),
				"-w",
				"\n%{url_effective}",
			])
			.output()
			.map_err(ProviderError::CurlSpawn)?;

		cancelled!();

		if !output.status.success() {
			return Err(ProviderError::network(format!(
				"curl exited {:?}: {}",
				output.status.code(),
				String::from_utf8_lossy(&output.stderr).trim()
			)));
		}
		let stdout = String::from_utf8_lossy(&output.stdout);
		let effective = stdout.rsplit('\n').next().unwrap_or_default().trim().to_string();

		// Record the effective URL where playback's `resolve_url` can read it.
		#[cfg(test)]
		if let Some((dir, name)) = &self.record
			&& let Some(name) = name(url)
		{
			fs::create_dir_all(dir).unwrap();
			fs::write(dir.join(format!("{name}.html")), &effective).unwrap();
		}

		Ok(effective)
	}

	/// Stream a URL to `dest` (extension-less; the response decides `.apk` vs
	/// `.xapk`/`.apkm`). Returns the path written. Failure leaves no file.
	///
	/// `dest`: Path without ext.
	pub fn download_to_file(
		&self,
		url: &str,
		headers: &[(String, String)],
		dest: &Path,
	) -> Result<PathBuf, ProviderError> {
		self.throttle();
		let result = Self::curl_to_file(url, headers, dest);
		if result.is_err() {
			let _ = fs::remove_file(dest);
		}
		result
	}

	/// `dest`: Path without ext
	fn curl_to_file(url: &str, headers: &[(String, String)], dest: &Path) -> Result<PathBuf, ProviderError> {
		let scratch = Scratch::new(dest)?; // fresh dir beside dest, Drop = remove_dir_all
		let status = Self::base_cmd(url, headers, true)
			.args([
				"--fail",
				"--retry",
				&MAX_RETRIES.to_string(),
				"-O",
				"-J",
				"--output-dir",
			])
			.arg(scratch.path())
			.stdout(Stdio::null())
			.stderr(Stdio::inherit())
			.spawn()
			.map_err(ProviderError::CurlSpawn)?
			.wait()?;
		cancelled!(clear); // Drop wipes scratch on every path
		if !status.success() {
			return Err(ProviderError::network(format!("curl download failed ({status})")));
		}

		let saved = scratch.only_file()?; // read_dir: exactly one entry
		if !starts_with_zip_magic(&saved) {
			return Err(ProviderError::network("server returned a non-APK response"));
		}
		let ext = saved
			.extension()
			.and_then(|e| e.to_str())
			.filter(|e| matches!(e.to_ascii_lowercase().as_str(), "apk" | "xapk" | "apkm" | "apks"))
			.ok_or_else(|| ProviderError::network("server gave no recognizable package filename"))?;
		let target = PathBuf::from(format!("{}.{ext}", dest.display()));
		fs::rename(&saved, &target)?;
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
		assert!(matches!(map_http_status(403, u), Some(ProviderError::Blocked)));
		assert!(matches!(map_http_status(429, u), Some(ProviderError::Blocked)));
		assert!(matches!(map_http_status(404, u), Some(ProviderError::NotFound(_))));
		assert!(map_http_status(200, u).is_none());
	}
}
