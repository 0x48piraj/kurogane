//! Chromium runtime fixtures for tests.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// Creates a temporary directory for a test.
pub fn tmp_dir() -> TempDir {
    tempfile::tempdir().expect("failed to create temp dir")
}

/// Platform for which a CEF runtime fixture is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Windows,
    Linux,
    MacOs,
}

/// Returns the host platform.
fn host_target() -> Target {
    #[cfg(target_os = "windows")]
    {
        Target::Windows
    }
    #[cfg(target_os = "macos")]
    {
        Target::MacOs
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        Target::Linux
    }
}

/// Creates a CEF runtime fixture for the host platform.
pub fn cef_runtime(dir: &Path) -> PathBuf {
    cef_runtime_for(dir, host_target())
}

/// Creates a CEF runtime fixture for the specified platform.
pub fn cef_runtime_for(dir: &Path, target: Target) -> PathBuf {
    std::fs::create_dir_all(dir).expect("create fixture dir");

    match target {
        Target::Windows => {
            write(dir.join("libcef.dll"), "cef");
            write(dir.join("chrome_elf.dll"), "elf");
            write(dir.join("icudtl.dat"), "icu");
            write(dir.join("v8_context_snapshot.bin"), "v8");
            write(dir.join("locales/en-US.pak"), "pak");
        }
        Target::MacOs => {
            let fw = dir.join("Chromium Embedded Framework.framework");
            std::fs::create_dir_all(&fw).unwrap();
            write(fw.join("Chromium Embedded Framework"), "cef");

            // ANGLE libraries required by Chromium's GPU process
            let libraries = fw.join("Libraries");
            std::fs::create_dir_all(&libraries).unwrap();
            for name in [
                "libEGL.dylib",
                "libGLESv2.dylib",
                "libvk_swiftshader.dylib",
                "vk_swiftshader_icd.json",
            ] {
                write(libraries.join(name), name);
            }

            let resources = fw.join("Resources");
            write(resources.join("en.lproj/locale.pak"), "pak");
            write(resources.join("icudtl.dat"), "icu");
            write(resources.join("v8_context_snapshot.arm64.bin"), "v8");
        }
        Target::Linux => {
            write(dir.join("libcef.so"), "cef");
            write(dir.join("chrome-sandbox"), "sandbox");
            write(dir.join("icudtl.dat"), "icu");
            write(dir.join("v8_context_snapshot.bin"), "v8");
            write(dir.join("locales/en-US.pak"), "pak");
        }
    }

    dir.to_path_buf()
}

fn write(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create fixture parent");
    }
    std::fs::write(path, contents).unwrap();
}
