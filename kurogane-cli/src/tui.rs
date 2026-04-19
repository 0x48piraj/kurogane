//! Lightweight helpers for styled CLI output.
//!
//! Provides consistent prefixes and coloring for common message types
//! like success, error, warnings and structured sections.

use colored::*;
use std::path::Path;
use std::fmt::Display;

/// Prints a success message.
pub fn success(msg: &str) {
    println!("{} {}", "[+]".green().bold(), msg);
}

/// Prints an error message to stderr.
pub fn error(msg: &str) {
    eprintln!("{} {}", "[-]".red().bold(), msg);
}

/// Prints a warning message.
pub fn warn(msg: &str) {
    println!("{} {}", "[!]".yellow().bold(), msg);
}

/// Prints an informational message
pub fn info(msg: &str) {
    println!("{} {}", "[~]".dimmed(), msg);
}

/// Prints a step indicator.
/// Useful for showing progress in multi-step operations.
pub fn step(msg: &str) {
    println!("{} {}", "[*]".cyan().bold(), msg);
}

/// Prints an indented key-value pair for hierarchical output.
pub fn field(key: &str, value: impl Display) {
    println!("    {}: {}", key.dimmed(), value);
}

/// Prints a section header with surrounding spacing for readability.
pub fn section(title: &str) {
    println!("\n{}\n", title.bold());
}

/// Formats a path for display ensuring consistent output across platforms.
pub fn format_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}
