//! CEF's framework linked into a macOS bundle's executables at launch.
//!
//! dyld fixes up an image it maps at launch through page-in linking, so the
//! framework's `__DATA_CONST` stays clean and shared between processes. A
//! framework opened later with dlopen is fixed up in every process, about
//! 12 MB each. The application's binary weakly links the framework at the path
//! the main executable finds it at. The helpers run a copy that finds it from
//! their own bundles. Each executable reaches only its bundle's framework.
//! The runtime still loads CEF through tanso, which takes the image dyld
//! mapped and checks it is the CEF the bindings were generated for.

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use tempfile::NamedTempFile;

/// The framework's name, as the linker's `-weak_framework` takes it.
const FRAMEWORK: &str = "Chromium Embedded Framework";

/// The framework's binary, as a load command names it.
const FRAMEWORK_BINARY: &str = "Chromium Embedded Framework.framework/Chromium Embedded Framework";

/// Where the main executable, in `Contents/MacOS`, finds the framework; the
/// install name CEF builds it with.
const APPLICATION_LOAD: &str = "@executable_path/../Frameworks/Chromium Embedded Framework.framework/Chromium Embedded Framework";

/// Where a helper, in `Contents/Frameworks/<app> Helper.app/Contents/MacOS`,
/// finds the framework.
const HELPER_LOAD: &str =
    "@executable_path/../../../Chromium Embedded Framework.framework/Chromium Embedded Framework";

/// The load commands that link a library at launch.
const DYLIB_LOADS: [&str; 4] = [
    "LC_LOAD_DYLIB",
    "LC_LOAD_WEAK_DYLIB",
    "LC_REEXPORT_DYLIB",
    "LC_LOAD_UPWARD_DYLIB",
];

/// The arguments `cargo rustc` passes the application's final link, which
/// weakly link the framework of the CEF installation at `cef`.
pub(crate) fn link_args(cef: &Path) -> Vec<OsString> {
    let mut search = OsString::from("link-arg=-F");
    search.push(cef);

    vec![
        "-C".into(),
        "link-arg=-weak_framework".into(),
        "-C".into(),
        format!("link-arg={FRAMEWORK}").into(),
        "-C".into(),
        search,
    ]
}

/// Writes the binary the helpers run, the application's at `application`
/// with its framework load moved to where a helper finds the framework.
///
/// Fails unless each binary weakly links the framework once, at the path its
/// executable finds it at. `install_name_tool` renews the signature the
/// linker gave the binary.
pub(crate) fn helper_executable(application: &Path) -> Result<NamedTempFile> {
    expect_framework_load(application, APPLICATION_LOAD)?;

    let helper = tempfile::Builder::new()
        .prefix("kurogane-helper")
        .tempfile()
        .context("failed to create the helpers' binary")?;
    fs::copy(application, helper.path()).with_context(|| {
        format!(
            "failed to copy {} to {}",
            application.display(),
            helper.path().display()
        )
    })?;

    let status = Command::new("install_name_tool")
        .arg("-change")
        .arg(APPLICATION_LOAD)
        .arg(HELPER_LOAD)
        .arg(helper.path())
        .status()
        .context("failed to run install_name_tool (Xcode command line tools)")?;
    if !status.success() {
        bail!("install_name_tool could not move the helpers' framework load ({status})");
    }

    expect_framework_load(helper.path(), HELPER_LOAD)?;

    Ok(helper)
}

