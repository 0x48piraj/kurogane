//! macOS: `NSWorkspace`, which hands the URL to Launch Services as an Apple
//! Event, never through a command line.

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};
use tracing::warn;

pub(super) fn open(link: String) {
    let Some(url) = NSURL::URLWithString(&NSString::from_str(&link)) else {
        warn!("the system did not take the link as a URL");
        return;
    };
    if !NSWorkspace::sharedWorkspace().openURL(&url) {
        warn!("the system did not open the link");
    }
}
