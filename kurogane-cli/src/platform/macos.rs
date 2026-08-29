//! macOS-only development helpers.

use anyhow::Result;
use std::path::Path;
use std::ffi::OsString;

use crate::tui;

/// Links ANGLE libraries into the directories used by `cargo run`.
///
/// Unbundled Chromium processes load ANGLE from the executable directory.
/// Kurogane exposes the CEF-provided libraries there as symlinks.
pub(crate) fn link_unbundled_gpu_libraries(
    cef: &Path,
    cargo_args: &[OsString],
) -> Result<()> {
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

        let profile = match arg.strip_prefix("--profile=") {
            Some(value) => Some(value.to_string()),
            None if arg == "--profile" => args.next().map(|v| v.to_string_lossy().into_owned()),
            None => None,
        };

        if let Some(profile) = profile {
            // Cargo names the `dev` profile directory `debug`
            return if profile == "dev" {
                "debug".into()
            } else {
                profile
            };
        }
    }

    "debug".into()
}
