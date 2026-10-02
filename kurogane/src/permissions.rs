//! What a page may use only with consent: a camera, a microphone, the
//! screen, the location, notifications, the clipboard and the rest of the
//! web's permissions.
//!
//! CEF asks in two ways. A camera, a microphone or the screen
//! (`getUserMedia`, `getDisplayMedia`) comes to
//! OnRequestMediaAccessPermission, whose answer is for that one request:
//! nothing is remembered. Everything else comes to OnShowPermissionPrompt,
//! the answer to Chromium's prompt, which Chromium remembers in the profile
//! for a web site (http, https), so that site's later requests never come
//! back. For the application's own scheme it remembers nothing, which also
//! means a grant that must still hold later, to show a notification or read
//! the location, is not there.
//!
//! Left unanswered, a window shows Chromium's own prompt, which every later
//! request waits behind, and an embedded browser denies devices and leaves
//! every other request pending forever. Kurogane answers each request in
//! both: the application's [`App::on_permission`](crate::App::on_permission)
//! hook decides, now or later through a [`PermissionResponder`], and
//! anything it does not allow is denied.

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use cef::sys::cef_content_setting_types_t as Setting;
use cef::sys::cef_media_access_permission_types_t as Media;
use cef::sys::cef_permission_request_types_t as Prompt;
use cef::*;
use tracing::{debug, error, warn};

use crate::acl::Origin;
use crate::browser_registry::BrowserId;
use crate::runtime::{AppHandle, RuntimeServices};

/// What a page asks for, in a [`PermissionRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Permission {
    /// A camera (`getUserMedia` with video).
    Camera,
    /// A microphone (`getUserMedia` with audio).
    Microphone,
    /// A picture of the screen (`getDisplayMedia`). There is no picker:
    /// allowed, the page records the whole screen.
    ScreenVideo,
    /// The sound the system plays, with a picture of the screen.
    ScreenAudio,
    /// The device's location (`navigator.geolocation`).
    Geolocation,
    /// Showing notifications (`Notification.requestPermission`).
    Notifications,
    /// Reading the clipboard (`navigator.clipboard.readText` and `read`).
    ClipboardRead,
    /// MIDI devices with system-exclusive messages
    /// (`requestMIDIAccess({ sysex: true })`).
    MidiSysex,
    /// Turning a camera: pan, tilt and zoom.
    CameraPanTiltZoom,
    /// An augmented-reality session (WebXR).
    ArSession,
    /// A virtual-reality session (WebXR).
    VrSession,
    /// Tracking the user's hands in a WebXR session.
    HandTracking,
    /// Scrolling and zooming a captured tab or window.
    CapturedSurfaceControl,
    /// An embedded site's own cookies and storage inside another site's
    /// page (`document.requestStorageAccess`).
    StorageAccess,
    /// A site's own cookies and storage inside the pages of other sites,
    /// asked for by the site itself (`requestStorageAccessFor`).
    TopLevelStorageAccess,
    /// More disk space for the site's storage.
    DiskQuota,
    /// The fonts installed on the system (`queryLocalFonts`).
    LocalFonts,
    /// Signing in with the site as an identity provider (FedCM).
    IdentityProvider,
    /// Whether the user is idle (`IdleDetector`).
    IdleDetection,
    /// Keys the system would otherwise take, such as Alt+Tab
    /// (`navigator.keyboard.lock`).
    KeyboardLock,
    /// Holding the pointer inside the page (`requestPointerLock`).
    PointerLock,
    /// An identifier for playing protected media.
    ProtectedMediaIdentifier,
    /// Handling links of a URL scheme (`registerProtocolHandler`).
    RegisterProtocolHandler,
    /// Installing the site as an app.
    WebAppInstallation,
    /// The screens' layout, and placing windows on them
    /// (`getScreenDetails`).
    WindowManagement,
    /// Writing to files and folders the user picked (File System Access).
    FileSystemAccess,
    /// Reaching devices on the local network.
    LocalNetwork,
    /// Reaching services on this computer.
    LoopbackNetwork,
    /// Motion and orientation sensors.
    Sensors,
}

