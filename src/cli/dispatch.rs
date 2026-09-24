//! Turning parsed arguments into a provider registry and a command call.

use crate::cli::args::{Cli, Command, ProvidersCmd};
use crate::cli::commands;
use crate::cli::exit::AppError;
use crate::common::{Provider, ProviderId, ProviderRegistry};
use crate::providers::{ApkCombo, ApkMirror, ApkPure};

pub fn dispatch(cli: Cli) -> Result<(), AppError> {
	let registry = build_registry(&selection(&cli.command));

	match cli.command {
		Command::Search { query, .. } => commands::search(&registry, &query, cli.json),
		Command::Versions { package_id, .. } => {
			commands::versions(&registry, &package_id, cli.json)
		}
		Command::Get {
			package_id,
			version,
			arch,
			output,
			..
		} => commands::get(
			&registry,
			&package_id,
			version.as_deref(),
			arch,
			&output,
			cli.json,
		),
		Command::Providers { cmd } => match cmd {
			ProvidersCmd::List => commands::providers_list(&registry, cli.json),
			ProvidersCmd::Check { .. } => commands::providers_check(&registry, cli.json),
		},
	}
}

/// Which providers this invocation may use, in order.
fn selection(cmd: &Command) -> Vec<ProviderId> {
	let all = || ProviderId::DEFAULT_PRIORITY.to_vec();
	let top = || vec![ProviderId::DEFAULT_PRIORITY[0]];
	match cmd {
		Command::Search { all: true, .. } => all(),
		Command::Search { provider, .. } if provider.is_empty() => top(),
		Command::Search { provider, .. } => provider.clone(),
		Command::Versions { all: true, .. } => all(),
		Command::Versions { provider, .. } if provider.is_empty() => top(),
		Command::Versions { provider, .. } => provider.clone(),
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

impl From<&ProviderId> for Box<dyn Provider> {
	fn from(id: &ProviderId) -> Self {
		match id {
			ProviderId::Apkmirror => Box::new(ApkMirror::default()),
			ProviderId::Apkpure => Box::new(ApkPure::default()),
			ProviderId::Apkcombo => Box::new(ApkCombo::default()),
		}
	}
}

/// Build [`ProviderRegistry`] in struct `impl` would cause circular import.
fn build_registry(order: &[ProviderId]) -> ProviderRegistry {
	order
		.iter()
		.map(<Box<dyn Provider>>::from)
		.collect::<Vec<_>>()
		.into()
}
