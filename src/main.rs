mod cli;
mod common;
mod providers;

use std::process::ExitCode;

use clap::Parser;

use crate::{
	cli::{Cli, dispatch},
	common::fetch::install_ctrlc,
	common::ui::error,
};

fn main() -> ExitCode {
	install_ctrlc();
	let cli = Cli::parse();

	match dispatch(cli) {
		Ok(()) => ExitCode::SUCCESS,
		Err(e) => {
			error!("{:#}", e.source);
			ExitCode::from(e.code)
		}
	}
}
