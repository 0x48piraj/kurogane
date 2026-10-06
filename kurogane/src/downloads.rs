//! What happens when a page downloads a file: a link the server answers with
//! an attachment, a link with a `download` attribute, a `blob:` or `data:`
//! export.
//!
//! Chromium would save every download silently into the user's Downloads
//! folder, with no prompt and nothing on screen (the download bubble lives
//! in the toolbar Kurogane's windows do not have), whichever page asked. By
//! default Kurogane asks the user instead, with the system's Save As
//! dialog: nothing reaches the disk unless the user picked the place. The
//! application's [`App::on_download`](crate::App::on_download) hook can
//! choose the place itself, or refuse.
//!
//! Chromium's own download UI is kept out of the way: its prompt for a
//! page's further downloads is granted (`grants_prompt`), since each of
//! them still comes here, and its bubble of finished downloads is turned
//! off at startup.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use tetsu::BeforeDownloadCallback;
use tracing::{debug, error, warn};

use crate::acl::Origin;
use crate::browser_registry::BrowserId;
use crate::runtime::AppHandle;

/// A download about to start, passed to
/// [`App::on_download`](crate::App::on_download).
#[derive(Debug, Clone)]
pub struct DownloadRequest {
    url: String,
    origin: Origin,
    suggested_name: String,
    mime_type: String,
    browser: Option<BrowserId>,
}

impl DownloadRequest {
    pub(crate) fn new(
        url: String,
        page_url: &str,
        suggested_name: String,
        mime_type: String,
        browser: Option<BrowserId>,
    ) -> Self {
        Self {
            url,
            origin: Origin::from_url(page_url),
            suggested_name,
            mime_type,
            browser,
        }
    }

    /// The URL the file comes from.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The origin of the page the download comes from: the document the
    /// window shows, which a frame inside it counts as.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    /// The file name the server or the page suggests, a name only, without
    /// a directory. Chromium has already made it safe for the platform.
    pub fn suggested_name(&self) -> &str {
        &self.suggested_name
    }

    /// The file's MIME type, empty when it is unknown.
    pub fn mime_type(&self) -> &str {
        &self.mime_type
    }

    /// The browser the download comes from.
    pub fn browser(&self) -> Option<BrowserId> {
        self.browser
    }
}

/// The answer of [`App::on_download`](crate::App::on_download).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum DownloadDecision {
    /// Kurogane's policy: [`Prompt`](Self::Prompt).
    #[default]
    Default,

    /// Asks the user with the system's Save As dialog, the suggested name
    /// filled in; cancelling it saves nothing. A window shows one dialog at
    /// a time, so a download may wait for the one before it.
    Prompt,

    /// Saves the file at this absolute path, which names the file itself,
    /// without asking, replacing a file already there. The folder is
    /// created when missing; a place whose folder cannot be made, and a
    /// relative path, save nothing.
    SaveTo(PathBuf),

    /// Saves nothing.
    Deny,
}

/// What Kurogane does with a download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Answer {
    Prompt,
    SaveTo(PathBuf),
    Refuse,
}

/// Asks the application's hook, if `ask` (not for Chromium's own browsers,
/// DevTools' included), then applies its answer. Runs on CEF's UI thread
/// with no lock held. A hook that panics refuses the download.
pub(crate) fn decide(app: &AppHandle, request: &DownloadRequest, ask: bool) -> Answer {
    let hooks = app.hooks();
    let hook = hooks
        .as_deref()
        .and_then(|hooks| hooks.download.as_ref())
        .filter(|_| ask);
    let decision = match hook {
        Some(hook) => match catch_unwind(AssertUnwindSafe(|| hook(request, app))) {
            Ok(decision) => decision,
            Err(_) => {
                error!(
                    "on_download panicked; the download of {} is refused",
                    request.url
                );
                return Answer::Refuse;
            }
        },
        None => DownloadDecision::Default,
    };
    let answer = match resolve(decision) {
        Answer::SaveTo(path) => match make_room(&path) {
            Ok(()) => Answer::SaveTo(path),
            Err(error) => {
                warn!(
                    "on_download's place {} has no folder ({error}); nothing is saved",
                    path.display()
                );
                Answer::Refuse
            }
        },
        answer => answer,
    };
    debug!(
        "download of {} ({}): {answer:?}",
        request.url, request.suggested_name
    );
    answer
}

/// Creates the folder `path` goes into. CEF creates it too, but saves into
/// the temporary folder when it cannot: a place nobody chose.
fn make_room(path: &Path) -> std::io::Result<()> {
    match path.parent() {
        Some(folder) => std::fs::create_dir_all(folder),
        None => Ok(()),
    }
}