impl Permission {
    /// Where Chromium remembers its answer to a prompt about it, as its
    /// permissions code maps each request type to a setting
    /// (`RequestTypeToContentSettingsType`), geolocation in either of its
    /// two settings. Every one is registered on every desktop platform but
    /// the one listed for Windows alone: CEF hands a setting Chromium does
    /// not register to a CHECK, which ends the browser process. A camera, a
    /// microphone and the screen asked for by a page's call are never
    /// remembered; the first two are listed for their prompts.
    fn remembered_in(self) -> &'static [Setting] {
        match self {
            Permission::Camera => &[Setting::CEF_CONTENT_SETTING_TYPE_MEDIASTREAM_CAMERA],
            Permission::Microphone => &[Setting::CEF_CONTENT_SETTING_TYPE_MEDIASTREAM_MIC],
            Permission::Geolocation => &[
                Setting::CEF_CONTENT_SETTING_TYPE_GEOLOCATION,
                Setting::CEF_CONTENT_SETTING_TYPE_GEOLOCATION_WITH_OPTIONS,
            ],
            Permission::Notifications => &[Setting::CEF_CONTENT_SETTING_TYPE_NOTIFICATIONS],
            Permission::ClipboardRead => &[Setting::CEF_CONTENT_SETTING_TYPE_CLIPBOARD_READ_WRITE],
            Permission::MidiSysex => &[Setting::CEF_CONTENT_SETTING_TYPE_MIDI_SYSEX],
            Permission::CameraPanTiltZoom => {
                &[Setting::CEF_CONTENT_SETTING_TYPE_CAMERA_PAN_TILT_ZOOM]
            }
            Permission::ArSession => &[Setting::CEF_CONTENT_SETTING_TYPE_AR],
            Permission::VrSession => &[Setting::CEF_CONTENT_SETTING_TYPE_VR],
            Permission::HandTracking => &[Setting::CEF_CONTENT_SETTING_TYPE_HAND_TRACKING],
            Permission::CapturedSurfaceControl => {
                &[Setting::CEF_CONTENT_SETTING_TYPE_CAPTURED_SURFACE_CONTROL]
            }
            Permission::StorageAccess => &[Setting::CEF_CONTENT_SETTING_TYPE_STORAGE_ACCESS],
            Permission::TopLevelStorageAccess => {
                &[Setting::CEF_CONTENT_SETTING_TYPE_TOP_LEVEL_STORAGE_ACCESS]
            }
            Permission::LocalFonts => &[Setting::CEF_CONTENT_SETTING_TYPE_LOCAL_FONTS],
            Permission::IdleDetection => &[Setting::CEF_CONTENT_SETTING_TYPE_IDLE_DETECTION],
            Permission::KeyboardLock => &[Setting::CEF_CONTENT_SETTING_TYPE_KEYBOARD_LOCK],
            Permission::PointerLock => &[Setting::CEF_CONTENT_SETTING_TYPE_POINTER_LOCK],
            #[cfg(target_os = "windows")]
            Permission::ProtectedMediaIdentifier => {
                &[Setting::CEF_CONTENT_SETTING_TYPE_PROTECTED_MEDIA_IDENTIFIER]
            }
            Permission::WebAppInstallation => {
                &[Setting::CEF_CONTENT_SETTING_TYPE_WEB_APP_INSTALLATION]
            }
            Permission::WindowManagement => &[Setting::CEF_CONTENT_SETTING_TYPE_WINDOW_MANAGEMENT],
            Permission::LocalNetwork => &[Setting::CEF_CONTENT_SETTING_TYPE_LOCAL_NETWORK],
            Permission::LoopbackNetwork => &[Setting::CEF_CONTENT_SETTING_TYPE_LOOPBACK_NETWORK],
            Permission::Sensors => &[Setting::CEF_CONTENT_SETTING_TYPE_SENSORS],
            #[cfg(not(target_os = "windows"))]
            Permission::ProtectedMediaIdentifier => &[],
            Permission::ScreenVideo
            | Permission::ScreenAudio
            | Permission::DiskQuota
            | Permission::IdentityProvider
            | Permission::RegisterProtocolHandler
            | Permission::FileSystemAccess => &[],
        }
    }

    fn name(self) -> &'static str {
        match self {
            Permission::Camera => "camera",
            Permission::Microphone => "microphone",
            Permission::ScreenVideo => "screen",
            Permission::ScreenAudio => "screen audio",
            Permission::Geolocation => "geolocation",
            Permission::Notifications => "notifications",
            Permission::ClipboardRead => "clipboard read",
            Permission::MidiSysex => "MIDI sysex",
            Permission::CameraPanTiltZoom => "camera pan, tilt and zoom",
            Permission::ArSession => "AR session",
            Permission::VrSession => "VR session",
            Permission::HandTracking => "hand tracking",
            Permission::CapturedSurfaceControl => "captured surface control",
            Permission::StorageAccess => "storage access",
            Permission::TopLevelStorageAccess => "top-level storage access",
            Permission::DiskQuota => "disk quota",
            Permission::LocalFonts => "local fonts",
            Permission::IdentityProvider => "identity provider",
            Permission::IdleDetection => "idle detection",
            Permission::KeyboardLock => "keyboard lock",
            Permission::PointerLock => "pointer lock",
            Permission::ProtectedMediaIdentifier => "protected media identifier",
            Permission::RegisterProtocolHandler => "protocol handler",
            Permission::WebAppInstallation => "web app installation",
            Permission::WindowManagement => "window management",
            Permission::FileSystemAccess => "file system access",
            Permission::LocalNetwork => "local network",
            Permission::LoopbackNetwork => "loopback network",
            Permission::Sensors => "sensors",
        }
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// What each bit of a request for a camera, a microphone or the screen
/// asks for.
const MEDIA: [(u32, Permission); 4] = [
    (
        Media::CEF_MEDIA_PERMISSION_DEVICE_VIDEO_CAPTURE as u32,
        Permission::Camera,
    ),
    (
        Media::CEF_MEDIA_PERMISSION_DEVICE_AUDIO_CAPTURE as u32,
        Permission::Microphone,
    ),
    (
        Media::CEF_MEDIA_PERMISSION_DESKTOP_VIDEO_CAPTURE as u32,
        Permission::ScreenVideo,
    ),
    (
        Media::CEF_MEDIA_PERMISSION_DESKTOP_AUDIO_CAPTURE as u32,
        Permission::ScreenAudio,
    ),
];

