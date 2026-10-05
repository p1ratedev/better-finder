//! The status bar: one line at the bottom of the window.
//!
//! - Left: the item count, the hidden count, the filter, the pending keys.
//! - Right: the message, `Loading…`, the sort key and direction.

use objc2::rc::Retained;
use objc2::{MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSBezierPath, NSResponder, NSView};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize};

use crate::app::{self, AppState};
use crate::list_view::draw_text;
use crate::theme::{self, Attrs};

/// The space between two parts of the right side.
const PART_GAP: f64 = 18.0;
/// The space at the left and right ends.
const END_PAD: f64 = 12.0;

define_class!(
    // SAFETY:
    // - NSView has no subclass requirements.
    // - StatusBar does not implement Drop.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BFStatusBar"]
    pub struct StatusBar;

    impl StatusBar {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let bounds = self.bounds();
            app::with_ref(|s| draw(s, bounds));
        }
    }
);

impl StatusBar {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 22.0));
        // SAFETY: `initWithFrame:` is the designated initializer of NSView.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

/// The text of the pending key sequence, for example `g-`.
pub fn pending_text(s: &AppState) -> Option<String> {
    let pending = s.keys.pending();
    if pending.is_empty() {
        return None;
    }
    let keys: Vec<String> = pending.iter().map(ToString::to_string).collect();
    Some(format!("{}-", keys.join(">")))
}

fn draw(s: &AppState, bounds: NSRect) {
    let st = &s.style;
    st.status_bg.setFill();
    NSBezierPath::fillRect(bounds);

    let y = ((bounds.size.height - st.status_line_height) / 2.0).round();
    let line = st.status_line_height;

    // The right side, drawn from the right end to the left.
    let mut right: Vec<(String, Attrs)> = Vec::new();
    if let Some(message) = &s.message {
        let alpha = message.alpha();
        if alpha > 0.0 {
            let base = if message.error {
                st.theme.accent
            } else {
                st.theme.status_foreground
            };
            let attrs = if alpha >= 1.0 {
                if message.error {
                    st.status_accent.clone()
                } else {
                    st.status.clone()
                }
            } else {
                theme::attrs(
                    &st.status_font,
                    &theme::color_alpha(base, alpha),
                    &tail_paragraph(),
                )
            };
            right.push((message.text.clone(), attrs));
        }
    }
    if s.loading_visible {
        right.push(("Loading\u{2026}".to_string(), st.status.clone()));
    }
    let sort = s.pane.sort();
    let arrow = if sort.reverse { "\u{2193}" } else { "\u{2191}" };
    right.push((format!("{} {arrow}", sort.key), st.status.clone()));

    let mut x = bounds.size.width - END_PAD;
    let left_limit = bounds.size.width * 0.35;
    for (i, (text, attrs)) in right.iter().enumerate().rev() {
        let width = theme::text_width(text, attrs).ceil() + 1.0;
        // The message (the first part) can shrink; it truncates at the tail.
        let width = if i == 0 {
            width.min((x - left_limit).max(0.0))
        } else {
            width
        };
        x -= width;
        draw_text(
            text,
            NSRect::new(NSPoint::new(x, y), NSSize::new(width, line)),
            attrs,
        );
        x -= PART_GAP;
    }
    let left_end = x;

    // The left side.
    let mut left: Vec<(String, &Attrs)> = Vec::new();
    if s.pane.is_loaded() {
        let (visible, _all, hidden) = s.pane.counts();
        let noun = if visible == 1 { "item" } else { "items" };
        left.push((format!("{visible} {noun}"), &st.status));
        if !s.pane.show_hidden() && hidden > 0 {
            left.push((format!("  \u{b7}  {hidden} hidden"), &st.status));
        }
        let filter = s.pane.filter();
        if !filter.is_empty() {
            left.push(("  \u{b7}  ".to_string(), &st.status));
            left.push((format!("filter \"{filter}\""), &st.status_accent));
        }
    }
    if let Some(keys) = pending_text(s) {
        left.push(("    ".to_string(), &st.status));
        left.push((keys, &st.status_accent));
    }
    let mut x = END_PAD;
    for (text, attrs) in left {
        let available = left_end - x;
        if available <= 0.0 {
            break;
        }
        let width = (theme::text_width(&text, attrs).ceil() + 1.0).min(available);
        draw_text(
            &text,
            NSRect::new(NSPoint::new(x, y), NSSize::new(width, line)),
            attrs,
        );
        x += width;
    }
}

fn tail_paragraph() -> Retained<objc2_app_kit::NSMutableParagraphStyle> {
    let p = objc2_app_kit::NSMutableParagraphStyle::new();
    p.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
    p
}
