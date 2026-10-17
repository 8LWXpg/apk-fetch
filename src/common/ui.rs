use std::sync::atomic::{AtomicBool, Ordering};

/// `--json` option.
static JSON: AtomicBool = AtomicBool::new(false);
/// `--quite` option.
static NO_PROGRESS: AtomicBool = AtomicBool::new(false);

pub fn global_set(json: bool, quiet: bool) {
	JSON.store(json, Ordering::Relaxed);
	NO_PROGRESS.store(quiet, Ordering::Relaxed);
}

/// `--json` value.
#[inline]
pub fn json() -> bool {
	JSON.load(Ordering::Relaxed)
}

/// `--no-progress` value.
#[inline]
pub fn no_progress() -> bool {
	NO_PROGRESS.load(Ordering::Relaxed)
}

/// `println!` with custom color and symbol, does nothing if [`json`] is `true`.
macro_rules! print_message {
	($symbol:expr, $color:ident, $($arg:tt)*) => {{
		if !$crate::common::ui::json() {
			use colored::Colorize;
			println!("{} {}", $symbol.$color().bold(), format!($($arg)*))
		}
	}};
}
pub(crate) use print_message;

macro_rules! eprint_message {
	($symbol:expr, $color:ident, $($arg:tt)*) => {{
		use colored::Colorize;
		eprintln!("{} {}", $symbol.$color().bold(), format!($($arg)*))
	}};
}
pub(crate) use eprint_message;

macro_rules! info {
	($($arg:tt)*) => { $crate::common::ui::print_message!("›", cyan, $($arg)*) };
}
pub(crate) use info;

macro_rules! success {
	($($arg:tt)*) => { $crate::common::ui::print_message!("✓", green, $($arg)*) };
}
pub(crate) use success;

macro_rules! warning {
	($($arg:tt)*) => { $crate::common::ui::eprint_message!("▲", yellow, $($arg)*) };
}
pub(crate) use warning;

/// For an anyhow chain: `error!("{:#}", err)`.
macro_rules! error {
	($($arg:tt)*) => { $crate::common::ui::eprint_message!("×", bright_red, $($arg)*) };
}
pub(crate) use error;
