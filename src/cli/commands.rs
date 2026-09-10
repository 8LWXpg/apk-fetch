use std::path::Path;

use std::future::Future;

use crate::common::{
	AppResult, HttpFetcher, ProviderError, ProviderFailure, ProviderId, ProviderRegistry,
	VersionInfo,
};
use crate::{error, info, success, warn};
use anyhow::anyhow;
use colored::Colorize;
use unicode_width::UnicodeWidthStr;

use super::exit::{AppError, EXIT_NETWORK, EXIT_NOT_FOUND};

// Pad `s` to `w` terminal columns, then color.
fn pad(s: &str, w: usize) -> String {
	format!("{s}{}", " ".repeat(w.saturating_sub(s.width())))
}

/// Width of the widest present value in a column, 0 if the column is all empty.
fn col_width<'a, T>(rows: &'a [T], f: impl Fn(&'a T) -> Option<&'a str>) -> usize {
	rows.iter().filter_map(f).map(str::width).max().unwrap_or(0)
}

fn render_results(provider: ProviderId, results: &[AppResult]) {
	println!("{}", provider.as_str().cyan().bold());
	let tw = col_width(results, |r| Some(r.title.as_str()));
	let vw = col_width(results, |r| r.version.as_deref());
	for r in results {
		let mut line = format!("{} {}", "•".cyan().bold(), pad(&r.title, tw).bold());
		if vw > 0 {
			line.push_str(&format!(
				"  {}",
				pad(r.version.as_deref().unwrap_or(""), vw).green()
			));
		}
		line.push_str(&format!("  {}", r.package.dimmed()));
		println!("{line}");
	}
}

fn render_versions(provider: ProviderId, list: &[VersionInfo]) {
	println!("{}", provider.as_str().cyan().bold());
	// Only pad the version column when a date follows it, else rows end in blanks.
	let vw = match col_width(list, |v| v.uploaded.as_deref()) {
		0 => 0,
		_ => col_width(list, |v| Some(v.version.as_str())),
	};
	for v in list {
		let mut line = format!("{} {}", "•".cyan().bold(), pad(&v.version, vw).bold());
		if let Some(up) = &v.uploaded {
			line.push_str(&format!("  {}", up.dimmed()));
		}
		println!("{line}");
	}
}

/// Run `fetch` against every selected provider, rendering (or merging, for
/// JSON) whatever each one returns. Errors are reported but don't stop the
/// sweep; if nothing came back at all, the last one becomes the exit status.
async fn fan_out<T, F, Fut>(
	registry: &ProviderRegistry,
	json: bool,
	verb: &str,
	mut fetch: F,
	render: fn(ProviderId, &[T]),
) -> Result<Vec<T>, Option<AppError>>
where
	F: FnMut(ProviderId) -> Fut,
	Fut: Future<Output = Result<Vec<T>, ProviderFailure>>,
{
	let mut merged = Vec::new();
	let mut hit = false;
	let mut last = None;
	for id in registry.names() {
		if !json {
			info!("{} {}...", verb, id);
		}
		match fetch(id).await {
			Ok(rows) if rows.is_empty() => {
				if !json {
					warn!("{}: no results", id);
				}
			}
			Ok(rows) => {
				hit = true;
				if json {
					merged.extend(rows);
				} else {
					render(id, &rows);
				}
			}
			Err(f) => {
				if !json {
					warn!("{f}");
				}
				last = Some(f.into());
			}
		}
	}
	if hit { Ok(merged) } else { Err(last) }
}

pub(super) async fn search(
	registry: &ProviderRegistry,
	query: &str,
	json: bool,
) -> Result<(), AppError> {
	let merged = fan_out(
		registry,
		json,
		"searching",
		|id| registry.search(id, query),
		render_results,
	)
	.await
	.map_err(|last| {
		last.unwrap_or(AppError {
			code: EXIT_NOT_FOUND,
			source: anyhow!("no results for '{query}'"),
		})
	})?;
	if json {
		println!("{}", serde_json::to_string_pretty(&merged)?);
	}
	Ok(())
}

