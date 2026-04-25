use serde::Serialize;
use std::{
    collections::BTreeMap,
    process::Command,
};

//
// Public API
//

pub fn collect_all() -> DoctorReport {
    DoctorReport {
        system: system::collect(),
        env: env::collect(),
        cef: cef::collect(),
        gpu: gpu::collect(),
    }
}

//
// Data model
//

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
    pub exists: bool,
}

#[derive(Serialize)]
pub struct GpuInfo {
    pub gl_vendor: Option<String>,
    pub gl_renderer: Option<String>,
    pub gl_version: Option<String>,
    pub adapter_name: Option<String>,
    pub source: String,
}

//
// System
//

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

        #[cfg(target_os = "windows")]
        {
            Command::new("cmd")
                .args(["/C", "ver"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            None
        }
    }

    fn display_server() -> String {
        #[cfg(target_os = "windows")]
        {
            return "win32".into();
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

        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            "unknown".into()
        }
    }
}

mod env {
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
        ];

        let relevant = relevant_keys
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| ((*k).to_string(), v)))
            .collect::<BTreeMap<_, _>>();

        EnvInfo {
            cef_path: std::env::var("CEF_PATH").ok(),
            display: std::env::var("DISPLAY").ok(),
            wayland_display: std::env::var("WAYLAND_DISPLAY").ok(),
            xdg_session_type: std::env::var("XDG_SESSION_TYPE").ok(),
            relevant,
        }
    }
}

//
// CEF
//

mod cef {
    use super::*;

    pub fn collect() -> CefInfo {
        let version = env!("KUROGANE_CEF_VERSION").to_string();

        let path = dirs::home_dir()
            .map(|h| h.join(".local/share/cef").join(&version))
            .unwrap_or_default();

        CefInfo {
            version,
            path: path.display().to_string(),
            exists: path.exists(),
        }
    }
}

//
// GPU
//

mod gpu {
    use super::*;

    pub fn collect() -> GpuInfo {
        #[cfg(target_os = "linux")]
        {
            return collect_linux();
        }

        #[cfg(target_os = "windows")]
        {
            return collect_windows();
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            return GpuInfo {
                gl_vendor: None,
                gl_renderer: None,
                gl_version: None,
                adapter_name: None,
                source: "unsupported".into(),
            };
        }
    }

    #[cfg(target_os = "linux")]
    fn collect_linux() -> GpuInfo {
        if let Some(info) = probe_glxinfo() {
            return info;
        }

        if let Some(info) = probe_eglinfo() {
            return info;
        }

        GpuInfo {
            gl_vendor: None,
            gl_renderer: None,
            gl_version: None,
            adapter_name: None,
            source: "unknown".into(),
        }
    }

    #[cfg(target_os = "windows")]
    fn collect_windows() -> GpuInfo {
        let adapter_name = probe_windows_adapter_name();

        GpuInfo {
            gl_vendor: None,
            gl_renderer: None,
            gl_version: None,
            adapter_name,
            source: "windows-adapter".into(),
        }
    }

    #[cfg(target_os = "linux")]
    fn probe_glxinfo() -> Option<GpuInfo> {
        let out = Command::new("glxinfo").arg("-B").output().ok()?;
        if !out.status.success() {
            return None;
        }

        let text = String::from_utf8_lossy(&out.stdout);

        Some(GpuInfo {
            gl_vendor: find(&text, "OpenGL vendor string:"),
            gl_renderer: find(&text, "OpenGL renderer string:"),
            gl_version: find(&text, "OpenGL version string:"),
            adapter_name: None,
            source: "glxinfo -B".into(),
        })
    }

    #[cfg(target_os = "linux")]
    fn probe_eglinfo() -> Option<GpuInfo> {
        let out = Command::new("eglinfo").arg("--client").output().ok()?;
        if !out.status.success() {
            return None;
        }

        let text = String::from_utf8_lossy(&out.stdout);

        Some(GpuInfo {
            gl_vendor: find(&text, "Vendor:"),
            gl_renderer: find(&text, "Device:"),
            gl_version: find(&text, "OpenGL version string:"),
            adapter_name: None,
            source: "eglinfo --client".into(),
        })
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

    #[cfg(target_os = "linux")]
    fn find(text: &str, key: &str) -> Option<String> {
        text.lines()
            .find(|l| l.contains(key))
            .and_then(|l| l.split_once(':'))
            .map(|(_, v)| v.trim().to_string())
            .filter(|s| !s.is_empty())
    }
}
