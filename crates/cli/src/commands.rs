//! One handler per subcommand. All business logic lives in `core` / `fetch` /
//! `providers`; these just orchestrate calls and render output.

use std::path::Path;

use anyhow::anyhow;
use apk_fetch_core::{
    ProviderError, ProviderRegistry, ResolveError, error, info, success, warn,
};
use apk_fetch_fetch::HttpFetcher;
use indicatif::{ProgressBar, ProgressStyle};

use crate::{AppError, DEFAULT_PRIORITY, EXIT_BLOCKED, EXIT_NETWORK, EXIT_NOT_FOUND};

fn provider_error_code(e: &ProviderError) -> u8 {
    match e {
        ProviderError::NotFound => EXIT_NOT_FOUND,
        ProviderError::Blocked { .. } | ProviderError::RateLimited => EXIT_BLOCKED,
        ProviderError::Network(_) => EXIT_NETWORK,
        ProviderError::ParseError(_) => crate::EXIT_GENERIC,
    }
}

fn provider_err(name: &str, e: ProviderError) -> AppError {
    AppError {
        code: provider_error_code(&e),
        source: anyhow!("{name}: {e}"),
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

fn order<'a>(provider: Option<&'a str>, priority: &'a [String]) -> Vec<&'a str> {
    match provider {
        Some(p) => vec![p],
        None => priority.iter().map(String::as_str).collect(),
    }
}

pub async fn search(
    registry: &ProviderRegistry,
    query: &str,
    provider: Option<&str>,
    priority: &[String],
    json: bool,
) -> Result<(), AppError> {
    let mut last: Option<AppError> = None;
    for name in order(provider, priority) {
        let Some(p) = registry.get(name) else { continue };
        info!("searching {}...", name);
        match p.search(query).await {
            Ok(results) if results.is_empty() => continue,
            Ok(results) => {
                if json {
                    println!("{}", serde_json::to_string_pretty(&results)?);
                } else {
                    for r in &results {
                        apk_fetch_core::print_message!(
                            "→", cyan, "{}  {}  ({})", r.package, r.title, r.provider
                        );
                    }
                }
                return Ok(());
            }
            Err(e) => {
                warn!("{}: {}", name, e);
                last = Some(provider_err(name, e));
            }
        }
    }
    Err(last.unwrap_or(AppError {
        code: EXIT_NOT_FOUND,
        source: anyhow!("no results for '{query}'"),
    }))
}

pub async fn versions(
    registry: &ProviderRegistry,
    pkg: &str,
    provider: Option<&str>,
    json: bool,
) -> Result<(), AppError> {
    let name = provider.unwrap_or_else(|| DEFAULT_PRIORITY.split(',').next().unwrap());
    let p = registry
        .get(name)
        .ok_or_else(|| AppError::from(anyhow!("unknown provider '{name}'")))?;
    let list = p.versions(pkg).await.map_err(|e| provider_err(name, e))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&list)?);
    } else {
        for v in &list {
            apk_fetch_core::print_message!("→", cyan, "{}  ({})", v.version, v.provider);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn get(
    registry: &ProviderRegistry,
    pkg: &str,
    version: Option<&str>,
    provider: Option<&str>,
    priority: &[String],
    output: &Path,
    fallback: bool,
    json: bool,
) -> Result<(), AppError> {
    let target = if let Some(name) = provider {
        let p = registry
            .get(name)
            .ok_or_else(|| AppError::from(anyhow!("unknown provider '{name}'")))?;
        info!("resolving {} via {}...", pkg, name);
        p.download_url(pkg, version)
            .await
            .map_err(|e| provider_err(name, e))?
    } else if fallback {
        let ord = order(None, priority);
        info!("resolving {} (fallback: {})...", pkg, ord.join(" -> "));
        registry
            .resolve_with_fallback(pkg, version, &ord)
            .await
            .map_err(resolve_err)?
    } else {
        let name = priority.first().map(String::as_str).unwrap_or("apkmirror");
        let p = registry
            .get(name)
            .ok_or_else(|| AppError::from(anyhow!("unknown provider '{name}'")))?;
        info!("resolving {} via {}...", pkg, name);
        p.download_url(pkg, version)
            .await
            .map_err(|e| provider_err(name, e))?
    };

    std::fs::create_dir_all(output)
        .map_err(|e| AppError { code: EXIT_NETWORK, source: anyhow!("{e}") })?;
    let dest = output.join(&target.filename);

    info!("downloading from {} ({})", target.provider, target.url);
    let fetcher = HttpFetcher::new();
    let pb = ProgressBar::new(0);
    pb.set_style(
        ProgressStyle::with_template("{bar:40} {bytes}/{total_bytes} {bytes_per_sec}")
            .unwrap_or_else(|_| ProgressStyle::default_bar()),
    );
    fetcher
        .download_to_file(&target.url, &target.headers, &dest, |done, total| {
            if let Some(t) = total {
                pb.set_length(t);
            }
            pb.set_position(done);
        })
        .await
        .map_err(|e| AppError { code: provider_error_code(&e), source: anyhow!("download failed: {e}") })?;
    pb.finish_and_clear();

    if json {
        println!(
            "{}",
            serde_json::json!({
                "path": dest.display().to_string(),
                "provider": target.provider,
                "version": target.version,
            })
        );
    } else {
        success!("saved {}", dest.display());
    }
    Ok(())
}

pub fn providers_list(registry: &ProviderRegistry, json: bool) -> Result<(), AppError> {
    let default: Vec<&str> = DEFAULT_PRIORITY.split(',').collect();
    let names = registry.names();
    if json {
        let rows: Vec<_> = names
            .iter()
            .map(|n| {
                serde_json::json!({
                    "name": n,
                    "priority": default.iter().position(|d| d == n).map(|i| i + 1),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    for name in &names {
        match default.iter().position(|d| d == name) {
            Some(i) => apk_fetch_core::print_message!("→", cyan, "{}  (priority {})", name, i + 1),
            None => apk_fetch_core::print_message!("→", cyan, "{}  (not in default priority)", name),
        }
    }
    Ok(())
}

pub async fn providers_check(
    registry: &ProviderRegistry,
    name: Option<&str>,
    json: bool,
) -> Result<(), AppError> {
    let targets: Vec<&str> = match name {
        Some(n) => vec![n],
        None => registry.names(),
    };
    let mut results = Vec::new();
    let mut worst: Option<AppError> = None;
    for n in targets {
        let Some(p) = registry.get(n) else {
            return Err(AppError::from(anyhow!("unknown provider '{n}'")));
        };
        match p.check().await {
            Ok(()) => {
                results.push((n, "ok".to_string()));
                if !json {
                    success!("{}: ok", n);
                }
            }
            Err(e) => {
                results.push((n, format!("{e}")));
                if !json {
                    error!("{}: {}", n, e);
                }
                worst = Some(provider_err(n, e));
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
