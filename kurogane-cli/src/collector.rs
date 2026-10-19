//! Diagnostic data collection.
//!
//! This module gathers system, environment, CEF and GPU information into a
//! serializable report suitable for both human-readable and JSON output.

use serde::Serialize;
use std::{collections::BTreeMap, process::Command};

pub fn collect_all() -> DoctorReport {
    DoctorReport {
        system: system::collect(),
        env: env::collect(),
        cef: cef::collect(),
        gpu: gpu::collect(),
    }
}

#[derive(Serialize)]
pub struct DoctorReport {
    pub system: SystemInfo,
    pub env: EnvInfo,
    pub cef: CefInfo,
    pub gpu: GpuInfo,
}

#[derive(Serialize)]
pub struct SystemInfo {
    pub os: String,
    pub arch: String,
    pub kernel: Option<String>,
    pub hostname: Option<String>,
    pub display_server: String,
}

#[derive(Serialize)]
pub struct EnvInfo {
    pub cef_path: Option<String>,
    pub display: Option<String>,
    pub wayland_display: Option<String>,
    pub xdg_session_type: Option<String>,
    pub relevant: BTreeMap<String, String>,
}

#[derive(Serialize)]
pub struct CefInfo {
    pub version: String,
    pub path: String,
    pub valid: bool,
    pub source: String,
}

#[derive(Serialize)]
pub struct GpuInfo {
    /// Tool the probe ran.
    pub source: &'static str,
    pub probe: GpuProbe,
}

/// Outcome of the GPU probe.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case", tag = "status")]
pub enum GpuProbe {
    /// Linux OpenGL renderer named by `glxinfo`.
    #[cfg(target_os = "linux")]
    Renderer(GlRenderer),
    /// macOS or Windows display adapter.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    Adapter { name: String },
    /// Linux `glxinfo` not installed.
    #[cfg(target_os = "linux")]
    Missing,
    /// Linux session without an X display to query.
    #[cfg(target_os = "linux")]
    NoDisplay,
    /// Probe ran without naming a GPU; what it printed.
    Failed { output: String },
}

/// OpenGL renderer as `glxinfo` names it.
#[cfg(target_os = "linux")]
#[derive(Serialize)]
pub struct GlRenderer {
    pub name: String,
    pub vendor: Option<String>,
    pub version: Option<String>,
    /// Whether OpenGL renders on the GPU rather than in software
    pub accelerated: Option<bool>,
}

/// System and desktop-session information.
mod system {
    use super::*;

    pub fn collect() -> SystemInfo {
        let os = std::env::consts::OS.to_string();
        let arch = std::env::consts::ARCH.to_string();
        let kernel = kernel_version();
        let hostname = hostname();
        let display_server = display_server();

        SystemInfo {
            os,
            arch,
            kernel,
            hostname,
            display_server,
        }
    }

    fn hostname() -> Option<String> {
        #[cfg(target_os = "windows")]
        {
            std::env::var("COMPUTERNAME").ok()
        }

        #[cfg(not(target_os = "windows"))]
        {
            std::env::var("HOSTNAME").ok().or_else(|| {
                Command::new("hostname")
                    .output()
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .filter(|s| !s.is_empty())
            })
        }
    }

