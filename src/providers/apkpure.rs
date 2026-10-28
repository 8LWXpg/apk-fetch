//! APKPure is package-id-addressable
//! — `/x/{pkg}/download`: latest download
//! - `/x/{pkg}/download/{version}`: versioned download
//! `d.apkpure.com/b/APK/{pkg}?versionCode=...` 302 straight to the APK.
mod parse;
use parse::{Url, Variant};
use scraper::Html;

use crate::common::contract::{
	AppResult, Arch, DownloadTarget, Provider, ProviderConst, ProviderId, Result, VersionInfo,
};
use crate::common::fetch::HttpFetcher;
use crate::providers::scrape::query;

#[derive(Default)]
pub struct ApkPure {
	fetcher: HttpFetcher,
}

impl ApkPure {
	fn version_rows(&self, pkg: &str) -> Result<Vec<parse::VersionRow>> {
		parse::parse_versions(&Html::parse_document(
			&self
				.fetcher
				.get_text(Url::from(format!("/x/{pkg}/versions")).as_str())?,
		))
	}

	fn get_variants(&self, pkg: &str, version: Option<&str>, arch: Arch) -> Result<Variant> {
		parse::parse_variants(
			&Html::parse_document(
				&self.fetcher.get_text(
					Url::from(format!(
						"/x/{pkg}/download/{}",
						if let Some(v) = version { v } else { "" }
					))
					.as_str(),
				)?,
			),
			arch,
		)
	}
}

impl ProviderConst for ApkPure {
	const ID: ProviderId = ProviderId::Apkpure;
	const BASE_URL: &'static str = "https://apkpure.com";
}

impl Provider for ApkPure {
	#[cfg(test)]
	fn with_fetcher(fetcher: HttpFetcher) -> Self
	where
		Self: Sized,
	{
		Self { fetcher }
	}

	fn id(&self) -> ProviderId {
		Self::ID
	}

	fn search(&self, q: &str) -> Result<Vec<AppResult>> {
		parse::parse_search(&Html::parse_document(
			&self
				.fetcher
				.get_text(Url::from(format!("/search?q={}", query(q))).as_str())?,
		))
	}

	fn versions(&self, pkg: &str) -> Result<Vec<VersionInfo>> {
		Ok(self
			.version_rows(pkg)?
			.into_iter()
			.map(|r| VersionInfo {
				version: r.version,
				uploaded: r.uploaded,
			})
			.collect())
	}

	fn download_url(&self, pkg: &str, version: Option<&str>, arch: Arch) -> Result<DownloadTarget> {
		// `code` is APKPure `versionCode`, `None` means "latest".
		let variant = self.get_variants(pkg, version, arch)?;

		Ok(DownloadTarget {
			url: variant.url.to_string(),
			version: variant.version,
			arch: variant.arch,
			provider: Self::ID,
			headers: Vec::new(),
		})
	}
}

/// Fixture file for each URL the provider fetches.
#[cfg(test)]
fn fixture_name(url: &str) -> Option<&'static str> {
	Some(if url.contains("/search?q=") {
		"search"
	} else if url.ends_with("/versions") {
		"versions"
	} else {
		"app"
	})
}

/// `cargo test refresh_fixtures -- --ignored`
#[cfg(test)]
mod refresh {
	use super::*;
	use crate::providers::fixtures;

	#[test]
	#[ignore = "network"]
	fn refresh_fixtures() {
		fixtures::refresh_fixtures::<ApkPure>(fixture_name);
	}
}

/// Replays the whole resolution flow offline against the recorded fixtures,
/// diffing search/versions/download_url output against the `record.json` one.
#[cfg(test)]
mod tests {
	use super::*;
	use crate::providers::fixtures::{APPS, assert_invariants, assert_record, root, run_flow};

	#[test]
	fn resolves_from_fixtures() {
		for (app, pkg) in APPS {
			let dir = root("apkpure").join(app);
			let p = ApkPure {
				fetcher: HttpFetcher::playback(dir.clone(), fixture_name),
			};

			let record = run_flow(&p, app, pkg);
			assert_record(&dir, &record, app);
			assert_invariants(&record, pkg, app);

			assert!(
				record.target.url.starts_with("https://d.apkpure.com/b/") && record.target.url.contains(pkg),
				"{app}: {}",
				record.target.url
			);
			assert!(record.target.headers.is_empty(), "{app}: unexpected headers");
		}
	}
}
