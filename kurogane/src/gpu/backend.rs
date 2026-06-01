use crate::chromium_flags::ChromiumFlags;
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
pub(crate) fn apply_gpu_flags(flags: &mut ChromiumFlags, requested: GpuMode) {
    let env = RenderingEnvironment::detect();

    let mode = resolve(requested, &env);

    match mode {
        ResolvedGpuMode::Hardware => platform::apply_hardware(flags),
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
    if env.is_virtual_gpu {
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
