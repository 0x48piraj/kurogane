//! A drag from another application entering a browser embedded in the
//! application's own window: files from a file manager, a link, selected
//! text.
//!
//! CEF asks as it enters (OnDragEnter), before the page sees a `dragenter`,
//! and only for an Alloy-style browser, an embedded one: into a Chrome-style
//! Views window, Kurogane's own, it asks nothing and the page gets the drop
//! (measured on Windows, CEF 150).
//! The application's [`App::on_drag_enter`](crate::App::on_drag_enter) hook
//! may refuse it, and sees the paths of the files dragged, which the page
//! never does: a page gets each dropped file's contents and name only. The
//! drags of DevTools and of Chromium's own browsers are not asked about.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use tracing::{debug, error};

use crate::acl::Origin;
use crate::browser_registry::BrowserId;
use crate::runtime::AppHandle;

/// A drag entering a window, passed to
/// [`App::on_drag_enter`](crate::App::on_drag_enter).
#[derive(Debug, Clone)]
pub struct DragEnter {
    files: Vec<PathBuf>,
    link_url: Option<String>,
    text: Option<String>,
    origin: Origin,
    browser: Option<BrowserId>,
}

impl DragEnter {
    pub(crate) fn new(
        files: Vec<PathBuf>,
        link_url: Option<String>,
        text: Option<String>,
        page_url: &str,
        browser: Option<BrowserId>,
    ) -> Self {
        Self {
            files,
            link_url,
            text,
            origin: Origin::from_url(page_url),
            browser,
        }
    }

    /// The paths of the files dragged, as the system gives them; empty for
    /// a drag of no file.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// The link dragged, if it is one.
    pub fn link_url(&self) -> Option<&str> {
        self.link_url.as_deref()
    }

    /// The text dragged, if it is some.
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// The origin of the page the drag enters: the window's own page.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    /// The browser the drag enters.
    pub fn browser(&self) -> Option<BrowserId> {
        self.browser
    }
}

/// The answer of [`App::on_drag_enter`](crate::App::on_drag_enter).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum DragDecision {
    /// Kurogane's policy: the drag goes on, as [`Allow`](Self::Allow).
    #[default]
    Default,

    /// The drag goes on: the page sees it, and may take its drop.
    Allow,

    /// The drag stops at the window: the page sees nothing of it and the
    /// pointer shows it cannot drop there.
    Refuse,
}

/// Asks the application's hook about `drag`: whether it is refused. Runs
/// on CEF's UI thread with no lock held. A hook that panics refuses the
/// drag: no file reaches the page an error might have kept out.
pub(crate) fn refused(app: &AppHandle, drag: &DragEnter) -> bool {
    let hooks = app.hooks();
    let Some(hook) = hooks.as_deref().and_then(|hooks| hooks.drag_enter.as_ref()) else {
        return false;
    };
    match catch_unwind(AssertUnwindSafe(|| hook(drag, app))) {
        Ok(decision) => {
            debug!(
                "[drag] {} file(s) into {}: {decision:?}",
                drag.files.len(),
                drag.origin
            );
            decision == DragDecision::Refuse
        }
        Err(_) => {
            error!("on_drag_enter panicked; the drag is refused");
            true
        }
    }
}

/// `path` unless it is empty.
pub(crate) fn non_empty(path: String) -> Option<PathBuf> {
    (!path.is_empty()).then(|| Path::new(&path).to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_reports_what_it_carries_and_where_it_enters() {
        let drag = DragEnter::new(
            vec![PathBuf::from("/home/me/notes.txt")],
            None,
            None,
            "app://app/index.html",
            None,
        );
        assert_eq!(drag.files(), [PathBuf::from("/home/me/notes.txt")]);
        assert_eq!(drag.link_url(), None);
        assert_eq!(drag.origin().to_string(), "app://app");
    }

    #[test]
    fn without_a_hook_nothing_is_refused() {
        let app = AppHandle::detached();
        let drag = DragEnter::new(Vec::new(), Some("https://docs.rs/".into()), None, "", None);
        assert!(!refused(&app, &drag));
    }

    #[test]
    fn an_empty_path_is_no_file() {
        assert_eq!(non_empty(String::new()), None);
        assert_eq!(non_empty("/a".into()), Some(PathBuf::from("/a")));
    }
}
