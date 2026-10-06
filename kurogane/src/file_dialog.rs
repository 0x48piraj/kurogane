//! The file chooser a page opens: `<input type=file>` and the File System
//! Access pickers (`showOpenFilePicker`, `showSaveFilePicker`,
//! `showDirectoryPicker`).
//!
//! CEF asks before it shows one (OnFileDialog). The application's
//! [`App::on_file_dialog`](crate::App::on_file_dialog) hook may answer with
//! files of its choosing, now or later through a [`FileDialogResponder`],
//! cancel the dialog, or leave it to Chromium's own. Kurogane's own Save As
//! dialog for a download (crate::downloads) is never asked about, nor are
//! the dialogs of DevTools and of Chromium's own browsers.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use tetsu::*;
use tracing::{debug, error, warn};

use crate::acl::Origin;
use crate::browser_registry::BrowserId;
use crate::runtime::{AppHandle, RuntimeServices};

/// What kind of file chooser a page opens ([`FileDialogRequest::kind`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FileDialogKind {
    /// One existing file.
    Open,
    /// One or more existing files (`<input type=file multiple>`).
    OpenMultiple,
    /// A folder (`<input type=file webkitdirectory>`,
    /// `showDirectoryPicker`).
    OpenFolder,
    /// A file to save to, which may not exist yet (`showSaveFilePicker`).
    Save,
}

impl FileDialogKind {
    pub(crate) fn from_cef(mode: tetsu::FileDialogMode) -> Option<Self> {
        if mode == tetsu::FileDialogMode::OPEN {
            Some(Self::Open)
        } else if mode == tetsu::FileDialogMode::OPEN_MULTIPLE {
            Some(Self::OpenMultiple)
        } else if mode == tetsu::FileDialogMode::OPEN_FOLDER {
            Some(Self::OpenFolder)
        } else if mode == tetsu::FileDialogMode::SAVE {
            Some(Self::Save)
        } else {
            None
        }
    }
}

/// Gives each request the id its deferred answer finds it by.
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

/// A page opening a file chooser, passed to
/// [`App::on_file_dialog`](crate::App::on_file_dialog).
#[derive(Debug)]
pub struct FileDialogRequest {
    kind: FileDialogKind,
    title: String,
    default_path: Option<PathBuf>,
    accept: Vec<String>,
    origin: Origin,
    browser: Option<BrowserId>,
    ticket: Arc<Ticket>,
}

impl FileDialogRequest {
    pub(crate) fn new(
        app: &AppHandle,
        kind: FileDialogKind,
        title: String,
        default_path: String,
        accept: Vec<String>,
        origin_url: &str,
        browser: Option<BrowserId>,
    ) -> Self {
        Self {
            kind,
            title,
            default_path: (!default_path.is_empty()).then(|| PathBuf::from(default_path)),
            accept,
            origin: Origin::from_url(origin_url),
            browser,
            ticket: Arc::new(Ticket {
                id: NEXT_REQUEST.fetch_add(1, Ordering::Relaxed),
                browser,
                runtime: app.downgrade(),
                answered: AtomicBool::new(false),
            }),
        }
    }

    /// What the page asks to choose.
    pub fn kind(&self) -> FileDialogKind {
        self.kind
    }

    /// The dialog's title as the page gives it; empty for the system's own.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The file name or path the dialog would start at, if any: a picker's
    /// suggested name for saving.
    pub fn default_path(&self) -> Option<&Path> {
        self.default_path.as_deref()
    }

    /// What the page accepts, as it wrote it: MIME types (`image/*`,
    /// `text/plain`) and extensions (`.png`). Empty when it accepts any
    /// file.
    pub fn accept(&self) -> &[String] {
        &self.accept
    }

    /// The origin of the page that opens the dialog: the frame that has the
    /// focus, the one whose control the user activated.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    /// The browser of the page.
    pub fn browser(&self) -> Option<BrowserId> {
        self.browser
    }

    /// A responder that answers this request later, from any thread, once
    /// the hook returns [`FileDialogDecision::Later`]: after the
    /// application's own picker closed, for example. With any other answer
    /// from the hook, the responder does nothing.
    pub fn responder(&self) -> FileDialogResponder {
        FileDialogResponder {
            ticket: Arc::clone(&self.ticket),
        }
    }

    fn describe(&self) -> String {
        format!("{:?} dialog for {}", self.kind, self.origin)
    }
}

/// The answer of [`App::on_file_dialog`](crate::App::on_file_dialog).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum FileDialogDecision {
    /// Chromium's own dialog, the system's file chooser.
    #[default]
    Default,

    /// No dialog: the page is told the user cancelled.
    Cancel,

    /// These files, as if the user chose them: the page reads them. An
    /// [`Open`](FileDialogKind::Open) or [`Save`](FileDialogKind::Save)
    /// dialog takes the first, an
    /// [`OpenFolder`](FileDialogKind::OpenFolder) one a folder. No file is
    /// [`Cancel`](Self::Cancel).
    Files(Vec<PathBuf>),

    /// The [`FileDialogResponder`] the hook took with
    /// [`FileDialogRequest::responder`] answers later; the page waits until
    /// then. Without a responder kept, the dialog is cancelled at once.
    Later,
}

