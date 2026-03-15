#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {{
        #[cfg(feature = "debug")]
        {
            println!($($arg)*);
        }

        #[cfg(not(feature = "debug"))]
        {
            let _ = format_args!($($arg)*);
        }
    }};
}
