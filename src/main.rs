mod cli;
mod common;
mod providers;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, dispatch};
use crate::common::fetch::install_ctrlc;
use crate::common::ui::{error, global_set};

fn main() -> ExitCode {
	install_ctrlc();
	let cli = Cli::parse();
	global_set(cli.json, cli.no_progress);

	match dispatch(cli) {
		Ok(()) => ExitCode::SUCCESS,
		Err(e) => {
			error!("{:#}", e.source);
			ExitCode::from(e.code)
		}
	}
}