/// What each bit of Chromium's prompt asks for. Its prompt for multiple
/// downloads is the download policy's (`crate::downloads`), not here.
const PROMPT: [(u32, Permission); 28] = [
    (
        Prompt::CEF_PERMISSION_TYPE_CAMERA_STREAM as u32,
        Permission::Camera,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_MIC_STREAM as u32,
        Permission::Microphone,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_GEOLOCATION as u32,
        Permission::Geolocation,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_NOTIFICATIONS as u32,
        Permission::Notifications,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_CLIPBOARD as u32,
        Permission::ClipboardRead,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_MIDI_SYSEX as u32,
        Permission::MidiSysex,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_CAMERA_PAN_TILT_ZOOM as u32,
        Permission::CameraPanTiltZoom,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_AR_SESSION as u32,
        Permission::ArSession,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_VR_SESSION as u32,
        Permission::VrSession,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_HAND_TRACKING as u32,
        Permission::HandTracking,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_CAPTURED_SURFACE_CONTROL as u32,
        Permission::CapturedSurfaceControl,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_STORAGE_ACCESS as u32,
        Permission::StorageAccess,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_TOP_LEVEL_STORAGE_ACCESS as u32,
        Permission::TopLevelStorageAccess,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_DISK_QUOTA as u32,
        Permission::DiskQuota,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_LOCAL_FONTS as u32,
        Permission::LocalFonts,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_IDENTITY_PROVIDER as u32,
        Permission::IdentityProvider,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_IDLE_DETECTION as u32,
        Permission::IdleDetection,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_KEYBOARD_LOCK as u32,
        Permission::KeyboardLock,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_POINTER_LOCK as u32,
        Permission::PointerLock,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_PROTECTED_MEDIA_IDENTIFIER as u32,
        Permission::ProtectedMediaIdentifier,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_REGISTER_PROTOCOL_HANDLER as u32,
        Permission::RegisterProtocolHandler,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_WEB_APP_INSTALLATION as u32,
        Permission::WebAppInstallation,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_WINDOW_MANAGEMENT as u32,
        Permission::WindowManagement,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_FILE_SYSTEM_ACCESS as u32,
        Permission::FileSystemAccess,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_LOCAL_NETWORK as u32,
        Permission::LocalNetwork,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_LOCAL_NETWORK_ACCESS_DEPRECATED as u32,
        Permission::LocalNetwork,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_LOOPBACK_NETWORK as u32,
        Permission::LoopbackNetwork,
    ),
    (
        Prompt::CEF_PERMISSION_TYPE_SENSORS as u32,
        Permission::Sensors,
    ),
];

