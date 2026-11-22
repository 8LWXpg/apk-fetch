//! Scraping helpers shared by the provider parsers.

use chrono::NaiveDate;
use form_urlencoded::byte_serialize;
use scraper::Selector;

use crate::common::{ProviderError, Result};

/// Parses a CSS selector.
///
/// Parsing is not free, so build each selector once (e.g. in a `LazyLock`
/// static) and reuse it rather than calling this per row or per request.
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

/// Encode the query with `form-urlencodes::byte_serialize`
pub fn query(q: &str) -> String {
	byte_serialize(q.trim().as_bytes()).collect()
}

pub fn parse_date(s: &str, fmt: &str) -> Result<NaiveDate> {
	NaiveDate::parse_from_str(s.trim(), fmt).map_err(|e| ProviderError::ParseError(e.to_string()))
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
	version == want || version.strip_prefix(want).is_some_and(|rest| rest.starts_with('.'))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn version_token_takes_the_trailing_number() {
		assert_eq!(version_token("Spotify 9.1.80.2221"), "9.1.80.2221");
		assert_eq!(version_token("YouTube 21.36.45 (arm64-v8a)"), "21.36.45");
		assert_eq!(version_token("nothing numeric"), "nothing numeric");
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
