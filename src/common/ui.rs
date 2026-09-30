use std::sync::atomic::{AtomicBool, Ordering};

/// Whether `--json` was passed.
static JSON: AtomicBool = AtomicBool::new(false);

pub fn set_json(on: bool) {
	JSON.store(on, Ordering::Relaxed);
}

/// True under `--json`.
#[inline]
pub fn json() -> bool {
	JSON.load(Ordering::Relaxed)
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

/// For an anyhow chain: `error!("{:#}", err)`.
macro_rules! error {
    ($($arg:tt)*) => {{
        use colored::Colorize;
        eprintln!("{} {}", "×".bright_red().bold(), format!($($arg)*))
    }};
}
pub(crate) use error;

macro_rules! info {
    ($($arg:tt)*) => { $crate::common::ui::print_message!("›", cyan, $($arg)*) };
}
pub(crate) use info;

macro_rules! warning {
    ($($arg:tt)*) => { $crate::common::ui::print_message!("▲", yellow, $($arg)*) };
}
pub(crate) use warning;

macro_rules! success {
    ($($arg:tt)*) => { $crate::common::ui::print_message!("✓", green, $($arg)*) };
}
pub(crate) use success;