/// The prompt for multiple downloads, which the download policy answers.
const MULTIPLE_DOWNLOADS: u32 = Prompt::CEF_PERMISSION_TYPE_MULTIPLE_DOWNLOADS as u32;

/// The kinds `bits` names by `table`, each once, in the table's order, and
/// the bits the table does not know.
fn read(bits: u32, table: &[(u32, Permission)]) -> (Vec<Permission>, u32) {
    let mut kinds = Vec::new();
    let mut known = 0;
    for &(bit, kind) in table {
        if bits & bit != 0 {
            known |= bit;
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }
    }
    (kinds, bits & !known)
}

/// Gives each request the id its deferred answer finds it by.
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

/// A page asking for a device or a permission, passed to
/// [`App::on_permission`](crate::App::on_permission).
#[derive(Debug)]
pub struct PermissionRequest {
    origin: Origin,
    permissions: Vec<Permission>,
    browser: Option<BrowserId>,
    /// Bits CEF set that Kurogane does not know: a kind a newer CEF added
    unknown: u32,
    ticket: Arc<Ticket>,
}

impl PermissionRequest {
    /// A request for a camera, a microphone or the screen.
    pub(crate) fn media(
        app: &AppHandle,
        origin_url: &str,
        bits: u32,
        browser: Option<BrowserId>,
    ) -> Self {
        Self::new(app, origin_url, read(bits, &MEDIA), browser)
    }

    /// The request of Chromium's prompt.
    pub(crate) fn prompt(
        app: &AppHandle,
        origin_url: &str,
        bits: u32,
        browser: Option<BrowserId>,
    ) -> Self {
        Self::new(
            app,
            origin_url,
            read(bits & !MULTIPLE_DOWNLOADS, &PROMPT),
            browser,
        )
    }

    fn new(
        app: &AppHandle,
        origin_url: &str,
        (permissions, unknown): (Vec<Permission>, u32),
        browser: Option<BrowserId>,
    ) -> Self {
        Self {
            origin: Origin::from_url(origin_url),
            permissions,
            browser,
            unknown,
            ticket: Arc::new(Ticket {
                id: NEXT_REQUEST.fetch_add(1, Ordering::Relaxed),
                browser,
                runtime: app.downgrade(),
                answered: AtomicBool::new(false),
            }),
        }
    }

    /// The origin that asks: the page's, or a frame's inside it when the
    /// frame asks.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    /// What the request asks for, all of it together: a page asking for a
    /// camera and a microphone at once is granted both or neither.
    pub fn permissions(&self) -> &[Permission] {
        &self.permissions
    }

    /// The browser of the page that asks.
    pub fn browser(&self) -> Option<BrowserId> {
        self.browser
    }

    /// A responder that answers this request later, from any thread, once
    /// the hook returns [`PermissionDecision::Later`]: after asking the
    /// user, for example. With any other answer from the hook, the
    /// responder does nothing.
    pub fn responder(&self) -> PermissionResponder {
        PermissionResponder {
            ticket: Arc::clone(&self.ticket),
        }
    }

    pub(crate) fn id(&self) -> u64 {
        self.ticket.id
    }

    fn describe(&self) -> String {
        let kinds: Vec<&str> = self.permissions.iter().map(|kind| kind.name()).collect();
        format!("{} for {}", kinds.join(", "), self.origin)
    }
}

