//! Links and new windows, checked by hand: every way a page asks for a
//! window, with what should happen next to it. The hook lets
//! https://example.org in and prints every request it is asked about.

use kurogane::{App, NewWindowDecision, Origin};

fn main() {
    kurogane_suite::logging();

    let allowed = Origin::parse("https://example.org").expect("example.org is an origin");
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
        .run_or_exit();
}
