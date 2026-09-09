//! Thin dispatch layer: parse args -> build registry -> hand off to a per-subcommand
//! handler in `commands`. No business logic here.

mod commands;

use std::path::PathBuf;
use std::process::ExitCode;

use apk_fetch::contract::{ProviderRegistry, error};
use apk_fetch::providers::{
    apkcombo::ApkCombo, apkmirror::ApkMirror, apkpure::ApkPure, uptodown::Uptodown,
};
use clap::builder::styling;
use clap::{Parser, Subcommand, ValueEnum};

/// The known providers, as a CLI-parseable enum. Variant names lower-case to the
/// registry names via clap's default kebab rename (all single words).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Provider {
    Apkmirror,
    Apkpure,
    Apkcombo,
    Uptodown,
}

impl Provider {
    fn as_str(self) -> &'static str {
        match self {
            Provider::Apkmirror => "apkmirror",
            Provider::Apkpure => "apkpure",
            Provider::Apkcombo => "apkcombo",
            Provider::Uptodown => "uptodown",
        }
    }
}

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
        /// Use only this provider.
        #[arg(long)]
        provider: Option<Provider>,
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "apkmirror,apkpure,apkcombo,uptodown"
        )]
        priority: Vec<Provider>,
    },
    /// List published versions of a package.
    Versions {
        package_id: String,
        #[arg(long)]
        provider: Option<Provider>,
    },
    /// Resolve and download an APK.
    Get {
        package_id: String,
        #[arg(long)]
        version: Option<String>,
        /// Use only this provider (overrides --priority / --fallback).
        #[arg(long)]
        provider: Option<Provider>,
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "apkmirror,apkpure,apkcombo,uptodown"
        )]
        priority: Vec<Provider>,
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
    Check { name: Option<Provider> },
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
        } => {
            let priority = names(&priority);
            commands::search(
                &registry,
                &query,
                provider.map(Provider::as_str),
                &priority,
                cli.json,
            )
            .await
        }
        Command::Versions {
            package_id,
            provider,
        } => {
            commands::versions(
                &registry,
                &package_id,
                provider.map(Provider::as_str),
                cli.json,
            )
            .await
        }
        Command::Get {
            package_id,
            version,
            provider,
            priority,
            arch,
            output,
            fallback,
        } => {
            let priority = names(&priority);
            commands::get(
                &registry,
                &package_id,
                version.as_deref(),
                provider.map(Provider::as_str),
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
                commands::providers_check(&registry, name.map(Provider::as_str), cli.json).await
            }
        },
    }
}

/// Provider enums -> registry name strings, in order.
fn names(providers: &[Provider]) -> Vec<String> {
    providers.iter().map(|p| p.as_str().to_string()).collect()
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
            error!(e.source);
            ExitCode::from(e.code)
        }
    }
}
