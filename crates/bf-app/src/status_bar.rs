//! The path and status bar: one line at the bottom of the content.
//!
//! With `path-bar = true` (like Finder):
//! - Left: the path from the volume root as crumbs with small icons, for
//!   example `Macintosh HD › Users › admin › Downloads`. When there is no
//!   space, the middle crumbs lose their text first. A click on a crumb goes
//!   to that folder.
//! - Right: the item count, the hidden count, the free space, the filter,
//!   the pending keys, and the message.
//!
//! With `path-bar = false`: the status text only, like the first version
//! (counts on the left; message and sort on the right).
//!
//! An error message can use more of the width than an info message: it
//! tells what to do next (for example the busy message of `eject`). The
//! crumbs get less space, and the free space hides when the message does
//! not fit.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use bf_core::Action;
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSBezierPath, NSEvent, NSResponder, NSView};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize};

use crate::app::{self, AppState};
use crate::icons;
use crate::list_view::draw_text;
use crate::theme::{self, Attrs};

/// The space between two parts of the status text.
const PART_GAP: f64 = 14.0;
/// The space at the left and right ends.
const END_PAD: f64 = 12.0;
/// The crumb icon size.
const CRUMB_ICON: f64 = 14.0;

#[derive(Default)]
pub struct BarIvars {
    /// The crumbs of the last draw: (x start, x end, folder).
    crumbs: RefCell<Vec<(f64, f64, PathBuf)>>,
}

define_class!(
    // SAFETY:
    // - NSView has no subclass requirements.
    // - StatusBar does not implement Drop.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BFStatusBar"]
    #[ivars = BarIvars]
    pub struct StatusBar;

    impl StatusBar {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let bounds = self.bounds();
            let crumbs = app::with_ref(|s| draw(s, bounds)).unwrap_or_default();
            *self.ivars().crumbs.borrow_mut() = crumbs;
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            let target = self
                .ivars()
                .crumbs
                .borrow()
                .iter()
                .find(|(start, end, _)| point.x >= *start && point.x <= *end)
                .map(|(_, _, path)| path.clone());
            if let Some(path) = target {
                app::dispatch(Action::Goto(path.to_string_lossy().into_owned()));
            }
        }
    }
);

impl StatusBar {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(BarIvars::default());
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 24.0));
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

/// The crumbs of `cwd`: (name, folder). The first crumb is the volume root.
pub fn crumbs(cwd: &Path, mount: Option<(&Path, &str)>) -> Vec<(String, PathBuf)> {
    let (root, root_name) = match mount {
        Some((root, name)) if cwd.starts_with(root) => (root.to_path_buf(), name.to_string()),
        _ => (PathBuf::from("/"), "/".to_string()),
    };
    let mut out = vec![(root_name, root.clone())];
    let mut path = root.clone();
    if let Ok(rest) = cwd.strip_prefix(&root) {
        for part in rest.components() {
            path.push(part);
            out.push((
                part.as_os_str().to_string_lossy().into_owned(),
                path.clone(),
            ));
        }
    }
    out
}

/// One status part: text and attributes.
type Part = (String, Attrs);

/// The message, faded during its last 0.4 s.
fn message_part(s: &AppState) -> Option<Part> {
    let st = &s.style;
    let message = s.message.as_ref()?;
    let alpha = message.alpha();
    if alpha <= 0.0 {
        return None;
    }
    let attrs = if message.error {
        st.bar_accent.clone()
    } else if alpha >= 1.0 {
        st.bar.clone()
    } else {
        let para = objc2_app_kit::NSMutableParagraphStyle::new();
        para.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        theme::attrs(
            &st.small_font,
            &st.colors.bar_fg.colorWithAlphaComponent(alpha),
            &para,
        )
    };
    Some((message.text.clone(), attrs))
}

/// The counts: `578 items, 6 hidden`.
fn counts_text(s: &AppState) -> Option<String> {
    if !s.pane.is_loaded() {
        return None;
    }
    let (visible, _all, hidden) = s.pane.counts();
    let noun = if visible == 1 { "item" } else { "items" };
    let mut text = format!("{visible} {noun}");
    if !s.pane.show_hidden() && hidden > 0 {
        text.push_str(&format!(", {hidden} hidden"));
    }
    Some(text)
}

