//! Turning parsed arguments into a provider registry and a command call.

use crate::cli::args::{Cli, Command, ProvidersCmd};
use crate::cli::commands;
use crate::cli::exit::AppError;
use crate::common::{Provider, ProviderId, ProviderRegistry};
use crate::providers::{ApkCombo, ApkMirror, ApkPure};

pub fn dispatch(cli: Cli) -> Result<(), AppError> {
	let registry = build_registry(&cli.command);

	match cli.command {
		Command::Search { query, .. } => commands::search(&registry, &query),
		Command::Versions { package_id, .. } => commands::versions(&registry, &package_id),
		Command::Get {
			package_id,
			version,
			arch,
			out_dir,
			..
		} => commands::get(&registry, &package_id, version.as_deref(), arch, &out_dir),
		Command::Providers { cmd } => match cmd {
			ProvidersCmd::List => commands::providers_list(&registry),
			ProvidersCmd::Check { .. } => commands::providers_check(&registry),
		},
	}
}

/// Build [`ProviderRegistry`] in struct `impl` would cause circular import.
fn build_registry(cmd: &Command) -> ProviderRegistry {
	let all = || ProviderId::DEFAULT_PRIORITY.to_vec();
	let top = || vec![ProviderId::DEFAULT_PRIORITY[0]];
	let order = match cmd {
		Command::Search { all: true, .. } => all(),
		Command::Search { provider, .. } if provider.is_empty() => top(),
		Command::Search { provider, .. } => provider.clone(),
		Command::Versions { all: true, .. } => all(),
		Command::Versions { provider, .. } if provider.is_empty() => top(),
		Command::Versions { provider, .. } => provider.clone(),
		Command::Get { provider: Some(p), .. } => vec![*p],
		Command::Get { priority, .. } if !priority.is_empty() => priority.clone(),
		Command::Providers {
			cmd: ProvidersCmd::Check { name: Some(n) },
		} => vec![*n],
		_ => all(),
	};

	order
		.iter()
		.copied()
		.map(|id| -> Box<dyn Provider> {
			match id {
				ProviderId::Apkcombo => Box::new(ApkCombo::default()),
				ProviderId::Apkpure => Box::new(ApkPure::default()),
				ProviderId::Apkmirror => Box::new(ApkMirror::default()),
			}
		})
		.collect::<ProviderRegistry>()
}