/// A download waiting to ask the user where to save it: what continues it,
/// and the file name to suggest.
pub(crate) struct SavePrompt {
    pub proceed: BeforeDownloadCallback,
    pub name: String,
}

/// One browser's downloads that Kurogane holds: those asking the user where
/// to save them, one Save As dialog at a time, and those refused, until CEF
/// reports them cancelled (only an update of a download can cancel it).
///
/// CEF shows one file dialog per browser. It cancels a second request at
/// once, and that also cuts off the dialog already open: the user's answer
/// to it is lost and its download never goes on. So a download waits until
/// the dialog before it is answered. Kurogane opens each dialog itself and
/// continues the download with the chosen place: CEF's own dialog for a
/// download reads the page that started it, which is gone once that page
/// navigated away, and CEF then crashes. A dialog still belongs to the
/// document it opened over, and Chromium drops its answer once that
/// document is replaced, so the user is then asked again. `P` is what a
/// waiting download needs to ask, `K` what cancels a download.
pub(crate) struct Downloads<P, K> {
    /// The download whose dialog is open
    open: Option<u32>,
    /// The downloads waiting to ask, in order
    waiting: VecDeque<(u32, P)>,
    /// What cancels each download that asks or waits, from its latest
    /// update
    cancels: Vec<(u32, K)>,
    /// Downloads to cancel at their next update
    refused: Vec<u32>,
}

impl<P, K> Default for Downloads<P, K> {
    fn default() -> Self {
        Self {
            open: None,
            waiting: VecDeque::new(),
            cancels: Vec::new(),
            refused: Vec::new(),
        }
    }
}

impl<P, K> Downloads<P, K> {
    /// Whether a download's Save As dialog is open: Kurogane's own, which
    /// the application's file dialog hook is never asked about.
    pub(crate) fn is_asking(&self) -> bool {
        self.open.is_some()
    }

    /// The download `id` is to ask the user: returns `prompt` when its
    /// dialog may show now, or keeps it until the dialog before it is
    /// answered.
    pub(crate) fn ask(&mut self, id: u32, prompt: P) -> Option<P> {
        if self.open.is_some() {
            self.waiting.push_back((id, prompt));
            return None;
        }
        self.open = Some(id);
        Some(prompt)
    }

    /// The download `id` is refused: its next update cancels it.
    pub(crate) fn refuse(&mut self, id: u32) {
        if !self.refused.contains(&id) {
            self.refused.push(id);
        }
    }

    /// The download `id` was updated, `cancel` cancelling it: returns
    /// `cancel` when the download was refused, to cancel it now. Otherwise
    /// keeps `cancel` while the download asks or waits. A download that
    /// `ended` is forgotten, a waiting one leaving the queue; an open one
    /// keeps its dialog until that is answered.
    pub(crate) fn update(&mut self, id: u32, cancel: Option<K>, ended: bool) -> Option<K> {
        self.cancels.retain(|(download, _)| *download != id);
        if ended {
            self.refused.retain(|download| *download != id);
            self.waiting.retain(|(download, _)| *download != id);
            return None;
        }
        if self.refused.contains(&id) {
            return cancel;
        }
        let asks =
            self.open == Some(id) || self.waiting.iter().any(|(download, _)| *download == id);
        if let Some(cancel) = cancel.filter(|_| asks) {
            self.cancels.push((id, cancel));
        }
        None
    }

    /// The dialog of download `id` was answered, `saved` with a place:
    /// returns what cancels the download now when the user dismissed it
    /// (or refuses it, for its next update, when nothing can yet), and the
    /// next waiting download, whose dialog may show now.
    pub(crate) fn answered(&mut self, id: u32, saved: bool) -> (Option<K>, Option<(u32, P)>) {
        let cancel = self
            .cancels
            .iter()
            .position(|(download, _)| *download == id)
            .map(|index| self.cancels.remove(index).1)
            .filter(|_| !saved);
        if !saved && cancel.is_none() {
            self.refuse(id);
        }
        if self.open != Some(id) {
            return (cancel, None);
        }
        self.open = None;
        let next = self.waiting.pop_front();
        self.open = next.as_ref().map(|(download, _)| *download);
        (cancel, next)
    }
}
/// Chromium's permission to download several files from one page without a
/// click for each, which it asks the user for in a prompt of its own.
const MULTIPLE_DOWNLOADS: u32 =
    tetsu::sys::cef_permission_request_types_t::CEF_PERMISSION_TYPE_MULTIPLE_DOWNLOADS as u32;

/// Whether Kurogane grants a permission prompt itself, without showing it:
/// one asking for multiple downloads alone. Every download still passes
/// [`decide`], so Chromium's prompt would only ask a second time. A prompt
/// that asks for anything else as well keeps CEF's default.
pub(crate) fn grants_prompt(requested_permissions: u32) -> bool {
    requested_permissions == MULTIPLE_DOWNLOADS
}

