//! Where a page asks to take the application, a new window
//! ([`crate::new_window`]) or a window it navigates ([`crate::navigation`]),
//! and the answers both share.

use crate::acl::Origin;

/// What becomes of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The window opens, or the navigation proceeds.
    Open,
    /// The system browser opens the URL; nothing opens or moves here.
    External,
    /// Nothing opens, nothing moves.
    Refuse,
}

/// Returns the system browser for a web link the user clicked and a refusal
/// otherwise. A page alone never starts another program.
pub(crate) fn external_or_refused(url: &str, user_gesture: bool) -> Outcome {
    if user_gesture && crate::external::is_web_link(url) {
        Outcome::External
    } else {
        Outcome::Refuse
    }
}

/// Whether `url` is one of Chromium's own pages, which pages cannot open or
/// navigate to and Chromium's own UI (DevTools') can.
pub(crate) fn is_chromium_page(url: &str) -> bool {
    url::Url::parse(url)
        .is_ok_and(|url| matches!(url.scheme(), "chrome" | "chrome-untrusted" | "devtools"))
}

/// Returns whether `url` is an empty document a page fills in itself (no
/// URL, `about:blank` or `about:srcdoc`).
pub(crate) fn is_blank(url: &str) -> bool {
    url.is_empty()
        || url::Url::parse(url)
            .is_ok_and(|url| url.scheme() == "about" && matches!(url.path(), "blank" | "srcdoc"))
}

/// The origin of the document `url` shows when a page of origin `from`
/// loads it. A blank document (see [`is_blank`]) inherits `from`; everything
/// else has the origin of its own URL, a `blob:` URL carrying the origin of
/// the page that made it (see [`Origin::from_url`]).
pub(crate) fn document_origin(url: &str, from: &Origin) -> Origin {
    if is_blank(url) {
        return from.clone();
    }
    Origin::from_url(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(text: &str) -> Origin {
        Origin::parse(text).unwrap()
    }

    #[test]
    fn a_url_s_origin_is_its_host_s_however_it_is_dressed() {
        let app = origin("app://app");
        for (url, expected) in [
            // Credentials name no host
            (
                "https://app.example.com@evil.example/",
                "https://evil.example",
            ),
            ("lc-other://app:app@x/marker.html", "lc-other://x"),
            // Case and default ports fold; other ports differ
            ("HTTPS://Docs.RS:443/x", "https://docs.rs"),
            ("http://127.0.0.1:8081/", "http://127.0.0.1:8081"),
            // A name that only starts like a trusted one
            (
                "https://app.example.com.evil.example/",
                "https://app.example.com.evil.example",
            ),
            // A blob carries the origin it was made in
            ("blob:https://evil.example/7f1c", "https://evil.example"),
        ] {
            assert_eq!(document_origin(url, &app), origin(expected), "{url}");
        }
        assert_ne!(
            document_origin("http://127.0.0.1:8081/", &app),
            origin("http://127.0.0.1:8080")
        );
    }

    #[test]
    fn a_document_the_guard_opens_reaches_the_commands_of_its_origin() {
        // The guard names a document to decide whether it may open; the ACL
        // names the same document to decide what it may call. When the two
        // disagree the page loads and reaches nothing.
        use crate::acl::CommandAcl;

        let app = origin("app://app");
        let acl = CommandAcl::new(app.clone());
        for url in ["app://app/index.html", "blob:app://app/7f1c"] {
            let opened = document_origin(url, &app);
            assert_eq!(opened, app, "{url}");
            assert!(
                acl.allows("anything", &opened),
                "{url} opens but reaches nothing"
            );
        }
        // A document of another origin opens nowhere and calls nothing.
        for url in ["blob:https://evil.example/7f1c", "https://docs.rs/"] {
            let opened = document_origin(url, &app);
            assert_ne!(opened, app, "{url}");
            assert!(!acl.allows("anything", &opened), "{url}");
        }
    }

    #[test]
    fn only_a_blank_document_inherits_and_everything_unhosted_is_opaque() {
        let from = origin("https://docs.rs");
        for url in ["", "about:blank", "about:blank#top", "about:srcdoc"] {
            assert!(is_blank(url), "{url:?}");
            assert_eq!(document_origin(url, &from), from, "{url:?}");
        }
        for url in [
            "about:version",
            "data:text/html,<p>hi",
            "file:///etc/hosts",
            "javascript:alert(1)",
            "blob:null/7f1c",
            "not a url",
        ] {
            assert!(!is_blank(url), "{url}");
            assert!(document_origin(url, &from).is_opaque(), "{url}");
        }
    }
}