    fn kernel_version() -> Option<String> {
        #[cfg(target_os = "linux")]
        {
            Command::new("uname")
                .arg("-r")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        }

        #[cfg(target_os = "macos")]
        {
            Command::new("sw_vers")
                .args(["-productVersion"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        }

        #[cfg(target_os = "windows")]
        {
            Command::new("cmd")
                .args(["/C", "ver"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            None
        }
    }

    fn display_server() -> String {
        #[cfg(target_os = "windows")]
        {
            "win32".into()
        }

        #[cfg(target_os = "macos")]
        {
            "appkit".into()
        }

        #[cfg(target_os = "linux")]
        {
            match std::env::var("XDG_SESSION_TYPE") {
                Ok(v) if v == "wayland" => "wayland".into(),
                Ok(v) if v == "x11" => "x11".into(),
                _ => {
                    if std::env::var("WAYLAND_DISPLAY").is_ok() {
                        "wayland".into()
                    } else if std::env::var("DISPLAY").is_ok() {
                        "x11".into()
                    } else {
                        "unknown".into()
                    }
                }
            }
        }

        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            "unknown".into()
        }
    }
}

mod env {
    //! Runtime environment variables relevant to Kurogane and CEF.
    use super::*;

    pub fn collect() -> EnvInfo {
        let relevant_keys = [
            // session
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "XDG_SESSION_TYPE",
            "XDG_CURRENT_DESKTOP",
            // GPU / GL overrides
            "__GLX_VENDOR_LIBRARY_NAME",
            "LIBGL_ALWAYS_SOFTWARE",
            "MESA_LOADER_DRIVER_OVERRIDE",
            // Wayland / toolkit
            "NIXOS_OZONE_WL",
            "QT_QPA_PLATFORM",
            "SDL_VIDEODRIVER",
            // runtime / linking
            "CEF_PATH",
            "LD_LIBRARY_PATH",
            "DYLD_LIBRARY_PATH",
            "DYLD_FALLBACK_LIBRARY_PATH",
            // macOS application identity
            "__CFBundleIdentifier",
        ];

        let relevant = relevant_keys
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| ((*k).to_string(), v)))
            .collect::<BTreeMap<_, _>>();

        EnvInfo {
            cef_path: crate::install::cef_path().map(|path| path.display().to_string()),
            display: std::env::var("DISPLAY").ok(),
            wayland_display: std::env::var("WAYLAND_DISPLAY").ok(),
            xdg_session_type: std::env::var("XDG_SESSION_TYPE").ok(),
            relevant,
        }
    }
}

mod cef {
    //! The CEF the project's application loads.
    use super::*;
    use crate::install::{CefSource, cef_path, find_cef, installed_cef_dir};

    /// The runtime the project's application loads, the one `CEF_PATH`
    /// names or else the installation of its CEF version, valid as `run`
    /// and `bundle` count it.
    pub fn collect() -> CefInfo {
        let version = crate::install::project_cef_version();
        let (root, source) = match cef_path() {
            Some(root) => (Some(root), CefSource::CefPath),
            None => (installed_cef_dir(&version).ok(), CefSource::Installed),
        };

        CefInfo {
            valid: find_cef(&version).is_ok(),
            version,
            path: root
                .as_ref()
                .map(|root| root.display().to_string())
                .unwrap_or_default(),
            source: source.to_string(),
        }
    }
}

pub(crate) mod gpu {
    //! Best-effort GPU and graphics-stack diagnostics.
    use super::*;

    pub fn collect() -> GpuInfo {
        #[cfg(target_os = "linux")]
        {
            collect_linux()
        }

        #[cfg(target_os = "macos")]
        {
            adapter_info(probe_macos_adapter_name(), "system_profiler")
        }

        #[cfg(target_os = "windows")]
        {
            adapter_info(probe_windows_adapter_name(), "windows-adapter")
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            return GpuInfo {
                source: "unsupported",
                probe: GpuProbe::Failed {
                    output: "no GPU probe for this platform".into(),
                },
            };
        }
    }

    /// Returns what a probe naming the display adapter found.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn adapter_info(name: Option<String>, source: &'static str) -> GpuInfo {
        let probe = match name {
            Some(name) => GpuProbe::Adapter { name },
            None => GpuProbe::Failed {
                output: format!("{source} named no display adapter"),
            },
        };

        GpuInfo { source, probe }
    }

    #[cfg(target_os = "linux")]
    fn collect_linux() -> GpuInfo {
        let probe = if std::env::var_os("DISPLAY").is_none() {
            GpuProbe::NoDisplay
        } else {
            match Command::new("glxinfo").arg("-B").output() {
                Ok(output) => read_glxinfo(
                    &String::from_utf8_lossy(&output.stdout),
                    &String::from_utf8_lossy(&output.stderr),
                ),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => GpuProbe::Missing,
                Err(e) => GpuProbe::Failed {
                    output: format!("glxinfo could not run: {e}"),
                },
            }
        };

        GpuInfo {
            source: "glxinfo -B",
            probe,
        }
    }

    /// Returns what `glxinfo -B` printed about the GPU.
    ///
    /// glxinfo can exit successfully without a usable OpenGL. A missing
    /// renderer is the failure.
    #[cfg(target_os = "linux")]
    fn read_glxinfo(stdout: &str, stderr: &str) -> GpuProbe {
        let Some(name) = find(stdout, "OpenGL renderer string:") else {
            let printed = [stderr.trim(), stdout.trim()]
                .into_iter()
                .find(|text| !text.is_empty())
                .unwrap_or("glxinfo printed nothing");
            return GpuProbe::Failed {
                output: printed.to_owned(),
            };
        };

        GpuProbe::Renderer(GlRenderer {
            name,
            vendor: find(stdout, "OpenGL vendor string:"),
            version: find(stdout, "OpenGL version string:"),
            accelerated: find(stdout, "Accelerated:").map(|value| value == "yes"),
        })
    }

