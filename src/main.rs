mod cli;
mod common;
mod providers;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, dispatch};
use crate::common::fetch::install_ctrlc;
use crate::common::ui::{error, set_json};

fn main() -> ExitCode {
	install_ctrlc();
	let cli = Cli::parse();
	set_json(cli.json);

	match dispatch(cli) {
		Ok(()) => ExitCode::SUCCESS,
		Err(e) => {
			error!("{:#}", e.source);
			ExitCode::from(e.code)
		}
	}
}