/// Answers a [`PermissionRequest`] the hook left for later
/// ([`PermissionDecision::Later`]), from any thread.
///
/// A request is answered once: the first answer counts. Dropping every
/// responder of a request without answering denies it, and an answer that
/// comes after the page went away grants nothing.
///
/// ```no_run
/// # use kurogane::{App, PermissionDecision};
/// # fn ask_the_user(_: &str) -> bool { true }
/// App::new("./dist")
///     .on_permission(|request, _| {
///         let question = format!("Allow {:?} for {}?", request.permissions(), request.origin());
///         let responder = request.responder();
///         std::thread::spawn(move || {
///             if ask_the_user(&question) {
///                 responder.allow();
///             } else {
///                 responder.deny();
///             }
///         });
///         PermissionDecision::Later
///     })
///     .run_or_exit();
/// ```
#[derive(Debug)]
pub struct PermissionResponder {
    ticket: Arc<Ticket>,
}

impl PermissionResponder {
    /// Grants everything the request asks for.
    pub fn allow(self) {
        self.ticket.answer(true);
    }

    /// Grants nothing.
    pub fn deny(self) {
        self.ticket.answer(false);
    }
}

/// What a request's deferred answer needs: the request's id, the browser
/// that holds its callback, and the runtime, held weakly so a responder an
/// application keeps does not keep the runtime alive. The last holder to
/// go without an answer denies the request.
#[derive(Debug)]
struct Ticket {
    id: u64,
    browser: Option<BrowserId>,
    runtime: Weak<RuntimeServices>,
    answered: AtomicBool,
}

impl Ticket {
    /// Takes the request's one answer: true the first time only.
    fn claim(&self) -> bool {
        !self.answered.swap(true, Ordering::AcqRel)
    }

    /// Brings the answer to the request's callback, on the UI thread, where
    /// the request waits since the hook returned (`hold`).
    fn answer(&self, allow: bool) {
        if !self.claim() {
            return;
        }
        let (Some(browser), Some(app)) = (self.browser, AppHandle::upgrade(&self.runtime)) else {
            return;
        };
        // Nothing reaches CEF once the application is ending; the browser's
        // close takes its requests
        if app.is_ending() {
            return;
        }
        let mut task = AnswerTask::new(app, browser, self.id, allow);
        if post_task(ThreadId::UI, Some(&mut task)) == 0 {
            debug!("[permission] CEF refused the answer to request {}", self.id);
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.answer(false);
    }
}

wrap_task! {
    struct AnswerTask {
        app: AppHandle,
        browser: BrowserId,
        id: u64,
        allow: bool,
    }

    impl Task {
        fn execute(&self) {
            if !self.app.is_ending() {
                answer_held(&self.app, self.browser, self.id, self.allow);
            }
        }
    }
}

/// The answer of [`App::on_permission`](crate::App::on_permission).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum PermissionDecision {
    /// Kurogane's policy: [`Deny`](Self::Deny).
    #[default]
    Default,

    /// Grants everything the request asks for.
    Allow,

    /// Grants nothing.
    Deny,

    /// The [`PermissionResponder`] the hook took with
    /// [`PermissionRequest::responder`] answers later; the page waits until
    /// then. Without a responder kept, the request is denied at once.
    Later,
}

/// What Kurogane does with a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    Allow,
    Deny,
    /// The request waits for its responder (`hold`)
    Later,
}

/// Asks the application's hook about `request`, if `ask` (not for
/// Chromium's own browsers, DevTools' included), and settles what Kurogane
/// does. Runs on CEF's UI thread with no lock held. A hook that panics
/// denies the request, as does a kind Kurogane does not know.
pub(crate) fn decide(app: &AppHandle, request: &PermissionRequest, ask: bool) -> Answer {
    let answer = if request.unknown != 0 {
        warn!(
            "[permission] {} asked for a permission Kurogane does not know ({:#x}); denied",
            request.origin, request.unknown
        );
        Answer::Deny
    } else {
        let hooks = app.hooks();
        let hook = hooks
            .as_deref()
            .and_then(|hooks| hooks.permission.as_ref())
            .filter(|_| ask);
        match hook {
            Some(hook) => match catch_unwind(AssertUnwindSafe(|| hook(request, app))) {
                Ok(decision) => resolve(request, decision),
                Err(_) => {
                    error!("on_permission panicked; {} is denied", request.describe());
                    Answer::Deny
                }
            },
            None if ask => resolve(request, PermissionDecision::Default),
            None => {
                debug!(
                    "[permission] {} denied: a page of Chromium's own",
                    request.describe()
                );
                Answer::Deny
            }
        }
    };
    // Answered now: a responder's answer finds nothing to answer
    if answer != Answer::Later {
        request.ticket.claim();
    }
    answer
}

