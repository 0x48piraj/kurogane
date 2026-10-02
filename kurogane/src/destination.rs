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

/// The system browser for a web link the user clicked, otherwise nothing:
/// a page alone never starts another program.
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

/// Whether `url` is an empty document a page fills in itself: no URL,
/// `about:blank` or `about:srcdoc`.
pub(crate) fn is_blank(url: &str) -> bool {
    url.is_empty()
        || url::Url::parse(url)
            .is_ok_and(|url| url.scheme() == "about" && matches!(url.path(), "blank" | "srcdoc"))
}

/// The origin of the document `url` shows when a page of origin `from`
/// loads it. A blank document (see [`is_blank`]) inherits `from`; a `blob:`
/// URL has the origin it was made in.
pub(crate) fn document_origin(url: &str, from: &Origin) -> Origin {
    if is_blank(url) {
        return from.clone();
    }
    match url::Url::parse(url) {
        Ok(parsed) if parsed.scheme() == "blob" => Origin::from_url(parsed.path()),
        _ => Origin::from_url(url),
    }
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