fn resolve(decision: DownloadDecision) -> Answer {
    match decision {
        DownloadDecision::Default | DownloadDecision::Prompt => Answer::Prompt,
        DownloadDecision::SaveTo(path) if path.is_absolute() => Answer::SaveTo(path),
        DownloadDecision::SaveTo(path) => {
            warn!(
                "on_download answered a relative path ({}); nothing is saved",
                path.display()
            );
            Answer::Refuse
        }
        DownloadDecision::Deny => Answer::Refuse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_default_the_user_is_asked_and_a_place_must_be_absolute() {
        assert_eq!(resolve(DownloadDecision::Default), Answer::Prompt);
        assert_eq!(resolve(DownloadDecision::Prompt), Answer::Prompt);
        assert_eq!(resolve(DownloadDecision::Deny), Answer::Refuse);
        let absolute = std::env::temp_dir().join("report.csv");
        assert_eq!(
            resolve(DownloadDecision::SaveTo(absolute.clone())),
            Answer::SaveTo(absolute)
        );
        assert_eq!(
            resolve(DownloadDecision::SaveTo(PathBuf::from(
                "exports/report.csv"
            ))),
            Answer::Refuse
        );
    }

    #[test]
    fn one_dialog_shows_at_a_time_and_each_download_gets_its_turn() {
        let mut held = Downloads::default();
        assert_eq!(held.ask(1, "first"), Some("first"));
        assert_eq!(held.ask(2, "second"), None);
        assert_eq!(held.ask(3, "third"), None);
        // What cancels a download is kept while it asks or waits, never
        // for one that does not
        assert_eq!(held.update(1, Some("cancel 1"), false), None);
        assert_eq!(held.update(3, Some("cancel 3, older"), false), None);
        assert_eq!(held.update(3, Some("cancel 3"), false), None);
        assert_eq!(held.update(7, Some("cancel 7"), false), None);
        // The second ends while it waits: it never asks
        assert_eq!(held.update(2, Some("cancel 2"), true), None);
        // The first is dismissed: cancelled now, and the third asks
        assert_eq!(
            held.answered(1, false),
            (Some("cancel 1"), Some((3, "third")))
        );
        // The third is saved: nothing to cancel, nothing waits
        assert_eq!(held.answered(3, true), (None, None));
        assert_eq!(held.answered(7, true), (None, None));
        // Nothing open: the next asks at once; dismissed before any update,
        // it is refused and its next update cancels it
        assert_eq!(held.ask(4, "fourth"), Some("fourth"));
        assert_eq!(held.answered(4, false), (None, None));
        assert_eq!(held.update(4, Some("cancel 4"), false), Some("cancel 4"));
        assert_eq!(held.update(4, None, true), None);
        assert_eq!(held.update(4, Some("cancel 4 again"), false), None);
    }

    #[test]
    fn a_refused_download_is_cancelled_by_its_next_update() {
        let mut held: Downloads<&str, &str> = Downloads::default();
        held.refuse(5);
        assert_eq!(held.update(5, None, false), None);
        assert_eq!(held.update(5, Some("cancel 5"), false), Some("cancel 5"));
        // Cancelled: forgotten
        assert_eq!(held.update(5, None, true), None);
        assert_eq!(held.update(5, Some("cancel 5"), false), None);
    }
    #[test]
    fn a_place_s_folder_is_made_or_the_download_refused() {
        let base = std::env::temp_dir().join(format!("kurogane-make-room-{}", std::process::id()));
        let place = base.join("new").join("deeper").join("report.csv");
        assert!(make_room(&place).is_ok());
        assert!(base.join("new").join("deeper").is_dir());
        // A file where the folder would be: CEF would fall back to the
        // temporary folder, Kurogane refuses
        let blocker = base.join("blocker");
        std::fs::write(&blocker, b"").unwrap();
        assert!(make_room(&blocker.join("report.csv")).is_err());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn only_the_multiple_downloads_prompt_is_granted() {
        use tetsu::sys::cef_permission_request_types_t as Permission;
        let camera = Permission::CEF_PERMISSION_TYPE_CAMERA_STREAM as u32;
        assert!(grants_prompt(MULTIPLE_DOWNLOADS));
        assert!(!grants_prompt(camera));
        assert!(!grants_prompt(MULTIPLE_DOWNLOADS | camera));
        assert!(!grants_prompt(0));
    }

    #[test]
    fn the_origin_is_the_page_s() {
        let request = DownloadRequest::new(
            "https://cdn.example/file.zip".into(),
            "app://app/index.html",
            "file.zip".into(),
            "application/zip".into(),
            None,
        );
        assert_eq!(request.origin(), &Origin::parse("app://app").unwrap());
    }
}