/// Fails unless `binary` weakly links the framework once, at `expected`.
fn expect_framework_load(binary: &Path, expected: &str) -> Result<()> {
    let output = Command::new("otool")
        .arg("-l")
        .arg(binary)
        .output()
        .context("failed to run otool (Xcode command line tools)")?;
    if !output.status.success() {
        bail!(
            "otool could not read {}: {}",
            binary.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let loads = dylib_loads(&String::from_utf8_lossy(&output.stdout));
    check_framework_load(&loads, expected)
        .map_err(|found| anyhow::anyhow!("{} {found}", binary.display()))
}

/// Each library load in `otool -l` output, its command and the path it
/// names.
fn dylib_loads(otool: &str) -> Vec<(String, String)> {
    let mut loads = Vec::new();
    let mut command = None;

    for line in otool.lines().map(str::trim) {
        if let Some(cmd) = line.strip_prefix("cmd ") {
            command = DYLIB_LOADS.contains(&cmd).then(|| cmd.to_owned());
        } else if let Some(name) = line.strip_prefix("name ")
            && let Some(cmd) = command.take()
        {
            let path = name.rsplit_once(" (offset ").map_or(name, |(path, _)| path);
            loads.push((cmd, path.to_owned()));
        }
    }

    loads
}

/// Checks `loads` hold one load of the framework, weak, at `expected`;
/// otherwise says what they hold.
fn check_framework_load(loads: &[(String, String)], expected: &str) -> Result<(), String> {
    let framework: Vec<_> = loads
        .iter()
        .filter(|(_, path)| Path::new(path).ends_with(FRAMEWORK_BINARY))
        .collect();

    match framework.as_slice() {
        [(cmd, path)] if cmd == "LC_LOAD_WEAK_DYLIB" && path == expected => Ok(()),
        [] => Err(format!(
            "does not link the CEF framework, expected a weak load of {expected} \
             (a link with -dead_strip_dylibs drops a framework no symbol uses)"
        )),
        found => Err(format!(
            "links the CEF framework as {}, expected one weak load of {expected}",
            found
                .iter()
                .map(|(cmd, path)| format!("{cmd} {path}"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Component, PathBuf};

    /// `load` with `@executable_path` replaced by `exe`'s directory, `..`
    /// taken lexically, as dyld resolves it.
    fn resolve(load: &str, exe: &Path) -> PathBuf {
        let relative = load.strip_prefix("@executable_path/").unwrap();
        let mut path = exe.parent().unwrap().to_path_buf();
        for component in Path::new(relative).components() {
            match component {
                Component::ParentDir => {
                    path.pop();
                }
                other => path.push(other),
            }
        }
        path
    }

    #[test]
    fn each_executable_finds_its_own_bundle_s_framework() {
        let framework = Path::new("/Apps/My App.app/Contents/Frameworks")
            .join(kurogane_layout::MACOS_FRAMEWORK)
            .join(FRAMEWORK);

        assert_eq!(
            resolve(
                APPLICATION_LOAD,
                Path::new("/Apps/My App.app/Contents/MacOS/myapp")
            ),
            framework
        );
        for helper in [
            "My App Helper",
            "My App Helper (GPU)",
            "My App Helper (Renderer)",
        ] {
            let exe = Path::new("/Apps/My App.app/Contents/Frameworks")
                .join(format!("{helper}.app/Contents/MacOS/{helper}"));
            assert_eq!(resolve(HELPER_LOAD, &exe), framework, "{helper}");
        }
    }

    #[test]
    fn the_link_weakly_links_the_framework_of_the_installation() {
        let cef =
            Path::new("/Users/me/Library/Application Support/tanso/cef/154/cef_macos_aarch64");
        let args = link_args(cef);

        assert_eq!(
            args,
            [
                "-C",
                "link-arg=-weak_framework",
                "-C",
                "link-arg=Chromium Embedded Framework",
                "-C",
                "link-arg=-F/Users/me/Library/Application Support/tanso/cef/154/cef_macos_aarch64",
            ]
            .map(OsString::from)
        );
    }

    const OTOOL: &str = "\
Load command 12
          cmd LC_LOAD_DYLIB
      cmdsize 56
         name /usr/lib/libSystem.B.dylib (offset 24)
Load command 13
          cmd LC_LOAD_WEAK_DYLIB
      cmdsize 120
         name @executable_path/../Frameworks/Chromium Embedded Framework.framework/Chromium Embedded Framework (offset 24)
   time stamp 2 Thu Jan  1 01:00:02 1970
Load command 14
          cmd LC_RPATH
      cmdsize 32
         path @executable_path/../lib (offset 12)
Load command 15
          cmd LC_ID_DYLIB
      cmdsize 48
         name @rpath/libself.dylib (offset 24)
";

    #[test]
    fn otool_output_gives_each_library_load_and_its_path() {
        assert_eq!(
            dylib_loads(OTOOL),
            [
                ("LC_LOAD_DYLIB", "/usr/lib/libSystem.B.dylib"),
                ("LC_LOAD_WEAK_DYLIB", APPLICATION_LOAD),
            ]
            .map(|(cmd, path)| (cmd.to_owned(), path.to_owned()))
        );
    }

    #[test]
    fn only_one_weak_load_at_the_expected_path_passes() {
        let load = |cmd: &str, path: &str| (cmd.to_owned(), path.to_owned());
        let system = load("LC_LOAD_DYLIB", "/usr/lib/libSystem.B.dylib");

        assert!(check_framework_load(&dylib_loads(OTOOL), APPLICATION_LOAD).is_ok());

        let refused = [
            vec![system.clone()],
            vec![load("LC_LOAD_DYLIB", APPLICATION_LOAD)],
            vec![load("LC_LOAD_WEAK_DYLIB", HELPER_LOAD)],
            vec![
                load("LC_LOAD_WEAK_DYLIB", APPLICATION_LOAD),
                load("LC_LOAD_WEAK_DYLIB", HELPER_LOAD),
            ],
            vec![load(
                "LC_LOAD_WEAK_DYLIB",
                "@rpath/Chromium Embedded Framework.framework/Chromium Embedded Framework",
            )],
        ];
        for loads in refused {
            assert!(
                check_framework_load(&loads, APPLICATION_LOAD).is_err(),
                "{loads:?}"
            );
        }
    }
}
