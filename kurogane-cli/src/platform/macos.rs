//! macOS-only development helpers.

use anyhow::Result;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use crate::tui;

/// Configures a macOS `cargo run` process to discover the CEF runtime and
/// links the unbundled GPU libraries into the target directory.
pub(crate) fn set_env(cmd: &mut Command, cef: &Path, cargo_args: &[OsString]) -> Result<()> {
    let existing = std::env::var("DYLD_FALLBACK_LIBRARY_PATH").unwrap_or_default();
    cmd.env(
        "DYLD_FALLBACK_LIBRARY_PATH",
        super::prepend_search_path(cef, &existing, ':'),
    );

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

    while let Some(arg) = args.next() {
        let arg = arg.to_string_lossy();

        if arg == "--release" {
            return "release".into();
        }

        if let Some(value) = arg.strip_prefix("--profile=") {
            return if value == "dev" {
                "debug".into()
            } else {
                value.into()
            };
        }

        if arg == "--profile" {
            return match args.next().map(|value| value.to_string_lossy()) {
                Some(profile) if profile == "dev" => "debug".into(),
                Some(profile) => profile.into_owned(),
                None => "debug".into(),
            };
        }
    }

    "debug".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn defaults_to_debug() {
        assert_eq!(profile_dir_name(&args(&[])), "debug");
    }

    #[test]
    fn non_profile_args_are_ignored() {
        assert_eq!(profile_dir_name(&args(&["--example", "foo"])), "debug");
    }

    #[test]
    fn release_flag_wins() {
        assert_eq!(profile_dir_name(&args(&["--release"])), "release");
    }

    #[test]
    fn release_after_other_args() {
        assert_eq!(
            profile_dir_name(&args(&["--example", "foo", "--release"])),
            "release"
        );
    }

    #[test]
    fn profile_equals_syntax() {
        assert_eq!(profile_dir_name(&args(&["--profile=custom"])), "custom");
    }

    #[test]
    fn profile_space_syntax() {
        assert_eq!(profile_dir_name(&args(&["--profile", "custom"])), "custom");
    }

    #[test]
    fn dev_profile_maps_to_debug() {
        assert_eq!(profile_dir_name(&args(&["--profile", "dev"])), "debug");
    }

    #[test]
    fn bare_profile_with_missing_value_defaults() {
        assert_eq!(profile_dir_name(&args(&["--profile"])), "debug");
    }
}
