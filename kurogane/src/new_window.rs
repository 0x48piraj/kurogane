//! What happens when a page asks for a window of its own: `window.open`, a
//! `target=_blank` link, a form that targets a new window, a link clicked
//! into a new tab or window (Ctrl or Cmd, the middle button, Shift).
//!
//! CEF asks before it creates the window. The application's
//! [`App::on_new_window`](crate::App::on_new_window) hook answers first;
//! Kurogane's policy answers whatever the hook leaves to it. A page of the
//! application's own origin gets an application window, a web link the user
//! clicked goes to the system browser, and every other request is refused,
//! so a page the application never chose cannot get a window of its own.

use std::panic::{AssertUnwindSafe, catch_unwind};

use tracing::{debug, error};

use crate::acl::Origin;
use crate::destination::{Outcome, document_origin, external_or_refused, is_chromium_page};
use crate::runtime::AppHandle;

/// A page's request for a new window, passed to
/// [`App::on_new_window`](crate::App::on_new_window).
#[derive(Debug, Clone)]
pub struct NewWindowRequest {
    url: String,
    origin: Origin,
    opener_origin: Origin,
    user_gesture: bool,
}

impl NewWindowRequest {
    /// The request of a page at `opener_url` for a window showing `url`.
    pub(crate) fn new(url: String, opener_url: &str, user_gesture: bool) -> Self {
        let opener_origin = Origin::from_url(opener_url);
        let origin = document_origin(&url, &opener_origin);
        Self {
            url,
            origin,
            opener_origin,
            user_gesture,
        }
    }

    /// The URL the window would load, as Chromium resolved it. Empty or
    /// `about:blank` when the page fills the window in itself.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The origin of the document the window would show. A window the page
    /// fills in itself has the page's origin, and a `blob:` URL the origin
    /// of the page that made it.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    /// The origin of the page that asked.
    pub fn opener_origin(&self) -> &Origin {
        &self.opener_origin
    }

    /// Whether the user asked, by clicking a link or a button, rather than a
    /// script on its own.
    pub fn user_gesture(&self) -> bool {
        self.user_gesture
    }
}

/// The answer of [`App::on_new_window`](crate::App::on_new_window).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum NewWindowDecision {
    /// Kurogane's policy decides: a page of the application's own origin
    /// gets a window, an `http` or `https` link the user clicked goes to
    /// the system browser, and anything else is refused.
    #[default]
    Default,

    /// Opens the page in an application window, whatever its origin. The
    /// page reaches only the commands and events that
    /// [`App::permit`](crate::App::permit) and its kin grant its origin.
    Allow,

    /// Opens no window.
    Deny,

    /// Opens the URL in the system's default browser instead. Only an
    /// `http` or `https` link the user clicked goes there; any other request
    /// is refused.
    OpenExternal,
}

/// Asks the application's hook, then applies its answer. Runs on CEF's UI
/// thread inside `OnBeforePopup` or `OnOpenURLFromTab`, with no lock held. A
/// hook that panics refuses the window: an error in the application's policy
/// must not open what the policy might have kept out. An ending application
/// opens nothing and asks no hook; that is checked again after the hook,
/// which may have begun the end itself.
pub(crate) fn decide(app: &AppHandle, request: &NewWindowRequest) -> Outcome {
    if app.is_ending() {
        debug!(
            "new window for {} refused: the application is ending",
            request.url
        );
        return Outcome::Refuse;
    }
    let decision = match app
        .hooks()
        .as_deref()
        .and_then(|hooks| hooks.new_window.as_ref())
    {
        Some(hook) => match catch_unwind(AssertUnwindSafe(|| hook(request, app))) {
            Ok(decision) => decision,
            Err(_) => {
                error!(
                    "on_new_window panicked; the window for {} is refused",
                    request.url
                );
                return Outcome::Refuse;
            }
        },
        None => NewWindowDecision::Default,
    };
    if app.is_ending() {
        debug!(
            "new window for {} refused: the application is ending",
            request.url
        );
        return Outcome::Refuse;
    }
    let outcome = resolve(decision, request, app.app_origin());
    debug!("new window for {}: {decision:?}, {outcome:?}", request.url);
    outcome
}

