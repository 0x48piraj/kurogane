//! Hands a web link to the system's default browser.
//!
//! Only `http` and `https` URLs go, at most 2048 characters long, and with
//! every character a command line could misread percent-encoded, as Chromium
//! passes URLs to the system: the system substitutes the URL into the
//! browser's registered command line, and a quote or a space there could add
//! arguments. Opening is fire-and-forget: a failure is logged, since the
//! page that asked has already been answered.

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use tracing::debug;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as platform;

/// The longest URL handed over: Windows' INTERNET_MAX_URL_LENGTH, which
/// Chromium applies on every platform.
const MAX_URL_LENGTH: usize = 2048;

/// Printable ASCII a URL may not carry unescaped to the system; non-ASCII
/// is always encoded. What remains is what Chromium leaves as it is.
const UNSAFE: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'\\')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// Whether `url` is a link the system browser may be given.
pub(crate) fn is_web_link(url: &str) -> bool {
    web_link(url).is_some()
}

/// Opens `url` in the system's default browser if it is a web link.
pub(crate) fn open(url: &str) {
    match web_link(url) {
        Some(link) => {
            debug!("opening {link} in the system browser");
            platform::open(link);
        }
        None => debug!("{url} is not a web link; not opened"),
    }
}

/// `url` as the system receives it, if it is a web link.
fn web_link(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let link = utf8_percent_encode(parsed.as_str(), UNSAFE).to_string();
    (link.len() <= MAX_URL_LENGTH).then_some(link)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_and_https_links_go_to_the_system() {
        assert!(is_web_link("https://docs.rs/kurogane"));
        assert!(is_web_link("HTTP://example.com"));
        for url in [
            "mailto:someone@example.com",
            "file:///C:/Windows/System32/calc.exe",
            "app://app/index.html",
            "javascript:alert(1)",
            "ms-settings:privacy",
            "not a url",
        ] {
            assert!(!is_web_link(url), "{url}");
        }
    }

    #[test]
    fn nothing_in_a_link_reads_as_a_separate_argument() {
        // A host keeps a quote the URL parser allows there
        let link = web_link("https://a\"b.example/x y?q=\"z\"#`f`").unwrap();
        assert_eq!(link, "https://a%22b.example/x%20y?q=%22z%22#%60f%60");
        assert!(!link.contains(|c: char| c == '"' || c.is_whitespace()));
    }

    #[test]
    fn links_are_capped_as_chromium_caps_them() {
        let base = "https://example.com/";
        let fits = format!("{base}{}", "a".repeat(MAX_URL_LENGTH - base.len()));
        assert!(is_web_link(&fits));
        assert!(!is_web_link(&format!("{fits}a")));
    }
}
