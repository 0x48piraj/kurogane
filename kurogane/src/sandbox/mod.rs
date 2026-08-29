use crate::chromium_flags::ChromiumFlags;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
mod macos;

pub(crate) fn apply_sandbox_flags(flags: &mut ChromiumFlags) {
    #[cfg(target_os = "linux")]
    linux::apply_sandbox_flags(flags);

    #[cfg(target_os = "windows")]
    windows::apply_sandbox_flags(flags);

    #[cfg(target_os = "macos")]
    macos::apply_sandbox_flags(flags);
}
