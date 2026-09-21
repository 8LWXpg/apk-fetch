//! Scraping helpers shared by the provider parsers.

use crate::common::{
	contract::{Arch, ProviderError},
	ui::error,
};
use chrono::NaiveDate;
use form_urlencoded::byte_serialize;
use scraper::Selector;

pub fn sel(s: &str) -> Selector {
	Selector::parse(s).unwrap_or_else(|e| panic!("invalid CSS selector {s:?}: {e}"))
}

pub fn text_of(el: scraper::ElementRef<'_>) -> String {
	el.text()
		.collect::<String>()
		.split_whitespace()
		.collect::<Vec<_>>()
		.join(" ")
}

pub fn parse_err(msg: impl Into<String>) -> ProviderError {
	ProviderError::ParseError(msg.into())
}

pub fn abs(base: &str, href: &str) -> String {
	if href.starts_with("http") {
		href.to_string()
	} else {
		format!("{base}/{}", href.trim_start_matches('/'))
	}
}

/// Encode the query with `form-urlencodes::byte_serialize`
pub fn query(q: &str) -> String {
	byte_serialize(q.trim().as_bytes()).collect()
}

/// Upload date off a listing row. A row that fails to parse is reported and
/// dropped by the caller, so a site format change shows up instead of silently
/// emptying the list.
pub fn parse_date(provider: &str, label: &str, s: &str, fmt: &str) -> Option<NaiveDate> {
	NaiveDate::parse_from_str(s.trim(), fmt)
		.inspect_err(|_| error!("{provider}: {label}: unparseable upload date {s:?}"))
		.ok()
}

/// One downloadable build off a version page.
#[derive(Debug)]
pub struct Variant {
	pub version: String,
	pub arch: Arch,
	/// Split bundle (XAPK / APKM) rather than a plain APK.
	pub bundle: bool,
	/// Next hop in the site's download flow (absolute).
	pub url: String,
}

/// Pick a variant for `arch`. Plain APKs first, then bundles; within each, the
/// build that includes `arch` with the fewest other ABIs (exact beats
/// universal), else any. Then the first row.
///
/// Note: dpi is ignored as most sites don't have them.
pub fn choose_variant(variants: &[Variant], arch: Arch) -> Option<&Variant> {
	let tightest = |ok: &dyn Fn(&&Variant) -> bool| {
		variants
			.iter()
			.filter(|v| ok(v) && v.arch.contains(arch))
			.min_by_key(|v| v.arch.bits().count_ones())
	};
	tightest(&|v| !v.bundle)
		.or_else(|| variants.iter().find(|v| !v.bundle))
		.or_else(|| tightest(&|_| true))
		.or_else(|| variants.first())
}

/// Best-effort: last whitespace token that looks like a version number.
pub fn version_token(name: &str) -> String {
	name.split_whitespace()
		.rev()
		.find(|t| t.starts_with(|c: char| c.is_ascii_digit()))
		.unwrap_or(name)
		.to_string()
}

/// Matches exact, or leading dotted-segment prefix. Not a substring test:
/// `"21.36.45".contains("1.3")` is true and would resolve the wrong build.
pub fn version_matches(version: &str, want: &str) -> bool {
	version == want
		|| version
			.strip_prefix(want)
			.is_some_and(|rest| rest.starts_with('.'))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn abs_resolves_against_base() {
		let base = "https://example.com";
		assert_eq!(abs(base, "https://cdn.x/y"), "https://cdn.x/y");
		assert_eq!(abs(base, "/apk/x"), "https://example.com/apk/x");
		assert_eq!(abs(base, "apk/x"), "https://example.com/apk/x");
	}

	#[test]
	fn version_token_takes_the_trailing_number() {
		assert_eq!(version_token("Spotify 9.1.80.2221"), "9.1.80.2221");
		assert_eq!(version_token("YouTube 21.36.45 (arm64-v8a)"), "21.36.45");
		assert_eq!(version_token("nothing numeric"), "nothing numeric");
	}

	fn variant(bundle: bool, arch: Arch) -> Variant {
		Variant {
			version: "1.0".into(),
			arch,
			bundle,
			url: "https://x/".into(),
		}
	}

	#[test]
	fn choose_variant_prefers_apk_then_exact_arch() {
		let vs = vec![
			variant(true, Arch::ARM64_V8A),
			variant(false, Arch::ARMEABI_V7A),
			variant(false, Arch::ARM64_V8A),
			variant(false, Arch::all()),
		];
		// Exact-arch APK wins over an arch-matching bundle and other APKs
		assert_eq!(
			choose_variant(&vs, Arch::ARM64_V8A).unwrap().arch,
			Arch::ARM64_V8A
		);
		assert_eq!(
			choose_variant(&vs, Arch::ARMEABI_V7A).unwrap().arch,
			Arch::ARMEABI_V7A
		);
		// No exact match -> an APK that includes the ABI (universal), never the bundle
		let picked = choose_variant(&vs, Arch::X86).unwrap();
		assert!(!picked.bundle);
		assert_eq!(picked.arch, Arch::all());

		// Bundle-only app: arch match on the bundle beats an off-arch bundle
		let bundles = vec![variant(true, Arch::all()), variant(true, Arch::ARM64_V8A)];
		assert_eq!(
			choose_variant(&bundles, Arch::ARM64_V8A).unwrap().arch,
			Arch::ARM64_V8A
		);
	}

	#[test]
	fn version_matches_is_boundary_aware() {
		assert!(version_matches("2.6.2", "2.6.2"));
		assert!(version_matches("2.6.2", "2.6"));
		assert!(!version_matches("2.60", "2.6"));
		assert!(!version_matches("12.6.2", "2.6"));
		assert!(!version_matches("21.36.45", "1.3"));
	}
}
