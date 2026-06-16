use crate::app::{ClientAppBrowserDelegate, ClientAppRendererDelegate, PumpScheduler};
use crate::chromium_flags::ChromiumFlag;
use crate::fs::CanonicalRoot;
use crate::gpu::GpuMode;
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
    pub persist_session_cookies: bool,
    pub gpu_mode: GpuMode,
    pub chromium_flags: Vec<ChromiumFlag>,
    pub scheduler: Option<PumpScheduler>,
    pub delegates: Vec<Arc<dyn ClientAppBrowserDelegate>>,
    pub renderer_delegates: Vec<Arc<dyn ClientAppRendererDelegate>>,
}