fn resolve(request: &PermissionRequest, decision: PermissionDecision) -> Answer {
    match decision {
        PermissionDecision::Allow => Answer::Allow,
        PermissionDecision::Deny => Answer::Deny,
        PermissionDecision::Default => {
            warn!(
                "[permission] {} denied: App::on_permission allows nothing by default",
                request.describe()
            );
            Answer::Deny
        }
        // A responder is out, or answered already and on its way
        PermissionDecision::Later
            if Arc::strong_count(&request.ticket) > 1 || !request.ticket.claim() =>
        {
            Answer::Later
        }
        PermissionDecision::Later => {
            warn!(
                "[permission] on_permission answered Later and kept no responder; {} is denied",
                request.describe()
            );
            Answer::Deny
        }
    }
}

/// A request waiting for its responder, with the callback that answers it.
pub(crate) enum Pending {
    /// A camera, a microphone or the screen, granted only while the frame
    /// that asked is still there
    Media {
        callback: MediaAccessCallback,
        requested: u32,
        frame: Option<Frame>,
    },
    /// Chromium's prompt `prompt`
    Prompt {
        callback: PermissionPromptCallback,
        prompt: u64,
    },
}

impl Pending {
    /// Answers the request. Not under the registry's lock: CEF answers a
    /// prompt at once, and calls back into Kurogane as it does.
    pub(crate) fn answer(self, allow: bool) {
        match self {
            Pending::Media {
                callback,
                requested,
                frame,
            } => {
                let there = frame.is_none_or(|frame| frame.is_valid() != 0);
                callback.cont(if allow && there { requested } else { 0 });
            }
            Pending::Prompt { callback, .. } => callback.cont(prompt_result(allow)),
        }
    }
}

/// The answer to Chromium's prompt: Chromium remembers either one for a web
/// site.
pub(crate) fn prompt_result(allow: bool) -> PermissionRequestResult {
    use cef::sys::cef_permission_request_result_t as Outcome;
    PermissionRequestResult::from(if allow {
        Outcome::CEF_PERMISSION_RESULT_ACCEPT
    } else {
        Outcome::CEF_PERMISSION_RESULT_DENY
    })
}

/// The requests of one browser waiting for their responders; they go with
/// the browser, which denies them.
#[derive(Default)]
pub(crate) struct PendingPermissions {
    waiting: Vec<(u64, Pending)>,
}

impl PendingPermissions {
    fn hold(&mut self, id: u64, pending: Pending) {
        self.waiting.push((id, pending));
    }

    fn take(&mut self, id: u64) -> Option<Pending> {
        let index = self
            .waiting
            .iter()
            .position(|(waiting, _)| *waiting == id)?;
        Some(self.waiting.swap_remove(index).1)
    }

    /// Takes the request of Chromium's prompt `prompt`, which Chromium took
    /// down: answered, or gone with its page.
    pub(crate) fn prompt_ended(&mut self, prompt: u64) -> Option<Pending> {
        let index = self
            .waiting
            .iter()
            .position(|(_, pending)| matches!(pending, Pending::Prompt { prompt: ended, .. } if *ended == prompt))?;
        Some(self.waiting.swap_remove(index).1)
    }

    /// Every request still waiting, for the browser's close to deny.
    pub(crate) fn take_all(&mut self) -> Vec<Pending> {
        self.waiting.drain(..).map(|(_, pending)| pending).collect()
    }
}

/// Keeps `pending` in `browser`'s state until its responder answers.
/// Gives it back when there is no such browser to keep it in.
pub(crate) fn hold(
    app: &AppHandle,
    browser: Option<BrowserId>,
    id: u64,
    pending: Pending,
) -> Result<(), Pending> {
    let Some(browser) = browser else {
        return Err(pending);
    };
    // The guard ends with the statement
    match app.registry().browsers.get_mut(browser) {
        Some(state) => {
            state.permissions.hold(id, pending);
            Ok(())
        }
        None => Err(pending),
    }
}