pub(super) async fn versions(
	registry: &ProviderRegistry,
	pkg: &str,
	json: bool,
) -> Result<(), AppError> {
	let merged = fan_out(
		registry,
		json,
		"listing versions from",
		|id| registry.versions(id, pkg),
		render_versions,
	)
	.await
	.map_err(|last| {
		last.unwrap_or(AppError {
			code: EXIT_NOT_FOUND,
			source: anyhow!("no versions for '{pkg}'"),
		})
	})?;
	if json {
		println!("{}", serde_json::to_string_pretty(&merged)?);
	}
	Ok(())
}

pub(super) async fn get(
	registry: &ProviderRegistry,
	pkg: &str,
	version: Option<&str>,
	arch: &str,
	output: &Path,
	json: bool,
) -> Result<(), AppError> {
	let names = registry.names();
	let order: Vec<&str> = names.iter().map(|p| p.as_str()).collect();
	info!("resolving {} ({})...", pkg, order.join(" -> "));
	let target = registry.resolve_with_fallback(pkg, version, arch).await?;

	std::fs::create_dir_all(output).map_err(|e| AppError {
		code: EXIT_NETWORK,
		source: anyhow!("{e}"),
	})?;
	let dest = output.join(crate::common::download_filename(
		pkg,
		target.version.as_deref().unwrap_or("latest"),
		target.arch.as_deref(),
	));

	info!("downloading from {} ({})", target.provider, target.url);
	let fetcher = HttpFetcher::new();
	let saved = fetcher
		.download_to_file(&target.url, &target.headers, &dest)
		.await
		.map_err(|e| AppError {
			code: super::exit::provider_error_code(&e),
			// A cancel isn't a failure — don't dress it up as one.
			source: match e {
				ProviderError::Cancelled => anyhow!("cancelled"),
				e => anyhow!("download failed: {e}"),
			},
		})?;

	if json {
		println!(
			"{}",
			serde_json::json!({
				"path": saved.display().to_string(),
				"provider": target.provider,
				"version": target.version,
				"arch": target.arch,
			})
		);
	} else {
		success!("saved {}", saved.display());
	}
	Ok(())
}

pub(super) fn providers_list(registry: &ProviderRegistry, json: bool) -> Result<(), AppError> {
	let names = registry.names();
	if json {
		let rows: Vec<_> = names
			.iter()
			.enumerate()
			.map(|(i, n)| serde_json::json!({ "name": n, "priority": i + 1 }))
			.collect();
		println!("{}", serde_json::to_string_pretty(&rows)?);
		return Ok(());
	}
	for (i, name) in names.iter().enumerate() {
		crate::print_message!("•", cyan, "{}  (priority {})", name, i + 1);
	}
	Ok(())
}

pub(super) async fn providers_check(
	registry: &ProviderRegistry,
	json: bool,
) -> Result<(), AppError> {
	let targets = registry.names();
	let mut results = Vec::new();
	let mut worst: Option<AppError> = None;
	for id in targets {
		match registry.check(id).await {
			Ok(()) => {
				results.push((id, "ok".to_string()));
				if !json {
					success!("{}: ok", id);
				}
			}
			Err(f) => {
				results.push((id, format!("{}", f.source)));
				if !json {
					error!("{f}");
				}
				worst = Some(f.into());
			}
		}
	}
	if json {
		let rows: Vec<_> = results
			.iter()
			.map(|(n, s)| serde_json::json!({ "name": n, "status": s }))
			.collect();
		println!("{}", serde_json::to_string_pretty(&rows)?);
	}
	// Only fail the process when a single named provider was checked and failed.
	match worst {
		Some(e) if results.len() == 1 => Err(e),
		_ => Ok(()),
	}
}
