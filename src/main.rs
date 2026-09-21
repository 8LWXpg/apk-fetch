mod cli;
mod common;
mod providers;

use std::process::ExitCode;

use clap::Parser;

use crate::{
	cli::{Cli, dispatch},
	common::ui::error,
};

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
