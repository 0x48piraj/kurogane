use cef::*;

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

/// Apply Chromium command-line flags for the configured GPU mode
pub(crate) fn apply(
    cmd: &mut CommandLine,
    requested: GpuMode,
) {
    let env = RenderingEnvironment::detect();

    let mode = resolve(requested, &env);

    match mode {
        GpuMode::Hardware => platform::apply_hardware(cmd),
        GpuMode::Software => apply_software(cmd),
        GpuMode::Disabled => apply_disabled(cmd),
        GpuMode::Auto => unreachable!(),
    }
}

fn resolve(
    requested: GpuMode,
    env: &RenderingEnvironment,
) -> GpuMode {
    match requested {
        GpuMode::Auto => default_mode(env),
        other => other,
    }
}

fn default_mode(env: &RenderingEnvironment) -> GpuMode {
    if env.is_virtual_gpu {
        GpuMode::Software
    } else {
        GpuMode::Hardware
    }
}

fn gpu_off(cmd: &mut CommandLine) {
    cmd.append_switch(Some(&CefString::from("disable-gpu")));
    cmd.append_switch(Some(&CefString::from("disable-gpu-compositing")));
}

fn apply_software(cmd: &mut CommandLine) {
    gpu_off(cmd);

    cmd.append_switch_with_value(
        Some(&CefString::from("use-gl")),
        Some(&CefString::from("swiftshader")),
    );
}

fn apply_disabled(cmd: &mut CommandLine) {
    gpu_off(cmd);

    cmd.append_switch(Some(&CefString::from("disable-software-rasterizer")));
}