/// Answers a [`FileDialogRequest`] the hook left for later
/// ([`FileDialogDecision::Later`]), from any thread.
///
/// A request is answered once: the first answer counts. Dropping every
/// responder of a request without answering cancels it, and an answer that
/// comes after the page went away gives it nothing.
///
/// ```no_run
/// # use kurogane::{App, FileDialogDecision};
/// # fn pick_files() -> Option<Vec<std::path::PathBuf>> { None }
/// App::new("./dist")
///     .on_file_dialog(|request, _| {
///         let responder = request.responder();
///         std::thread::spawn(move || match pick_files() {
///             Some(files) => responder.select(files),
///             None => responder.cancel(),
///         });
///         FileDialogDecision::Later
///     })
///     .run_or_exit();
/// ```
#[derive(Debug)]
pub struct FileDialogResponder {
    ticket: Arc<Ticket>,
}

impl FileDialogResponder {
    /// Gives the page `files`, as [`FileDialogDecision::Files`] does.
    pub fn select(self, files: Vec<PathBuf>) {
        self.ticket.answer(files);
    }

    /// Tells the page the user cancelled.
    pub fn cancel(self) {
        self.ticket.answer(Vec::new());
    }
}

/// What a request's deferred answer needs: the request's id, the browser
/// that holds its callback, and the runtime, held weakly so a responder an
/// application keeps does not keep the runtime alive. The last holder to go
/// without an answer cancels the request.
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
    /// the request waits since the hook returned (`hold`). No file cancels.
    fn answer(&self, files: Vec<PathBuf>) {
        if !self.claim() {
            return;
        }
        let (Some(browser), Some(app)) = (self.browser, AppHandle::upgrade(&self.runtime)) else {
            return;
        };
        // Nothing reaches CEF once the application is ending; the browser's
        // close takes its dialogs
        if app.is_ending() {
            return;
        }
        let mut task = AnswerTask::new(app, browser, self.id, files);
        if post_task(ThreadId::UI, Some(&mut task)) == 0 {
            debug!(
                "[file dialog] CEF refused the answer to request {}",
                self.id
            );
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.answer(Vec::new());
    }
}

wrap_task! {
    struct AnswerTask {
        app: AppHandle,
        browser: BrowserId,
        id: u64,
        files: Vec<PathBuf>,
    }

    impl Task {
        fn execute(&self) {
            if !self.app.is_ending() {
                answer_held(&self.app, self.browser, self.id, &self.files);
            }
        }
    }
}

/// What Kurogane does with a dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Answer {
    /// Chromium's own dialog
    Default,
    /// These files; none cancels
    Files(Vec<PathBuf>),
    /// The dialog waits for its responder (`hold`)
    Later,
}

/// Asks the application's hook about `request`, and settles what Kurogane
/// does. Runs on CEF's UI thread with no lock held. A hook that panics
/// cancels the dialog: the page gets no file an error might have chosen.
pub(crate) fn decide(app: &AppHandle, request: &FileDialogRequest) -> Answer {
    let hooks = app.hooks();
    let Some(hook) = hooks
        .as_deref()
        .and_then(|hooks| hooks.file_dialog.as_ref())
    else {
        request.ticket.claim();
        return Answer::Default;
    };
    let answer = match catch_unwind(AssertUnwindSafe(|| hook(request, app))) {
        Ok(decision) => resolve(request, decision),
        Err(_) => {
            error!(
                "on_file_dialog panicked; the {} is cancelled",
                request.describe()
            );
            Answer::Files(Vec::new())
        }
    };
    // Answered now: a responder's answer finds nothing to answer
    if answer != Answer::Later {
        request.ticket.claim();
    }
    answer
}

fn resolve(request: &FileDialogRequest, decision: FileDialogDecision) -> Answer {
    match decision {
        FileDialogDecision::Default => Answer::Default,
        FileDialogDecision::Cancel => Answer::Files(Vec::new()),
        FileDialogDecision::Files(files) => Answer::Files(files),
        // A responder is out, or answered already and on its way
        FileDialogDecision::Later
            if Arc::strong_count(&request.ticket) > 1 || !request.ticket.claim() =>
        {
            Answer::Later
        }
        FileDialogDecision::Later => {
            warn!(
                "[file dialog] on_file_dialog answered Later and kept no responder; the {} is cancelled",
                request.describe()
            );
            Answer::Files(Vec::new())
        }
    }
}

