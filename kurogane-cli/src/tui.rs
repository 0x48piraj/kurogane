//! Styled command-line output helpers.
//!
//! Each kind of message has its own prefix and color.

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

/// Prints a warning message to stderr.
pub fn warn(msg: &str) {
    eprintln!("{} {}", "[!]".yellow().bold(), msg);
}

/// Prints a blank line for spacing.
pub fn blank() {
    println!();
}

/// Prints an informational message.
pub fn info(msg: &str) {
    println!("{} {}", "[~]".dimmed(), msg);
}

/// Prints a progress step of a multi-step operation.
pub fn step(msg: &str) {
    println!("{} {}", "[*]".cyan().bold(), msg);
}

/// Prints an indented key-value pair for hierarchical output.
pub fn field(key: &str, value: impl Display) {
    println!("    {}: {}", key.dimmed(), value);
}

/// Prints an error as a `reason` field and each of its causes as a `cause`
/// field.
pub fn error_fields(error: &dyn std::error::Error) {
    field("reason", error);

    let mut cause = error.source();
    while let Some(current) = cause {
        field("cause", current);
        cause = current.source();
    }
}

/// Prints a section header between blank lines.
pub fn section(title: &str) {
    println!("\n{}\n", title.bold());
}

/// Prompts until the answer is yes or no; empty input means no.
pub fn confirm(question: &str) -> std::io::Result<bool> {
    loop {
        print!("\n{question} [y/N]: ");
        std::io::Write::flush(&mut std::io::stdout())?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;

        match input.trim() {
            "y" | "Y" | "yes" | "Yes" | "YES" => return Ok(true),
            "n" | "N" | "no" | "No" | "NO" | "" => return Ok(false),
            _ => warn("Please enter y or n"),
        }
    }
}

/// Formats a path for display with forward slashes on every platform.
pub fn format_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}
