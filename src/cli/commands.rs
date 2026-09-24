use super::exit::{AppError, EXIT_NETWORK, EXIT_NOT_FOUND};

use crate::common::ui::{error, info, print_message, success, warning};
use crate::common::{
	self, Arch, HttpFetcher, ProviderError, ProviderFailure, ProviderId, ProviderRegistry,
};

use std::path::Path;

use anyhow::anyhow;
use colored::Colorize;
use unicode_width::UnicodeWidthStr;

fn pad(s: &str, w: usize) -> String {
	format!("{s}{}", " ".repeat(w.saturating_sub(s.width())))
}

/// Width of the widest present value in a column, 0 if the column is all empty.
fn col_width<'a, T>(rows: &'a [T], f: impl Fn(&'a T) -> Option<&'a str>) -> usize {
	rows.iter().filter_map(f).map(str::width).max().unwrap_or(0)
}

fn emit<T>(provider: ProviderId, rows: &[T], label: fn(&T) -> &str, sub: fn(&T) -> String) {
	println!("{}", provider.as_str().cyan().bold());
	let w = col_width(rows, |r| Some(label(r)));
	for r in rows {
		println!(
			"{} {}  {}",
			"•".cyan().bold(),
			pad(label(r), w).bold(),
			sub(r).dimmed()
		);
	}
}

/// Run `fetch` against every selected provider, rendering (or merging, for
/// JSON) whatever each one returns. Errors are reported but don't stop the
/// sweep; if nothing came back at all, the last one becomes the exit status.
fn fan_out<T, F, R>(
	registry: &ProviderRegistry,
	json: bool,
	verb: &str,
	notfound: &str,
	mut fetch: F,
	mut render: R,
) -> Result<(), AppError>
where
	T: serde::Serialize,
	F: FnMut(ProviderId) -> Result<Vec<T>, ProviderFailure>,
	R: FnMut(ProviderId, &[T]),
{
	let mut merged = Vec::new();
	let mut hit = false;
	let mut last = None;
	for id in registry.names() {
		if !json {
			info!("{} {}...", verb, id);
		}
		match fetch(id) {
			Ok(rows) if rows.is_empty() => {
				if !json {
					warning!("{}: no results", id);
				}
			}
			Ok(rows) => {
				hit = true;
				if json {
					merged.push((id, rows));
				} else {
					render(id, &rows);
				}
			}
			Err(f) => {
				if !json {
					warning!("{f}");
				}
				last = Some(f.into());
			}
		}
	}
	if json {
		let mut map = serde_json::Map::new();
		for (id, rows) in merged {
			map.insert(id.to_string(), serde_json::to_value(rows)?);
		}
		println!(
			"{}",
			serde_json::to_string_pretty(&serde_json::Value::Object(map))?
		);
	} else if !hit {
		return Err(last.unwrap_or(AppError {
			code: EXIT_NOT_FOUND,
			source: anyhow!("{notfound}"),
		}));
	}
	Ok(())
}

pub fn search(registry: &ProviderRegistry, query: &str, json: bool) -> Result<(), AppError> {
	fan_out(
		registry,
		json,
		"searching",
		&format!("no results for '{query}'"),
		|id| registry.search(id, query),
		|id, rows| emit(id, rows, |r| r.title.as_str(), |r| r.package.to_string()),
	)
}

pub fn versions(registry: &ProviderRegistry, pkg: &str, json: bool) -> Result<(), AppError> {
	fan_out(
		registry,
		json,
		"listing versions from",
		&format!("no versions for '{pkg}'"),
		|id| registry.versions(id, pkg),
		|id, rows| emit(id, rows, |v| v.version.as_str(), |v| v.uploaded.to_string()),
	)
}

pub fn get(
	registry: &ProviderRegistry,
	pkg: &str,
	version: Option<&str>,
	arch: Arch,
	output: &Path,
	json: bool,
) -> Result<(), AppError> {
	let order: Vec<&str> = registry.names().iter().map(|p| p.as_str()).collect();
	info!("resolving {} ({})...", pkg, order.join(" -> "));
	let target = registry.resolve_with_fallback(pkg, version, arch)?;

	std::fs::create_dir_all(output).map_err(|e| AppError {
		code: EXIT_NETWORK,
		source: anyhow!("{e}"),
	})?;
	let dest = output.join(common::download_filename(pkg, &target.version, target.arch));

	info!(
		"downloading {} {} ({}) from {} ({})",
		pkg,
		target.version.bold(),
		target.arch.to_string().bold(),
		target.provider,
		target.url.dimmed()
	);
	let fetcher = HttpFetcher::new();
	let saved = fetcher
		.download_to_file(&target.url, &target.headers, &dest)
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
		print_message!("•", cyan, "{}  (priority {})", name, i + 1);
	}
	Ok(())
}

pub fn providers_check(registry: &ProviderRegistry, json: bool) -> Result<(), AppError> {
	let names = registry.names();
	let mut rows = Vec::new();
	for id in &names {
		if let Err(f) = registry.check(*id) {
			if json {
				rows.push(serde_json::json!({ "name": id, "status": format!("{}", f.source) }));
			} else {
				error!("{f}");
			}
		} else if json {
			rows.push(serde_json::json!({ "name": id, "status": "ok" }));
		} else {
			success!("{}: ok", id);
		}
	}
	if json {
		println!("{}", serde_json::to_string_pretty(&rows)?);
	}
	Ok(())
}
