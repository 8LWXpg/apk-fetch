//! The command-line surface: what `apk-fetch` accepts.

use std::path::PathBuf;

use clap::builder::styling;
use clap::{Parser, Subcommand};

use crate::common::{Arch, ProviderId};

#[derive(Parser)]
#[command(
    name = "apk-fetch",
    version,
    about = "Download APKs from third-party mirrors",
    styles = get_styles(),
    arg_required_else_help = true
)]
pub(crate) struct Cli {
	/// Machine-readable JSON output.
	#[arg(long, global = true)]
	pub(super) json: bool,

	#[command(subcommand)]
	pub(super) command: Command,
}

#[derive(Subcommand)]
pub(super) enum Command {
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
		/// Providers to query (comma-separated or repeated). Default: just the
		/// top-priority one.
		#[arg(long, value_delimiter = ',', conflicts_with = "all")]
		provider: Vec<ProviderId>,
		/// Search every available provider.
		#[arg(long)]
		all: bool,
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
		/// Preferred ABI (arm64-v8a, armeabi-v7a, x86, x86_64, universal);
		/// providers fall back to a universal build if unavailable.
		#[arg(long, default_value = "arm64-v8a")]
		arch: Arch,
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
pub(super) enum ProvidersCmd {
	/// List configured providers and their priority.
	List,
	/// Check provider reachability.
	Check { name: Option<ProviderId> },
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
