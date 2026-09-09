//! Shared user-facing output macros. All `#[macro_export]` so they land at the
//! crate root: `provider::info!`, etc.

/// General base: `print_message!("·", cyan, "trying {}", name)`.
#[macro_export]
macro_rules! print_message {
    ($symbol:expr, $color:ident, $msg:expr) => {{
        use colored::Colorize;
        println!("{} {}", $symbol.$color().bold(), $msg)
    }};
    ($symbol:expr, $color:ident, $fmt:expr, $($arg:tt)*) => {{
        use colored::Colorize;
        println!("{} {}", $symbol.$color().bold(), format!($fmt, $($arg)*))
    }};
}

/// `error!(err)` prints the full `{:#}` anyhow chain; `error!("fmt", args)` for plain.
#[macro_export]
macro_rules! error {
    ($msg:expr) => {{
        use colored::Colorize;
        eprintln!("{} {:#}", "error:".bright_red().bold(), $msg)
    }};
    ($fmt:expr, $($arg:tt)*) => {{
        use colored::Colorize;
        eprintln!("{} {}", "error:".bright_red().bold(), format!($fmt, $($arg)*))
    }};
}

/// Resolver progress: "trying apkmirror...".
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => { $crate::print_message!("·", cyan, $($arg)*) };
}

/// Fallback transitions: "apkmirror blocked, falling back...".
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => { $crate::print_message!("!", yellow, $($arg)*) };
}

/// Completed download.
#[macro_export]
macro_rules! success {
    ($($arg:tt)*) => { $crate::print_message!("✓", green, $($arg)*) };
}