/// The left limit of the message, as a part of the bar width. `normal` is
/// the limit for an info message.
fn message_limit(s: &AppState, normal: f64) -> f64 {
    if s.message.as_ref().is_some_and(|m| m.error) {
        0.05
    } else {
        normal
    }
}

/// The width of `parts` with the gaps between them.
fn parts_width(parts: &[Part]) -> f64 {
    parts
        .iter()
        .map(|(text, attrs)| theme::text_width(text, attrs).ceil() + 1.0 + PART_GAP)
        .sum()
}

/// Draws the parts from `right` to the left. The first part (the message)
/// can shrink to `limit`. Returns the left end.
fn draw_right(parts: &[Part], mut right: f64, limit: f64, y: f64, line: f64) -> f64 {
    for (i, (text, attrs)) in parts.iter().enumerate().rev() {
        let width = theme::text_width(text, attrs).ceil() + 1.0;
        let width = if i == 0 {
            width.min((right - limit).max(0.0))
        } else {
            width
        };
        right -= width;
        draw_text(
            text,
            NSRect::new(NSPoint::new(right, y), NSSize::new(width, line)),
            attrs,
        );
        right -= PART_GAP;
    }
    right
}

/// Draws the bar. Returns the crumb rects for the clicks.
fn draw(s: &AppState, bounds: NSRect) -> Vec<(f64, f64, PathBuf)> {
    let st = &s.style;
    st.colors.bar_bg.setFill();
    NSBezierPath::fillRect(bounds);
    st.colors.separator.setFill();
    NSBezierPath::fillRect(NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(bounds.size.width, 1.0),
    ));
    let line = st.small_line_height;
    let y = ((bounds.size.height - line) / 2.0).round() + 0.5;
    if s.config.path_bar {
        draw_path_bar(s, bounds, y, line)
    } else {
        draw_status_only(s, bounds, y, line);
        Vec::new()
    }
}

/// The Finder layout: crumbs on the left, status on the right.
fn draw_path_bar(s: &AppState, bounds: NSRect, y: f64, line: f64) -> Vec<(f64, f64, PathBuf)> {
    let st = &s.style;
    // The right side, from the left to the right; the first part can shrink.
    let mut right: Vec<Part> = Vec::new();
    if let Some(message) = message_part(s) {
        right.push(message);
    }
    if let Some(keys) = pending_text(s) {
        right.push((keys, st.bar_accent.clone()));
    }
    if s.loading_visible {
        right.push(("Loading\u{2026}".to_string(), st.bar.clone()));
    }
    let filter = s.pane.filter();
    if !filter.is_empty() {
        right.push((format!("filter \"{filter}\""), st.bar_accent.clone()));
    }
    let mut status = counts_text(s).unwrap_or_default();
    if let Some(volume) = &s.volume {
        let free = crate::columns::byte_count(volume.free);
        if status.is_empty() {
            status = format!("{free} available");
        } else {
            status.push_str(&format!(", {free} available"));
        }
    }
    if !status.is_empty() {
        right.push((status, st.bar.clone()));
    }
    // An error message that does not fit hides the free space.
    let limit = bounds.size.width * message_limit(s, 0.45);
    let is_error = s.message.as_ref().is_some_and(|m| m.error);
    if is_error && parts_width(&right) > bounds.size.width - END_PAD - limit {
        if let (Some(counts), Some(last)) = (counts_text(s), right.last_mut()) {
            last.0 = counts;
        }
    }
    let left_end = draw_right(&right, bounds.size.width - END_PAD, limit, y, line);

    // The crumbs.
    let cwd = s.pane.cwd();
    let mount = s
        .volume
        .as_ref()
        .map(|v| (v.mount.as_path(), v.name.as_str()));
    let crumbs = crumbs(cwd, mount);
    let separator = " \u{203a} ";
    let sep_w = theme::text_width(separator, &st.bar).ceil();
    let text_w: Vec<f64> = crumbs
        .iter()
        .map(|(name, _)| theme::text_width(name, &st.bar).ceil() + 1.0)
        .collect();
    let icon_w = CRUMB_ICON + 3.0;
    let available = (left_end - END_PAD).max(0.0);
    // Which crumbs keep their text. The middle crumbs lose it first.
    let mut show_text = vec![true; crumbs.len()];
    let total = |show: &[bool]| -> f64 {
        crumbs
            .iter()
            .enumerate()
            .map(|(i, _)| icon_w + if show[i] { text_w[i] } else { 0.0 })
            .sum::<f64>()
            + sep_w * crumbs.len().saturating_sub(1) as f64
    };
    let mut order: Vec<usize> = (1..crumbs.len().saturating_sub(1)).collect();
    let middle = crumbs.len() as f64 / 2.0;
    order.sort_by(|a, b| {
        ((*a as f64) - middle)
            .abs()
            .total_cmp(&((*b as f64) - middle).abs())
    });
    for i in order {
        if total(&show_text) <= available {
            break;
        }
        show_text[i] = false;
    }
    let mut x = END_PAD;
    let mut rects = Vec::new();
    let last = crumbs.len().saturating_sub(1);
    for (i, (name, path)) in crumbs.iter().enumerate() {
        if x >= left_end - icon_w {
            break;
        }
        let start = x;
        let icon = if i == 0 {
            icons::volume_icon()
        } else {
            icons::folder_icon()
        };
        if let Some(icon) = icon {
            let rect = NSRect::new(
                NSPoint::new(x, ((st.bar_height - CRUMB_ICON) / 2.0).round()),
                NSSize::new(CRUMB_ICON, CRUMB_ICON),
            );
            icons::draw_in(&icon, rect);
        }
        x += icon_w;
        if show_text[i] {
            let width = if i == last {
                text_w[i].min((left_end - x).max(0.0))
            } else {
                text_w[i]
            };
            draw_text(
                name,
                NSRect::new(NSPoint::new(x, y), NSSize::new(width, line)),
                &st.bar,
            );
            x += width;
        }
        rects.push((start, x, path.clone()));
        if i != last {
            draw_text(
                separator,
                NSRect::new(NSPoint::new(x, y), NSSize::new(sep_w, line)),
                &st.bar,
            );
            x += sep_w;
        }
    }
    rects
}

