//! What happens when a page navigates the window it is in: a link, a
//! `location` assignment, a form, a redirect on the way.
//!
//! A window shows only what was let into it: the application's own origin,
//! the origins the application loaded there itself (the start page,
//! `create_window`, `BrowserHandle::navigate`, and the redirects they lead
//! to), the origin [`App::on_new_window`](crate::App::on_new_window) opened a
//! popup to, and the origins [`App::on_navigation`](crate::App::on_navigation)
//! let in. A page's navigation anywhere else goes, as a new window's request
//! does, to the system browser if it is a web link the user clicked, and
//! nowhere otherwise: the window stays on its page. Frames inside a page are
//! not guarded; the ACL keeps another origin's frame from the bridge.

use std::panic::{AssertUnwindSafe, catch_unwind};

use tracing::{debug, error};

use crate::acl::Origin;
use crate::destination::{Outcome, document_origin, external_or_refused};
use crate::runtime::AppHandle;

/// A page's navigation of the window it is in, passed to
/// [`App::on_navigation`](crate::App::on_navigation).
#[derive(Debug, Clone)]
pub struct NavigationRequest {
    url: String,
    origin: Origin,
    from_origin: Origin,
    user_gesture: bool,
    is_redirect: bool,
}

impl NavigationRequest {
    /// The navigation of a window showing `from_url` to `url`.
    pub(crate) fn new(url: String, from_url: &str, user_gesture: bool, is_redirect: bool) -> Self {
        let from_origin = Origin::from_url(from_url);
        let origin = document_origin(&url, &from_origin);
        Self {
            url,
            origin,
            from_origin,
            user_gesture,
            is_redirect,
        }
    }

    /// The URL the window would load.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The origin of the document the window would show.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    /// The origin of the document the window shows now.
    pub fn from_origin(&self) -> &Origin {
        &self.from_origin
    }

    /// Whether the user asked, by clicking a link or a button, rather than a
    /// script on its own.
    pub fn user_gesture(&self) -> bool {
        self.user_gesture
    }

    /// Whether the server sent the navigation here from the URL it asked
    /// for first.
    pub fn is_redirect(&self) -> bool {
        self.is_redirect
    }
}

/// The answer of [`App::on_navigation`](crate::App::on_navigation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum NavigationDecision {
    /// Kurogane's policy decides: an origin the window may show is loaded,
    /// an `http` or `https` link the user clicked to any other goes to the
    /// system browser, and anything else is refused.
    #[default]
    Default,

    /// Loads the page, and lets its origin into this window from now on.
    /// The page reaches only the commands and events that
    /// [`App::permit`](crate::App::permit) and its kin grant its origin.
    Allow,

    /// Refuses the navigation; the window stays on its page.
    Deny,

    /// Opens the URL in the system's default browser instead, and the
    /// window stays on its page. Only an `http` or `https` link the user
    /// clicked goes there; any other navigation is refused.
    OpenExternal,
}

/// Asks the application's hook, then applies its answer for a window that
/// does or does not already admit the request's origin. Runs on CEF's UI
/// thread inside `OnBeforeBrowse`, with no lock held. A hook that panics
/// refuses the navigation.
pub(crate) fn decide(app: &AppHandle, request: &NavigationRequest, admitted: bool) -> Outcome {
    let decision = match app
        .hooks()
        .as_deref()
        .and_then(|hooks| hooks.navigation.as_ref())
    {
        Some(hook) => match catch_unwind(AssertUnwindSafe(|| hook(request, app))) {
            Ok(decision) => decision,
            Err(_) => {
                error!(
                    "on_navigation panicked; the navigation to {} is refused",
                    request.url
                );
                return Outcome::Refuse;
            }
        },
        None => NavigationDecision::Default,
    };
    let outcome = resolve(decision, request, admitted);
    debug!("navigation to {}: {decision:?}, {outcome:?}", request.url);
    outcome
}

/// The outcome of `decision` for `request` in a window that does or does
/// not already admit its origin.
fn resolve(decision: NavigationDecision, request: &NavigationRequest, admitted: bool) -> Outcome {
    match decision {
        NavigationDecision::Default if admitted => Outcome::Open,
        NavigationDecision::Default | NavigationDecision::OpenExternal => {
            external_or_refused(&request.url, request.user_gesture)
        }
        NavigationDecision::Allow => Outcome::Open,
        NavigationDecision::Deny => Outcome::Refuse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(url: &str, user_gesture: bool) -> NavigationRequest {
        NavigationRequest::new(url.to_owned(), "app://app/index.html", user_gesture, false)
    }

    #[test]
    fn by_default_a_window_goes_only_where_it_may() {
        use NavigationDecision::Default;
        assert_eq!(
            resolve(Default, &request("app://app/b.html", false), true),
            Outcome::Open
        );
        assert_eq!(
            resolve(Default, &request("https://docs.rs/", true), false),
            Outcome::External
        );
        for (url, user_gesture) in [
            ("https://docs.rs/", false),
            ("lc-other://x/page.html", true),
            ("mailto:someone@example.com", true),
            ("file:///etc/passwd", true),
        ] {
            assert_eq!(
                resolve(Default, &request(url, user_gesture), false),
                Outcome::Refuse,
                "{url}"
            );
        }
    }

    #[test]
    fn the_hook_s_answer_stands_but_the_system_browser_needs_a_clicked_web_link() {
        let foreign = request("https://accounts.example/login", false);
        assert_eq!(
            resolve(NavigationDecision::Allow, &foreign, false),
            Outcome::Open
        );
        assert_eq!(
            resolve(NavigationDecision::Deny, &foreign, true),
            Outcome::Refuse
        );
        assert_eq!(
            resolve(
                NavigationDecision::OpenExternal,
                &request("https://docs.rs/", true),
                true
            ),
            Outcome::External
        );
        assert_eq!(
            resolve(NavigationDecision::OpenExternal, &foreign, false),
            Outcome::Refuse
        );
    }

    #[test]
    fn a_blank_page_has_the_origin_of_the_page_it_replaces() {
        let blank = NavigationRequest::new("about:blank".into(), "https://docs.rs/x", false, false);
        assert_eq!(blank.origin(), &Origin::parse("https://docs.rs").unwrap());
        assert_eq!(blank.from_origin(), blank.origin());
    }
}
