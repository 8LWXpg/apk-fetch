/// Always uses a format string — a lone `$msg:expr` arm would print `"{f}"` literally.
#[macro_export]
macro_rules! print_message {
    ($symbol:expr, $color:ident, $($arg:tt)*) => {{
        use colored::Colorize;
        println!("{} {}", $symbol.$color().bold(), format!($($arg)*))
    }};
}

/// For an anyhow chain: `error!("{:#}", err)`.
#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {{
        use colored::Colorize;
        eprintln!("{} {}", "×".bright_red().bold(), format!($($arg)*))
    }};
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => { $crate::print_message!("›", cyan, $($arg)*) };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => { $crate::print_message!("▲", yellow, $($arg)*) };
}

#[macro_export]
macro_rules! success {
    ($($arg:tt)*) => { $crate::print_message!("✓", green, $($arg)*) };
}