/// Answers request `id` of `browser`, if it still waits. UI thread.
fn answer_held(app: &AppHandle, browser: BrowserId, id: u64, allow: bool) {
    // The guard ends with the statement, before CEF is called
    let pending = app
        .registry()
        .browsers
        .get_mut(browser)
        .and_then(|state| state.permissions.take(id));
    match pending {
        Some(pending) => {
            debug!("[permission] request {id} answered: allow={allow}");
            pending.answer(allow);
        }
        None => debug!("[permission] request {id} answered after it ended"),
    }
}

wrap_task! {
    struct ForgetTask {
        app: AppHandle,
        origin: Origin,
    }

    impl Task {
        fn execute(&self) {
            if !self.app.is_ending() {
                forget(&self.app, &self.origin);
            }
        }
    }
}

/// Forgets what Chromium remembers about `origin`'s permissions, on the UI
/// thread: at once when called there, posted there otherwise.
pub(crate) fn forget_on_ui(app: &AppHandle, origin: &Origin) {
    if app.is_ending() || origin.is_opaque() {
        return;
    }
    if app.on_ui_thread() {
        forget(app, origin);
    } else {
        let mut task = ForgetTask::new(app.clone(), origin.clone());
        if post_task(ThreadId::UI, Some(&mut task)) == 0 {
            debug!("[permission] CEF refused to forget {origin}'s permissions");
        }
    }
}