    #[cfg(target_os = "macos")]
    fn probe_macos_adapter_name() -> Option<String> {
        let out = Command::new("system_profiler")
            .arg("SPDisplaysDataType")
            .output()
            .ok()?;

        if !out.status.success() {
            return None;
        }

        let text = String::from_utf8_lossy(&out.stdout);
        let chipset = find(&text, "Chipset Model:");
        let vendor = find(&text, "Vendor:");

        match (chipset, vendor) {
            (Some(c), Some(v)) => Some(format!("{} ({})", c, v)),
            (Some(c), None) => Some(c),
            (None, Some(v)) => Some(v),
            (None, None) => None,
        }
    }

    #[cfg(target_os = "windows")]
    fn probe_windows_adapter_name() -> Option<String> {
        let out = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "(Get-CimInstance Win32_VideoController | Select-Object -ExpandProperty Name) -join '; '",
            ])
            .output()
            .ok()?;

        if !out.status.success() {
            return None;
        }

        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() { None } else { Some(s) }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn find(text: &str, key: &str) -> Option<String> {
        text.lines()
            .find(|l| l.contains(key))
            .and_then(|l| l.split_once(':'))
            .map(|(_, v)| v.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    #[cfg(all(test, target_os = "linux"))]
    mod tests {
        use super::*;

        const WORKING: &str = "\
name of display: :0
display: :0  screen: 0
direct rendering: Yes
Extended renderer info (GLX_MESA_query_renderer):
    Vendor: AMD (0x1002)
    Device: AMD Radeon RX 6500 XT (radeonsi, navi24, ACO, DRM 3.64, 7.2.8) (0x743f)
    Version: 26.2.3
    Accelerated: yes
OpenGL vendor string: AMD
OpenGL renderer string: AMD Radeon RX 6500 XT (radeonsi, navi24, ACO, DRM 3.64, 7.2.8)
OpenGL core profile version string: 4.6 (Core Profile) Mesa 26.2.3
OpenGL version string: 4.6 (Compatibility Profile) Mesa 26.2.3
OpenGL ES profile version string: OpenGL ES 3.2 Mesa 26.2.3
";

        /// Returns the renderer `probe` names, failing the test otherwise.
        #[track_caller]
        fn renderer(probe: GpuProbe) -> GlRenderer {
            let GpuProbe::Renderer(renderer) = probe else {
                panic!("glxinfo naming a renderer finds the GPU");
            };
            renderer
        }

        /// Returns what a failed `probe` printed, failing the test otherwise.
        #[track_caller]
        fn failure(probe: GpuProbe) -> String {
            let GpuProbe::Failed { output } = probe else {
                panic!("glxinfo without a renderer fails");
            };
            output
        }

        #[test]
        fn glxinfo_naming_a_renderer_finds_the_gpu() {
            let gl = renderer(read_glxinfo(WORKING, ""));

            assert_eq!(
                gl.name,
                "AMD Radeon RX 6500 XT (radeonsi, navi24, ACO, DRM 3.64, 7.2.8)"
            );
            assert_eq!(gl.vendor.as_deref(), Some("AMD"));
            assert_eq!(
                gl.version.as_deref(),
                Some("4.6 (Compatibility Profile) Mesa 26.2.3")
            );
            assert_eq!(gl.accelerated, Some(true));
        }

        #[test]
        fn a_software_renderer_is_not_accelerated() {
            let software = WORKING.replace("Accelerated: yes", "Accelerated: no");
            assert_eq!(
                renderer(read_glxinfo(&software, "")).accelerated,
                Some(false)
            );
        }

        #[test]
        fn glxinfo_without_a_renderer_failed_whatever_its_exit() {
            let output = failure(read_glxinfo(
                "name of display: :0\n",
                "Error: couldn't find RGB GLX visual or fbconfig\n",
            ));
            assert_eq!(output, "Error: couldn't find RGB GLX visual or fbconfig");
        }

        #[test]
        fn a_silent_glxinfo_failure_says_so() {
            assert_eq!(failure(read_glxinfo("", "")), "glxinfo printed nothing");
        }
    }
}
