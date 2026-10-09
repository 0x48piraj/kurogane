//! Links and new windows, checked by hand: every way a page asks for a
//! window or takes its own somewhere, with what should happen next to it.
//! The hooks let https://example.org in and print every request they are
//! asked about.

use kurogane::{App, NavigationDecision, NewWindowDecision, Origin};

fn main() {
    kurogane_suite::logging();

    let allowed = Origin::parse("https://example.org").expect("example.org is an origin");
    let allowed_too = allowed.clone();
    App::new("scenarios/new-window/frontend")
        .on_new_window(move |request, _| {
            println!(
                "new window for {:?}: origin {}, opener {}, clicked: {}",
                request.url(),
                request.origin(),
                request.opener_origin(),
                request.user_gesture()
            );
            if request.origin() == &allowed {
                NewWindowDecision::Allow
            } else {
                NewWindowDecision::Default
            }
        })
        .on_navigation(move |navigation, _| {
            println!(
                "navigation to {:?}: origin {}, from {}, clicked: {}, redirect: {}",
                navigation.url(),
                navigation.origin(),
                navigation.from_origin(),
                navigation.user_gesture(),
                navigation.is_redirect()
            );
            if navigation.origin() == &allowed_too {
                NavigationDecision::Allow
            } else {
                NavigationDecision::Default
            }
        })
        .run_or_exit();
}