/// The outcome of `decision` for `request` in the application of
/// `app_origin`.
fn resolve(
    decision: NewWindowDecision,
    request: &NewWindowRequest,
    app_origin: &Origin,
) -> Outcome {
    match decision {
        NewWindowDecision::Default => {
            if !request.origin.is_opaque() && request.origin == *app_origin {
                Outcome::Open
            } else if is_chromium_page(&request.url) {
                // Pages cannot open these; Chromium's own UI can, DevTools'
                Outcome::Open
            } else {
                external_or_refused(&request.url, request.user_gesture)
            }
        }
        NewWindowDecision::Allow => Outcome::Open,
        NewWindowDecision::Deny => Outcome::Refuse,
        NewWindowDecision::OpenExternal => external_or_refused(&request.url, request.user_gesture),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> Origin {
        Origin::parse("app://app").unwrap()
    }

    /// The outcome of `decision` for a request from the application's page.
    fn outcome(decision: NewWindowDecision, url: &str, user_gesture: bool) -> Outcome {
        let request = NewWindowRequest::new(url.to_owned(), "app://app/index.html", user_gesture);
        resolve(decision, &request, &app())
    }

    #[test]
    fn by_default_only_the_application_s_own_pages_get_a_window() {
        use NewWindowDecision::Default;
        for url in [
            "app://app/settings.html",
            "",
            "about:blank",
            "about:blank#top",
            "blob:app://app/7f1c",
        ] {
            assert_eq!(outcome(Default, url, false), Outcome::Open, "{url:?}");
        }
        assert_eq!(
            outcome(Default, "https://docs.rs/", true),
            Outcome::External
        );
        for (url, user_gesture) in [
            ("https://docs.rs/", false),
            ("app://other/index.html", true),
            ("lc-other://x/probe.html", true),
            ("mailto:someone@example.com", true),
            ("file:///etc/passwd", true),
            ("data:text/html,hi", true),
            ("blob:https://evil.example/7f1c", true),
            ("javascript:alert(1)", true),
        ] {
            assert_eq!(
                outcome(Default, url, user_gesture),
                Outcome::Refuse,
                "{url:?}"
            );
        }
    }

    #[test]
    fn a_blank_window_has_its_opener_s_origin() {
        let foreign = NewWindowRequest::new("about:blank".into(), "https://evil.example/", true);
        assert_eq!(
            foreign.origin(),
            &Origin::parse("https://evil.example").unwrap()
        );
        assert_eq!(
            resolve(NewWindowDecision::Default, &foreign, &app()),
            Outcome::Refuse
        );
    }

    #[test]
    fn chromium_s_own_pages_keep_their_windows() {
        for url in [
            "devtools://devtools/bundled/devtools_app.html",
            "chrome://version/",
        ] {
            assert_eq!(
                outcome(NewWindowDecision::Default, url, false),
                Outcome::Open,
                "{url}"
            );
        }
    }

    #[test]
    fn the_hook_s_answer_stands_but_the_system_browser_needs_a_clicked_web_link() {
        let foreign = "https://accounts.example/login";
        assert_eq!(
            outcome(NewWindowDecision::Allow, foreign, false),
            Outcome::Open
        );
        assert_eq!(
            outcome(NewWindowDecision::Deny, "app://app/settings.html", true),
            Outcome::Refuse
        );
        assert_eq!(
            outcome(NewWindowDecision::OpenExternal, foreign, true),
            Outcome::External
        );
        assert_eq!(
            outcome(NewWindowDecision::OpenExternal, foreign, false),
            Outcome::Refuse
        );
        assert_eq!(
            outcome(
                NewWindowDecision::OpenExternal,
                "mailto:someone@example.com",
                true
            ),
            Outcome::Refuse
        );
        assert_eq!(
            outcome(
                NewWindowDecision::OpenExternal,
                "app://app/settings.html",
                true
            ),
            Outcome::Refuse
        );
    }

    #[test]
    fn an_application_of_an_opaque_origin_owns_no_page() {
        // An opaque application origin (an App::url file: page) matches no
        // page, the opaque ones included
        let request = NewWindowRequest::new("about:blank".into(), "file:///app/index.html", false);
        assert_eq!(
            resolve(NewWindowDecision::Default, &request, &Origin::OPAQUE),
            Outcome::Refuse
        );
    }
}