/// Answers `callback` with `files`; none cancels. Not under the registry's
/// lock: CEF may call back into Kurogane as it answers.
pub(crate) fn complete(callback: &FileDialogCallback, files: &[PathBuf]) {
    if files.is_empty() {
        callback.cancel();
        return;
    }
    let mut paths = CefStringList::new();
    for file in files {
        paths.append(&file.to_string_lossy());
    }
    callback.cont(Some(&mut paths));
}

/// The dialogs of one browser waiting for their responders; they go with
/// the browser, whose close cancels them.
#[derive(Default)]
pub(crate) struct PendingFileDialogs {
    waiting: Vec<(u64, FileDialogCallback)>,
}

impl PendingFileDialogs {
    fn hold(&mut self, id: u64, callback: FileDialogCallback) {
        self.waiting.push((id, callback));
    }

    fn take(&mut self, id: u64) -> Option<FileDialogCallback> {
        let index = self
            .waiting
            .iter()
            .position(|(waiting, _)| *waiting == id)?;
        Some(self.waiting.swap_remove(index).1)
    }

    /// Every dialog still waiting, for the browser's close to cancel.
    pub(crate) fn take_all(&mut self) -> Vec<FileDialogCallback> {
        self.waiting
            .drain(..)
            .map(|(_, callback)| callback)
            .collect()
    }
}

/// Keeps `callback` in `browser`'s state until request `request`'s
/// responder answers. Gives it back when there is no such browser.
pub(crate) fn hold(
    app: &AppHandle,
    request: &FileDialogRequest,
    callback: FileDialogCallback,
) -> Result<(), FileDialogCallback> {
    let Some(browser) = request.browser else {
        return Err(callback);
    };
    // The guard ends with the statement
    match app.registry().browsers.get_mut(browser) {
        Some(state) => {
            state.file_dialogs.hold(request.ticket.id, callback);
            Ok(())
        }
        None => Err(callback),
    }
}

/// Answers request `id` of `browser`, if it still waits. UI thread.
fn answer_held(app: &AppHandle, browser: BrowserId, id: u64, files: &[PathBuf]) {
    // The guard ends with the statement, before CEF is called
    let callback = app
        .registry()
        .browsers
        .get_mut(browser)
        .and_then(|state| state.file_dialogs.take(id));
    match callback {
        Some(callback) => {
            debug!(
                "[file dialog] request {id} answered: {} file(s)",
                files.len()
            );
            complete(&callback, files);
        }
        None => debug!("[file dialog] request {id} answered after it ended"),
    }
}

/// The strings of a list CEF lends a callback. The list is borrowed: the
/// copy read here does not free it.
pub(crate) fn lent_strings(list: Option<&mut CefStringList>) -> Vec<String> {
    list.map(|list| {
        let raw: *mut sys::_cef_string_list_t = list.into();
        CefStringList::from(raw).into_iter().collect()
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(app: &AppHandle) -> FileDialogRequest {
        FileDialogRequest::new(
            app,
            FileDialogKind::Open,
            String::new(),
            String::new(),
            vec![".txt".into()],
            "app://app/index.html",
            None,
        )
    }

    #[test]
    fn each_answer_settles_what_the_page_gets() {
        let app = AppHandle::detached();
        let request = request(&app);
        assert_eq!(
            resolve(&request, FileDialogDecision::Default),
            Answer::Default
        );
        assert_eq!(
            resolve(&request, FileDialogDecision::Cancel),
            Answer::Files(Vec::new())
        );
        let chosen = vec![PathBuf::from("/tmp/notes.txt")];
        assert_eq!(
            resolve(&request, FileDialogDecision::Files(chosen.clone())),
            Answer::Files(chosen)
        );
    }

    #[test]
    fn later_without_a_responder_cancels_and_with_one_waits() {
        let app = AppHandle::detached();
        let alone = request(&app);
        assert_eq!(
            resolve(&alone, FileDialogDecision::Later),
            Answer::Files(Vec::new())
        );

        let kept = request(&app);
        let responder = kept.responder();
        assert_eq!(resolve(&kept, FileDialogDecision::Later), Answer::Later);
        // A detached runtime has no CEF: the answer goes nowhere
        responder.cancel();
    }

    #[test]
    fn a_request_reports_what_the_page_asked() {
        let app = AppHandle::detached();
        let request = FileDialogRequest::new(
            &app,
            FileDialogKind::Save,
            "Export".into(),
            "report.csv".into(),
            vec!["text/csv".into(), ".csv".into()],
            "https://example.com/page",
            None,
        );
        assert_eq!(request.kind(), FileDialogKind::Save);
        assert_eq!(request.title(), "Export");
        assert_eq!(request.default_path(), Some(Path::new("report.csv")));
        assert_eq!(request.accept(), ["text/csv", ".csv"]);
        assert_eq!(request.origin().to_string(), "https://example.com");
        let blank = super::tests::request(&app);
        assert_eq!(blank.default_path(), None);
        assert_eq!(blank.title(), "");
    }
}
