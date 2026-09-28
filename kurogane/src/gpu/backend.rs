use crate::chromium_flags::ChromiumFlags;
use crate::spec::SandboxMode;
use super::detection::RenderingEnvironment;

#[cfg(target_os = "linux")]
use super::linux as platform;

#[cfg(target_os = "windows")]
use super::windows as platform;

#[cfg(target_os = "macos")]
use super::macos as platform;

/// GPU backend selection strategy.
///
/// Pass to App::gpu_mode to control how Chromium selects its rendering backend (default = GpuMode::Auto).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GpuMode {
    /// Automatically select a backend
    #[default]
    Auto,

    /// Use hardware acceleration
    ///
    /// On Windows without [`SandboxMode::Chromium`](crate::SandboxMode::Chromium),
    /// GPU work runs inside the browser process rather than a GPU process.
    Hardware,

    /// Use SwiftShader software rendering
    Software,

    /// Disable GPU entirely. No canvas acceleration, no WebGL
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResolvedGpuMode {
    Hardware,
    Software,
    Disabled,
}

/// Apply Chromium command-line flags for the configured GPU mode
pub(crate) fn apply_gpu_flags(flags: &mut ChromiumFlags, requested: GpuMode, sandbox: SandboxMode) {
    let env = RenderingEnvironment::detect();

    let mode = resolve(requested, &env);

    match mode {
        ResolvedGpuMode::Hardware => platform::apply_hardware(flags, sandbox),
        ResolvedGpuMode::Software => apply_software(flags),
        ResolvedGpuMode::Disabled => apply_disabled(flags),
    }
}

fn resolve(requested: GpuMode, env: &RenderingEnvironment) -> ResolvedGpuMode {
    match requested {
        GpuMode::Auto => resolve_auto(env),

        GpuMode::Hardware => ResolvedGpuMode::Hardware,

        GpuMode::Software => ResolvedGpuMode::Software,

        GpuMode::Disabled => ResolvedGpuMode::Disabled,
    }
}

fn resolve_auto(env: &RenderingEnvironment) -> ResolvedGpuMode {
    if env.virtualization {
        ResolvedGpuMode::Software
    } else {
        ResolvedGpuMode::Hardware
    }
}

fn apply_software(flags: &mut ChromiumFlags) {
    flags.set_with_value("use-gl", "angle");
    flags.set_with_value("use-angle", "swiftshader");
}

fn apply_disabled(flags: &mut ChromiumFlags) {
    flags.set("disable-gpu");
    flags.set("disable-gpu-compositing");
    flags.set("disable-software-rasterizer");
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests for GPU policy resolution independent of environment detection

    #[test]
    fn auto_uses_hardware_for_real_gpu() {
        let env = RenderingEnvironment {
            virtualization: false,
        };

        assert_eq!(resolve(GpuMode::Auto, &env), ResolvedGpuMode::Hardware,);
    }

    #[test]
    fn auto_uses_software_for_virtual_gpu() {
        let env = RenderingEnvironment {
            virtualization: true,
        };

        assert_eq!(resolve(GpuMode::Auto, &env), ResolvedGpuMode::Software,);
    }

    #[test]
    fn explicit_hardware_overrides_environment() {
        let env = RenderingEnvironment {
            virtualization: true,
        };

        assert_eq!(resolve(GpuMode::Hardware, &env), ResolvedGpuMode::Hardware,);
    }

    #[test]
    fn explicit_software_overrides_environment() {
        let env = RenderingEnvironment {
            virtualization: false,
        };

        assert_eq!(resolve(GpuMode::Software, &env), ResolvedGpuMode::Software,);
    }

    #[test]
    fn the_sandbox_keeps_gpu_work_out_of_the_browser_process() {
        let mut flags = ChromiumFlags::default();
        apply_gpu_flags(&mut flags, GpuMode::Hardware, SandboxMode::Chromium);

        assert!(
            !flags.contains("in-process-gpu"),
            "the browser process is not sandboxed, so GPU work must not run there"
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn unsandboxed_windows_runs_gpu_work_in_process() {
        let mut flags = ChromiumFlags::default();
        apply_gpu_flags(&mut flags, GpuMode::Hardware, SandboxMode::Disabled);

        assert!(flags.contains("in-process-gpu"));
    }

    #[test]
    fn disabled_always_disables_gpu() {
        let env = RenderingEnvironment {
            virtualization: false,
        };

        assert_eq!(resolve(GpuMode::Disabled, &env), ResolvedGpuMode::Disabled,);
    }
}
