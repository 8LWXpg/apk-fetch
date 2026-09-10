mod commands;

use std::path::PathBuf;
use std::process::ExitCode;

use apk_fetch::contract::{ProviderId, ProviderRegistry, error};
use apk_fetch::providers::{apkcombo::ApkCombo, apkmirror::ApkMirror, apkpure::ApkPure};
use clap::builder::styling;
use clap::{Parser, Subcommand};

pub const EXIT_GENERIC: u8 = 1;
pub const EXIT_NOT_FOUND: u8 = 3;
pub const EXIT_BLOCKED: u8 = 4;
pub const EXIT_NETWORK: u8 = 5;
/// Ctrl+C, by the usual `128 + SIGINT` convention.
pub const EXIT_CANCELLED: u8 = 130;

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
#[command(
    name = "apk-fetch",
    version,
    about = "Download APKs from third-party mirrors",
    styles = get_styles(),
    arg_required_else_help = true
)]
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
        /// Providers to search (comma-separated or repeated). Default: just the
        /// top-priority one.
        #[arg(long, value_delimiter = ',', conflicts_with = "all")]
        provider: Vec<ProviderId>,
        /// Search every available provider.
        #[arg(long)]
        all: bool,
    },
    /// List published versions of a package.
    Versions {
        package_id: String,
        #[arg(long)]
        provider: Option<ProviderId>,
    },
    /// Resolve and download an APK.
    Get {
        package_id: String,
        #[arg(long)]
        version: Option<String>,
        /// Use only this provider (skips the priority-ordered fallback).
        #[arg(long)]
        provider: Option<ProviderId>,
        /// Providers to try, in order, until one resolves (comma-separated or repeated).
        #[arg(long, value_delimiter = ',', default_values_t = ProviderId::DEFAULT_PRIORITY.to_vec())]
        priority: Vec<ProviderId>,
        /// Preferred ABI; providers fall back to a universal build if unavailable.
        #[arg(long, default_value = "arm64-v8a")]
        arch: String,
        /// Output directory.
        #[arg(long, default_value = ".")]
        output: PathBuf,
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
    Check { name: Option<ProviderId> },
}

fn build_registry() -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ApkMirror::new()));
    registry.register(Box::new(ApkPure::new()));
    registry.register(Box::new(ApkCombo::new()));
    registry
}

async fn dispatch(cli: Cli) -> Result<(), AppError> {
    let registry = build_registry();
    match cli.command {
        Command::Search {
            query,
            provider,
            all,
        } => commands::search(&registry, &query, &provider, all, cli.json).await,
        Command::Versions {
            package_id,
            provider,
        } => commands::versions(&registry, &package_id, provider, cli.json).await,
        Command::Get {
            package_id,
            version,
            provider,
            priority,
            arch,
            output,
        } => {
            commands::get(
                &registry,
                &package_id,
                version.as_deref(),
                provider,
                &priority,
                &arch,
                &output,
                cli.json,
            )
            .await
        }
        Command::Providers { cmd } => match cmd {
            ProvidersCmd::List => commands::providers_list(&registry, cli.json),
            ProvidersCmd::Check { name } => {
                commands::providers_check(&registry, name, cli.json).await
            }
        },
    }
}

fn get_styles() -> clap::builder::Styles {
    clap::builder::Styles::default()
        .usage(styling::AnsiColor::BrightGreen.on_default())
        .header(styling::AnsiColor::BrightGreen.on_default())
        .literal(styling::AnsiColor::BrightCyan.on_default())
        .invalid(styling::AnsiColor::BrightYellow.on_default())
        .error(styling::AnsiColor::BrightRed.on_default().bold())
        .valid(styling::AnsiColor::BrightGreen.on_default())
        .placeholder(styling::AnsiColor::Cyan.on_default())
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
            error!("{:#}", e.source);
            ExitCode::from(e.code)
        }
    }
}
