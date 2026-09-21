/// Always uses a format string — a lone `$msg:expr` arm would print `"{f}"` literally.
macro_rules! print_message {
    ($symbol:expr, $color:ident, $($arg:tt)*) => {{
        use colored::Colorize;
        println!("{} {}", $symbol.$color().bold(), format!($($arg)*))
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
