use crate::app::{
    ClientAppBrowserDelegate, ClientAppRendererDelegate, PumpScheduler, SecondInstanceHandler,
};
use crate::chromium_flags::ChromiumFlag;
use crate::fs::CanonicalRoot;
use crate::credentials::CredentialStorage;
use crate::gpu::GpuMode;
use crate::scheme::CustomScheme;
use crate::window::WindowIdentity;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeMode {
    Views,
    Embedded,
}

/// Chromium process sandbox policy.
///
/// Selected with [`App::sandbox_mode`](crate::App::sandbox_mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum SandboxMode {
    /// Run Chromium without its helper-process sandbox.
    ///
    /// CEF is initialized with `no_sandbox=1` and the platform's
    /// sandbox-disabling switches are set.
    #[default]
    Disabled,

    /// Run renderer, GPU and utility processes inside Chromium's sandbox.
    ///
    /// Startup fails unless the platform can enforce it:
    /// - Linux: unprivileged user namespaces, or a root-owned setuid
    ///   `chrome-sandbox` helper.
    /// - macOS: the app runs from a `.app` bundle, so each helper can enter
    ///   its seatbelt sandbox before loading CEF.
    /// - Windows: the app is loaded by CEF's bootstrap executable, which
    ///   brokers the sandbox. See [`sandbox_entry!`](crate::sandbox_entry).
    Chromium,
}

/// Immutable startup intent for the runtime.
#[derive(Clone)]
pub(crate) struct RuntimeSpec {
    pub mode: RuntimeMode,
    pub sandbox_mode: SandboxMode,
    pub start_url: String,
    pub asset_root: Option<CanonicalRoot>,
    pub profile_id: Option<String>,
    /// Where the Chromium profile (cache_path) lives; None derives it from profile_id.
    pub cache_dir: Option<PathBuf>,
    pub persist_session_cookies: bool,
    pub gpu_mode: GpuMode,
    pub credential_storage: CredentialStorage,
    pub chromium_flags: Vec<ChromiumFlag>,
    pub scheduler: Option<PumpScheduler>,
    pub on_second_instance: Option<SecondInstanceHandler>,
    pub delegates: Vec<Arc<dyn ClientAppBrowserDelegate>>,
    pub renderer_delegates: Vec<Arc<dyn ClientAppRendererDelegate>>,
    pub scheme_handlers: Vec<CustomScheme>,
    /// How the window manager sees the application's first window.
    pub window_identity: WindowIdentity,
}
