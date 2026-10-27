//! The open requests of other apps to the file viewer (`bf_core::viewer`).
//!
//! The requests come as the Apple Event `aevt`/`odoc`. AppKit gives its
//! URLs to `application:openURLs:` of the app delegate, and the delegate
//! gives the paths to `app::show_items`. Two senders were seen on macOS 26:
//!
//! - Launch Services, for the default app of `public.folder`
//!   (`open ~/Downloads`).
//! - `NSWorkspace`, for the app in the `NSFileViewer` default.
//!   `activateFileViewerSelectingURLs:` ("Show in Finder") and
//!   `selectFile:inFileViewerRootedAtPath:` (`open -R <file>`) both send
//!   `odoc`, not the Finder event `misc`/`mvis`.
//!
//! `BETTER_FINDER_TRACE=1` prints the event that arrives.

use std::path::PathBuf;

use objc2::msg_send;
use objc2_foundation::{NSAppleEventManager, NSArray, NSBundle, NSString, NSURL, NSUserDefaults};

use crate::debug::trace;

/// True when the `NSFileViewer` default names this app. Then `NSWorkspace`
/// sends its reveal requests ("Show in Finder") to this app.
pub fn is_default_viewer() -> bool {
    let key = NSString::from_str("NSFileViewer");
    let viewer = NSUserDefaults::standardUserDefaults().stringForKey(&key);
    let own = NSBundle::mainBundle().bundleIdentifier();
    match (viewer, own) {
        // Launch Services ignores the case of bundle IDs.
        (Some(viewer), Some(own)) => viewer.to_string().eq_ignore_ascii_case(&own.to_string()),
        _ => false,
    }
}

/// The paths of the file URLs in `urls`. Other URLs are ignored.
pub fn url_paths(urls: &NSArray<NSURL>) -> Vec<PathBuf> {
    urls.iter().filter_map(|url| url.to_file_path()).collect()
}

/// Prints the class and the ID of the Apple Event in progress, when
/// `BETTER_FINDER_TRACE=1`.
pub fn trace_current_event(context: &str) {
    if !crate::debug::trace_on() {
        return;
    }
    let Some(event) = NSAppleEventManager::sharedAppleEventManager().currentAppleEvent() else {
        trace!("file viewer: {context}: no Apple Event");
        return;
    };
    // SAFETY: `eventClass` and `eventID` take no arguments and return a
    // four-character code (`UInt32`). The typed methods need the crate
    // objc2-core-services. These calls do not.
    let (class, id): (u32, u32) =
        unsafe { (msg_send![&event, eventClass], msg_send![&event, eventID]) };
    trace!(
        "file viewer: {context}: Apple Event {}/{}",
        code_text(class),
        code_text(id)
    );
}

/// The text form of a four-character code, for example `odoc`.
fn code_text(value: u32) -> String {
    String::from_utf8_lossy(&value.to_be_bytes()).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_text_reads_four_characters() {
        assert_eq!(code_text(u32::from_be_bytes(*b"odoc")), "odoc");
        assert_eq!(code_text(0x6165_7674), "aevt");
    }
}
