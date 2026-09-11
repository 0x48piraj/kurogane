use crate::app::{ClientAppBrowserDelegate, ClientAppRendererDelegate, PumpScheduler};
use crate::window::WindowIdentity;
use crate::chromium_flags::ChromiumFlag;
use crate::fs::CanonicalRoot;
use crate::credentials::CredentialStorage;
use crate::gpu::GpuMode;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeMode {
    Views,
    Embedded,
}

/// Immutable startup intent for the runtime.
#[derive(Clone)]
pub(crate) struct RuntimeSpec {
    pub mode: RuntimeMode,
    pub start_url: String,
    pub asset_root: Option<CanonicalRoot>,
    pub profile_id: Option<String>,
    /// Where the Chromium profile (cache_path) lives; None derives it from profile_id and the exe.
    pub cache_dir: Option<PathBuf>,
    pub persist_session_cookies: bool,
    pub gpu_mode: GpuMode,
    pub credential_storage: CredentialStorage,
    pub chromium_flags: Vec<ChromiumFlag>,
    pub scheduler: Option<PumpScheduler>,
    pub delegates: Vec<Arc<dyn ClientAppBrowserDelegate>>,
    pub renderer_delegates: Vec<Arc<dyn ClientAppRendererDelegate>>,
    /// How the window manager sees the windows this app opens.
    pub window_identity: WindowIdentity,
}
