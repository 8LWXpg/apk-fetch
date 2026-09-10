use std::path::Path;

use crate::common::{AppResult, HttpFetcher, ProviderError, ProviderId, ProviderRegistry};
use crate::{error, info, success, warn};
use anyhow::anyhow;
use colored::Colorize;
use unicode_width::UnicodeWidthStr;

use super::{AppError, EXIT_NETWORK, EXIT_NOT_FOUND};

fn render_results(provider: ProviderId, results: &[AppResult]) {
	// Pad `s` to `w` terminal columns, then color.
	let pad = |s: &str, w: usize| format!("{s}{}", " ".repeat(w.saturating_sub(s.width())));

	println!("{}", provider.as_str().cyan().bold());
	let tw = results.iter().map(|r| r.title.width()).max().unwrap_or(0);
	let vw = results
		.iter()
		.filter_map(|r| r.version.as_deref())
		.map(str::width)
		.max()
		.unwrap_or(0);
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

pub async fn search(registry: &ProviderRegistry, query: &str, json: bool) -> Result<(), AppError> {
	let mut merged: Vec<AppResult> = Vec::new();
	let mut hit = false;
	let mut last: Option<AppError> = None;
	for id in registry.names() {
		if !json {
			info!("searching {}...", id);
		}
		match registry.search(id, query).await {
			Ok(results) if results.is_empty() => {
				if !json {
					warn!("{}: no results", id);
				}
			}
			Ok(results) => {
				hit = true;
				if json {
					merged.extend(results);
				} else {
					render_results(id, &results);
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

	if json {
		println!("{}", serde_json::to_string_pretty(&merged)?);
	}
	if hit {
		Ok(())
	} else {
		Err(last.unwrap_or(AppError {
			code: EXIT_NOT_FOUND,
			source: anyhow!("no results for '{query}'"),
		}))
	}
}

pub async fn versions(registry: &ProviderRegistry, pkg: &str, json: bool) -> Result<(), AppError> {
	let id = registry.top();
	let list = registry.versions(id, pkg).await?;
	if json {
		println!("{}", serde_json::to_string_pretty(&list)?);
	} else {
		for v in &list {
			crate::print_message!("•", cyan, "{}  ({})", v.version, v.provider);
		}
	}
	Ok(())
}

pub async fn get(
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
			code: super::provider_error_code(&e),
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

pub fn providers_list(registry: &ProviderRegistry, json: bool) -> Result<(), AppError> {
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

pub async fn providers_check(registry: &ProviderRegistry, json: bool) -> Result<(), AppError> {
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
