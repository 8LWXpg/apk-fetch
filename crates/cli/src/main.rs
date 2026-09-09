//! Thin dispatch layer: parse args -> build registry -> hand off to a per-subcommand
//! handler in `commands`. No business logic here.

mod commands;

use std::path::PathBuf;
use std::process::ExitCode;

use apk_fetch_apkcombo::ApkCombo;
use apk_fetch_apkmirror::ApkMirror;
use apk_fetch_apkpure::ApkPure;
use apk_fetch_core::{ProviderRegistry, error};
use apk_fetch_uptodown::Uptodown;
use clap::{Parser, Subcommand};

/// Default provider priority. Also the clap default for `--priority` below (kept as
/// a literal there because clap's derive wants one).
pub const DEFAULT_PRIORITY: &str = "apkmirror,apkpure,apkcombo,uptodown";

// Exit codes (spec: scriptable). clap emits 2 for invalid args on its own.
pub const EXIT_GENERIC: u8 = 1;
pub const EXIT_NOT_FOUND: u8 = 3;
pub const EXIT_BLOCKED: u8 = 4;
pub const EXIT_NETWORK: u8 = 5;

/// CLI error carrying the process exit code to use.
pub struct AppError {
    pub code: u8,
    pub source: anyhow::Error,
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        Self {
            code: EXIT_GENERIC,
            source: e.into(),
        }
    }
}

#[derive(Parser)]
#[command(name = "apk-fetch", version, about = "Download APKs from third-party mirrors")]
struct Cli {
    /// Machine-readable JSON output.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Search for an app by name.
    Search {
        query: String,
        /// Use only this provider.
        #[arg(long)]
        provider: Option<String>,
        #[arg(long, value_delimiter = ',', default_value = "apkmirror,apkpure,apkcombo,uptodown")]
        priority: Vec<String>,
    },
    /// List published versions of a package.
    Versions {
        package_id: String,
        #[arg(long)]
        provider: Option<String>,
    },
    /// Resolve and download an APK.
    Get {
        package_id: String,
        #[arg(long)]
        version: Option<String>,
        /// Use only this provider (overrides --priority / --fallback).
        #[arg(long)]
        provider: Option<String>,
        #[arg(long, value_delimiter = ',', default_value = "apkmirror,apkpure,apkcombo,uptodown")]
        priority: Vec<String>,
        /// Preferred ABI; providers fall back to a universal build if unavailable.
        #[arg(long, default_value = "arm64-v8a")]
        arch: String,
        /// Output directory.
        #[arg(long, default_value = ".")]
        output: PathBuf,
        /// Try providers in priority order until one succeeds.
        #[arg(long)]
        fallback: bool,
    },
    /// Provider management.
    Providers {
        #[command(subcommand)]
        cmd: ProvidersCmd,
    },
}

#[derive(Subcommand)]
enum ProvidersCmd {
    /// List configured providers and their priority.
    List,
    /// Probe provider reachability.
    Check { name: Option<String> },
}

fn build_registry() -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ApkMirror::new()));
    registry.register(Box::new(ApkPure::new()));
    registry.register(Box::new(ApkCombo::new()));
    registry.register(Box::new(Uptodown::new()));
    registry
}

async fn dispatch(cli: Cli) -> Result<(), AppError> {
    let registry = build_registry();
    match cli.command {
        Command::Search {
            query,
            provider,
            priority,
        } => commands::search(&registry, &query, provider.as_deref(), &priority, cli.json).await,
        Command::Versions {
            package_id,
            provider,
        } => commands::versions(&registry, &package_id, provider.as_deref(), cli.json).await,
        Command::Get {
            package_id,
            version,
            provider,
            priority,
            arch,
            output,
            fallback,
        } => {
            commands::get(
                &registry,
                &package_id,
                version.as_deref(),
                provider.as_deref(),
                &priority,
                &arch,
                &output,
                fallback,
                cli.json,
            )
            .await
        }
        Command::Providers { cmd } => match cmd {
            ProvidersCmd::List => commands::providers_list(&registry, cli.json),
            ProvidersCmd::Check { name } => {
                commands::providers_check(&registry, name.as_deref(), cli.json).await
            }
        },
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // Single CLI invocation, all I/O-bound (curl subprocesses, sequential
    // provider calls) — a current-thread runtime is plenty.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    match rt.block_on(dispatch(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!(e.source);
            ExitCode::from(e.code)
        }
    }
}
