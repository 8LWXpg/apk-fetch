mod commands;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::builder::styling;
use clap::{Parser, Subcommand};

use crate::common::{
	Provider, ProviderError, ProviderFailure, ProviderId, ProviderRegistry, ResolveError,
};
use crate::error;
use crate::providers::{ApkCombo, ApkMirror, ApkPure};

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

impl From<serde_json::Error> for AppError {
	fn from(e: serde_json::Error) -> Self {
		Self {
			code: EXIT_GENERIC,
			source: e.into(),
		}
	}
}

/// Exit code for one provider's failure.
fn provider_error_code(e: &ProviderError) -> u8 {
	match e {
		ProviderError::NotFound(_) => EXIT_NOT_FOUND,
		ProviderError::Blocked => EXIT_BLOCKED,
		ProviderError::Network(_) => EXIT_NETWORK,
		ProviderError::Cancelled => EXIT_CANCELLED,
		ProviderError::ParseError(_) => EXIT_GENERIC,
	}
}

impl From<ProviderFailure> for AppError {
	fn from(f: ProviderFailure) -> Self {
		Self {
			code: provider_error_code(&f.source),
			source: anyhow::anyhow!("{f}"),
		}
	}
}

impl From<ResolveError> for AppError {
	fn from(e: ResolveError) -> Self {
		// If every provider failed the same way, that's the reason; otherwise a
		// network failure is the one the user can act on.
		let codes: Vec<u8> = e
			.attempts
			.iter()
			.map(|f| provider_error_code(&f.source))
			.collect();
		let code = match codes.first() {
			Some(&c) if codes.iter().all(|&x| x == c) => c,
			_ if codes.contains(&EXIT_NETWORK) => EXIT_NETWORK,
			_ => EXIT_GENERIC,
		};
		Self {
			code,
			source: anyhow::anyhow!("{e}"),
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
		#[arg(long, conflicts_with = "priority")]
		provider: Option<ProviderId>,
		/// Providers to try, in order, until one resolves (comma-separated or
		/// repeated). Default: the built-in priority order.
		#[arg(long, value_delimiter = ',')]
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
	/// Check provider reachability.
	Check { name: Option<ProviderId> },
}

/// Build [`ProviderRegistry`] in struct `impl` would cause circular import.
fn build_registry(order: &[ProviderId]) -> ProviderRegistry {
	order
		.iter()
		.map(|id| -> Box<dyn Provider> {
			match id {
				ProviderId::Apkmirror => Box::new(ApkMirror::new()),
				ProviderId::Apkpure => Box::new(ApkPure::new()),
				ProviderId::Apkcombo => Box::new(ApkCombo::new()),
			}
		})
		.collect::<Vec<_>>()
		.into()
}

/// Which providers this invocation may use, in order.
fn selection(cmd: &Command) -> Vec<ProviderId> {
	let all = || ProviderId::DEFAULT_PRIORITY.to_vec();
	let top = || vec![ProviderId::DEFAULT_PRIORITY[0]];
	match cmd {
		Command::Search { all: true, .. } => all(),
		Command::Search { provider, .. } if provider.is_empty() => top(),
		Command::Search { provider, .. } => provider.clone(),
		Command::Versions { provider, .. } => provider.map_or_else(top, |p| vec![p]),
		Command::Get {
			provider: Some(p), ..
		} => vec![*p],
		Command::Get { priority, .. } if !priority.is_empty() => priority.clone(),
		Command::Providers {
			cmd: ProvidersCmd::Check { name: Some(n) },
		} => vec![*n],
		_ => all(),
	}
}

async fn dispatch(cli: Cli) -> Result<(), AppError> {
	let registry = build_registry(&selection(&cli.command));

	match cli.command {
		Command::Search { query, .. } => commands::search(&registry, &query, cli.json).await,
		Command::Versions { package_id, .. } => {
			commands::versions(&registry, &package_id, cli.json).await
		}
		Command::Get {
			package_id,
			version,
			arch,
			output,
			..
		} => {
			commands::get(
				&registry,
				&package_id,
				version.as_deref(),
				&arch,
				&output,
				cli.json,
			)
			.await
		}
		Command::Providers { cmd } => match cmd {
			ProvidersCmd::List => commands::providers_list(&registry, cli.json),
			ProvidersCmd::Check { .. } => commands::providers_check(&registry, cli.json).await,
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

pub fn run() -> ExitCode {
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
