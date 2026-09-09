//! One handler per subcommand. All business logic lives in `contract` / `fetch` /
//! `providers`; these just orchestrate calls and render output.

use std::path::Path;

use anyhow::anyhow;
use apk_fetch::contract::{
    AppResult, ProviderId, ProviderError, ProviderFailure, ProviderRegistry, ResolveError, error,
    info, success, warn,
};
use apk_fetch::fetch::HttpFetcher;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use unicode_width::UnicodeWidthStr;

use crate::{AppError, EXIT_BLOCKED, EXIT_NETWORK, EXIT_NOT_FOUND};

/// First entry of the default priority — the single provider `search` / `versions`
/// / plain `get` use when none is named.
const DEFAULT_PROVIDER: ProviderId = ProviderId::DEFAULT_PRIORITY[0];

fn provider_error_code(e: &ProviderError) -> u8 {
    match e {
        ProviderError::NotFound(_) => EXIT_NOT_FOUND,
        ProviderError::Blocked { .. } | ProviderError::RateLimited => EXIT_BLOCKED,
        ProviderError::Network(_) => EXIT_NETWORK,
        ProviderError::ParseError(_) => crate::EXIT_GENERIC,
    }
}

fn provider_fail(f: ProviderFailure) -> AppError {
    AppError {
        code: provider_error_code(&f.source),
        source: anyhow!("{f}"),
    }
}

fn resolve_err(e: ResolveError) -> AppError {
    let code = if e.all_not_found() {
        EXIT_NOT_FOUND
    } else if e.all_blocked() {
        EXIT_BLOCKED
    } else if e.any_network() {
        EXIT_NETWORK
    } else {
        crate::EXIT_GENERIC
    };
    AppError {
        code,
        source: anyhow!("{e}"),
    }
}

/// `• {title}  {version}  {package}`, columns padded to line up, under a
/// provider-name header.
fn render_results(provider: ProviderId, results: &[AppResult]) {
    // Pad `s` to `w` terminal columns, then colour — `{:<w$}` counts chars, which
    // is wrong for CJK / wide glyphs, so measure with unicode-width instead.
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

/// Search every provider in `providers` (or all registered ones when `all`, or
/// just the default when neither) and show each one's hits — this is discovery,
/// not download failover, so we don't stop at the first that answers.
pub async fn search(
    registry: &ProviderRegistry,
    query: &str,
    providers: &[ProviderId],
    all: bool,
    json: bool,
) -> Result<(), AppError> {
    let targets: Vec<ProviderId> = if all {
        registry.names()
    } else if providers.is_empty() {
        vec![DEFAULT_PROVIDER]
    } else {
        providers.to_vec()
    };

    let mut merged: Vec<AppResult> = Vec::new();
    let mut hit = false;
    let mut last: Option<AppError> = None;
    for id in targets {
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
                last = Some(provider_fail(f));
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

pub async fn versions(
    registry: &ProviderRegistry,
    pkg: &str,
    provider: Option<ProviderId>,
    json: bool,
) -> Result<(), AppError> {
    let id = provider.unwrap_or(DEFAULT_PROVIDER);
    let list = registry.versions(id, pkg).await.map_err(provider_fail)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&list)?);
    } else {
        for v in &list {
            apk_fetch::contract::print_message!("•", cyan, "{}  ({})", v.version, v.provider);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn get(
    registry: &ProviderRegistry,
    pkg: &str,
    version: Option<&str>,
    provider: Option<ProviderId>,
    priority: &[ProviderId],
    arch: &str,
    output: &Path,
    fallback: bool,
    json: bool,
) -> Result<(), AppError> {
    let target = if fallback && provider.is_none() {
        let order: Vec<&str> = priority.iter().map(|p| p.as_str()).collect();
        info!("resolving {} (fallback: {})...", pkg, order.join(" -> "));
        registry
            .resolve_with_fallback(pkg, version, arch, priority)
            .await
            .map_err(resolve_err)?
    } else {
        let id = provider
            .or_else(|| priority.first().copied())
            .unwrap_or(DEFAULT_PROVIDER);
        info!("resolving {} via {}...", pkg, id);
        registry
            .download_url(id, pkg, version, arch)
            .await
            .map_err(provider_fail)?
    };

    std::fs::create_dir_all(output).map_err(|e| AppError {
        code: EXIT_NETWORK,
        source: anyhow!("{e}"),
    })?;
    let dest = output.join(&target.filename);

    info!("downloading from {} ({})", target.provider, target.url);
    let fetcher = HttpFetcher::new();
    let pb = ProgressBar::new(0);
    pb.set_style(
        ProgressStyle::with_template("{bar:40} {bytes}/{total_bytes} {bytes_per_sec}")
            .unwrap_or_else(|_| ProgressStyle::default_bar()),
    );
    let saved = fetcher
        .download_to_file(&target.url, &target.headers, &dest, |done, total| {
            if let Some(t) = total {
                pb.set_length(t);
            }
            pb.set_position(done);
        })
        .await
        .map_err(|e| AppError {
            code: provider_error_code(&e),
            source: anyhow!("download failed: {e}"),
        })?;
    pb.finish_and_clear();

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
    let default = ProviderId::DEFAULT_PRIORITY;
    let rank = |id: &ProviderId| default.iter().position(|d| d == id).map(|i| i + 1);
    let names = registry.names();
    if json {
        let rows: Vec<_> = names
            .iter()
            .map(|n| serde_json::json!({ "name": n, "priority": rank(n) }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    for name in &names {
        match rank(name) {
            Some(i) => apk_fetch::contract::print_message!("•", cyan, "{}  (priority {})", name, i),
            None => apk_fetch::contract::print_message!(
                "•",
                cyan,
                "{}  (not in default priority)",
                name
            ),
        }
    }
    Ok(())
}

pub async fn providers_check(
    registry: &ProviderRegistry,
    name: Option<ProviderId>,
    json: bool,
) -> Result<(), AppError> {
    let targets: Vec<ProviderId> = match name {
        Some(n) => vec![n],
        None => registry.names(),
    };
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
                worst = Some(provider_fail(f));
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
    match (name, worst) {
        (Some(_), Some(e)) => Err(e),
        _ => Ok(()),
    }
}
