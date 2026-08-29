//! macOS-only development helpers.

use anyhow::Result;
use std::borrow::Cow;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use crate::tui;

/// Configures a macOS `cargo run` process to discover the CEF runtime and
/// links the unbundled GPU libraries into the target directory.
pub(crate) fn set_env(cmd: &mut Command, cef: &Path, cargo_args: &[OsString]) -> Result<()> {
    let mut dyld = std::env::var("DYLD_FALLBACK_LIBRARY_PATH").unwrap_or_default();
    dyld = format!("{}:{}", cef.display(), dyld);
    cmd.env("DYLD_FALLBACK_LIBRARY_PATH", dyld);

    link_unbundled_gpu_libraries(cef, cargo_args)
}

/// Links ANGLE libraries into the directories used by `cargo run`.
///
/// Unbundled Chromium processes load ANGLE from the executable directory.
/// Kurogane exposes the CEF-provided libraries there as symlinks.
fn link_unbundled_gpu_libraries(cef: &Path, cargo_args: &[OsString]) -> Result<()> {
    use cargo_metadata::MetadataCommand;
    use kurogane_layout::link_unbundled_angle_libraries;

    let metadata = MetadataCommand::new().no_deps().exec()?;
    let profile = metadata.target_directory.join(profile_dir_name(cargo_args));

    // `cargo run` uses the profile root for binaries and `examples/` for examples
    let targets = [profile.clone(), profile.join("examples")];

    let mut installed = Vec::new();

    for dir in &targets {
        for name in link_unbundled_angle_libraries(cef, dir.as_std_path())? {
            if !installed.contains(&name) {
                installed.push(name);
            }
        }
    }

    if !installed.is_empty() {
        tui::step("Linking GPU libraries");
        tui::field("libraries", installed.join(", "));
    }

    Ok(())
}

/// Returns the target directory for the selected Cargo profile.
fn profile_dir_name(cargo_args: &[OsString]) -> String {
    let mut args = cargo_args.iter();
    let mut profile: Option<Cow<str>> = None;

    while let Some(arg) = args.next() {
        let arg = arg.to_string_lossy();

        if arg == "--release" {
            return "release".into();
        }

        if let Some(value) = arg.strip_prefix("--profile=") {
            profile = Some(Cow::Borrowed(value));
            break;
        }

        if arg == "--profile" {
            if let Some(value) = args.next() {
                profile = Some(value.to_string_lossy());
            }
            break;
        }
    }

    match profile.map(Cow::into_owned) {
        // Cargo names the `dev` profile directory `debug`
        Some("dev") | None => "debug".into(),
        Some(other) => other,
    }
}
