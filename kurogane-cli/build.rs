//! CEF version of the CLI.
//!
//! The CLI's tanso-sys names it, so the CLI builds the same in Kurogane's
//! workspace and on its own (`cargo install kurogane-cli`).

use tanso_sys::{CEF_VERSION_MAJOR, CEF_VERSION_MINOR, CEF_VERSION_PATCH};

fn main() {
    println!(
        "cargo::rustc-env=KUROGANE_CEF_VERSION={CEF_VERSION_MAJOR}.{CEF_VERSION_MINOR}.{CEF_VERSION_PATCH}"
    );
    println!("cargo::rerun-if-changed=build.rs");
}