/// The first layout: counts on the left; message and sort on the right.
fn draw_status_only(s: &AppState, bounds: NSRect, y: f64, line: f64) {
    let st = &s.style;
    let mut right: Vec<Part> = Vec::new();
    if let Some(message) = message_part(s) {
        right.push(message);
    }
    if s.loading_visible {
        right.push(("Loading\u{2026}".to_string(), st.bar.clone()));
    }
    if !s.config.column_header {
        let sort = s.pane.sort();
        let arrow = if sort.reverse { "\u{2193}" } else { "\u{2191}" };
        right.push((format!("{} {arrow}", sort.key), st.bar.clone()));
    }
    let left_end = draw_right(
        &right,
        bounds.size.width - END_PAD,
        bounds.size.width * message_limit(s, 0.35),
        y,
        line,
    );
    let mut left: Vec<(String, &Attrs)> = Vec::new();
    if let Some(counts) = counts_text(s) {
        left.push((counts, &st.bar));
    }
    let filter = s.pane.filter();
    if !filter.is_empty() {
        left.push(("  \u{b7}  ".to_string(), &st.bar));
        left.push((format!("filter \"{filter}\""), &st.bar_accent));
    }
    if let Some(keys) = pending_text(s) {
        left.push(("    ".to_string(), &st.bar));
        left.push((keys, &st.bar_accent));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crumbs_from_the_volume_root() {
        let c = crumbs(
            Path::new("/Users/admin/Downloads"),
            Some((Path::new("/"), "Macintosh HD")),
        );
        let names: Vec<&str> = c.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Macintosh HD", "Users", "admin", "Downloads"]);
        assert_eq!(c[2].1, PathBuf::from("/Users/admin"));
        let c = crumbs(
            Path::new("/Volumes/X/a"),
            Some((Path::new("/Volumes/X"), "X")),
        );
        assert_eq!(c.len(), 2);
        assert_eq!(c[1].1, PathBuf::from("/Volumes/X/a"));
        let c = crumbs(Path::new("/tmp"), None);
        assert_eq!(c[0].0, "/");
    }
}