/// Clears every answer Chromium remembers for `origin`, in the profile of
/// every browser, and its record of prompts dismissed or ignored, from
/// which it would block the site on its own. UI thread.
fn forget(app: &AppHandle, origin: &Origin) {
    let url = CefString::from(origin.to_string().as_str());
    // The guard ends with the statement, before CEF is called
    let browsers: Vec<Browser> = app
        .registry()
        .browsers
        .iter()
        .map(|(_, state)| state.browser.clone())
        .collect();
    let mut contexts: Vec<RequestContext> =
        request_context_get_global_context().into_iter().collect();
    for browser in browsers {
        let Some(mut context) = browser.host().and_then(|host| host.request_context()) else {
            continue;
        };
        if !contexts
            .iter()
            .any(|known| known.is_same(Some(&mut context)) != 0)
        {
            contexts.push(context);
        }
    }
    // Each setting cleared as a website setting, which every permission
    // setting also is: clearing a setting this way takes no CHECK the
    // content settings call takes, which geolocation's second setting fails.
    // The record of dismissed and ignored prompts goes too
    let settings = PROMPT
        .iter()
        .chain(&MEDIA)
        .flat_map(|(_, kind)| kind.remembered_in())
        .chain(&[Setting::CEF_CONTENT_SETTING_TYPE_PERMISSION_AUTOBLOCKER_DATA]);
    for context in &contexts {
        for setting in settings.clone() {
            context.set_website_setting(
                Some(&url),
                Some(&url),
                ContentSettingTypes::from(*setting),
                None,
            );
        }
    }
    debug!(
        "[permission] forgot {origin}'s permissions in {} profiles",
        contexts.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(app: &AppHandle, permissions: Vec<Permission>) -> PermissionRequest {
        PermissionRequest::new(app, "https://example.com/page", (permissions, 0), None)
    }

    #[test]
    fn a_device_request_names_each_device_once() {
        let audio = Media::CEF_MEDIA_PERMISSION_DEVICE_AUDIO_CAPTURE as u32;
        let video = Media::CEF_MEDIA_PERMISSION_DEVICE_VIDEO_CAPTURE as u32;
        let screen = Media::CEF_MEDIA_PERMISSION_DESKTOP_VIDEO_CAPTURE as u32;
        assert_eq!(
            read(audio | video, &MEDIA),
            (vec![Permission::Camera, Permission::Microphone], 0)
        );
        assert_eq!(read(screen, &MEDIA), (vec![Permission::ScreenVideo], 0));
        assert_eq!(
            read(0x40 | audio, &MEDIA),
            (vec![Permission::Microphone], 0x40)
        );
    }

    #[test]
    fn a_prompt_names_its_kinds_and_leaves_multiple_downloads_out() {
        let app = AppHandle::detached();
        let camera = Prompt::CEF_PERMISSION_TYPE_CAMERA_STREAM as u32;
        let mic = Prompt::CEF_PERMISSION_TYPE_MIC_STREAM as u32;
        let request = PermissionRequest::prompt(
            &app,
            "https://example.com/",
            mic | camera | MULTIPLE_DOWNLOADS,
            None,
        );
        assert_eq!(
            request.permissions(),
            &[Permission::Camera, Permission::Microphone]
        );
        assert_eq!(request.unknown, 0);
        assert_eq!(
            request.origin(),
            &Origin::parse("https://example.com").unwrap()
        );
        // Two bits Chromium has for the local network are one kind
        let local = Prompt::CEF_PERMISSION_TYPE_LOCAL_NETWORK as u32
            | Prompt::CEF_PERMISSION_TYPE_LOCAL_NETWORK_ACCESS_DEPRECATED as u32;
        assert_eq!(read(local, &PROMPT), (vec![Permission::LocalNetwork], 0));
        // A kind a newer CEF adds is not known
        assert_eq!(read(1 << 30, &PROMPT), (vec![], 1 << 30));
    }

    #[test]
    fn each_bit_of_a_table_is_one_bit_and_named_once() {
        for table in [&MEDIA[..], &PROMPT[..]] {
            for (index, (bit, _)) in table.iter().enumerate() {
                assert_eq!(bit.count_ones(), 1, "{bit:#x}");
                assert!(
                    table[index + 1..].iter().all(|(other, _)| other != bit),
                    "{bit:#x}"
                );
            }
        }
        assert!(PROMPT.iter().all(|(bit, _)| *bit != MULTIPLE_DOWNLOADS));
    }

    #[test]
    fn default_denies_and_later_needs_a_responder() {
        let app = AppHandle::detached();
        let asked = request(&app, vec![Permission::Camera]);
        assert_eq!(resolve(&asked, PermissionDecision::Allow), Answer::Allow);
        assert_eq!(resolve(&asked, PermissionDecision::Deny), Answer::Deny);
        assert_eq!(resolve(&asked, PermissionDecision::Default), Answer::Deny);
        // No responder kept: denied now, and the request is answered
        assert_eq!(resolve(&asked, PermissionDecision::Later), Answer::Deny);
        assert!(!asked.ticket.claim());

        let kept = request(&app, vec![Permission::Camera]);
        let responder = kept.responder();
        assert_eq!(resolve(&kept, PermissionDecision::Later), Answer::Later);
        // Answered once; the first answer counts
        responder.allow();
        assert!(!kept.ticket.claim());

        // A responder that answered before the hook returned: its answer is
        // on its way
        let early = request(&app, vec![Permission::Microphone]);
        early.responder().deny();
        assert_eq!(resolve(&early, PermissionDecision::Later), Answer::Later);
    }

    #[test]
    fn an_answer_from_the_hook_leaves_the_responder_nothing_to_answer() {
        let app = AppHandle::detached();
        let asked = request(&app, vec![Permission::Geolocation]);
        let responder = asked.responder();
        assert_eq!(decide(&app, &asked, true), Answer::Deny);
        assert!(!responder.ticket.claim());
    }

    #[test]
    fn a_kind_kurogane_does_not_know_is_denied() {
        let app = AppHandle::detached();
        let asked = PermissionRequest::prompt(&app, "https://example.com/", 1 << 30, None);
        assert_eq!(decide(&app, &asked, true), Answer::Deny);
    }

    #[test]
    fn only_kinds_chromium_maps_to_no_setting_name_none() {
        let mut unremembered = vec![
            Permission::ScreenVideo,
            Permission::ScreenAudio,
            Permission::DiskQuota,
            Permission::IdentityProvider,
            Permission::RegisterProtocolHandler,
            Permission::FileSystemAccess,
        ];
        if cfg!(not(target_os = "windows")) {
            unremembered.push(Permission::ProtectedMediaIdentifier);
        }
        for (_, kind) in PROMPT.iter().chain(&MEDIA) {
            assert_eq!(
                kind.remembered_in().is_empty(),
                unremembered.contains(kind),
                "{kind}"
            );
        }
    }
}
