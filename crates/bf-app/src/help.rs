//! The key binding help overlay: all bindings from `Keymap::bindings()`,
//! grouped by action.
//!
//! The overlay takes the keyboard focus while it is open. Its keys go
//! through the user keymap (`app::handle_key` sends them to `app::help_key`):
//! the movement actions scroll it, and `cancel`, `help`, and `q` close it.

use std::cell::Cell;

use bf_core::{Action, Keymap};
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSBorderType, NSEvent, NSEventModifierFlags, NSResponder, NSScrollView, NSView,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize};

use crate::app::{self, AppState};
use crate::list_view::{self, draw_text};
use crate::smooth;
use crate::theme::{self, Style};

const PAD: f64 = 22.0;
const GAP: f64 = 18.0;
/// The space around the panel.
const MARGIN: f64 = 24.0;
/// The maximum width of the panel.
const MAX_WIDTH: f64 = 860.0;

/// One line of the overlay: all triggers of one action.
#[derive(Debug, Clone)]
pub struct HelpLine {
    pub keys: String,
    pub action: String,
    pub description: String,
}

/// The lines and the column widths of the overlay.
#[derive(Debug, Clone)]
pub struct HelpModel {
    pub lines: Vec<HelpLine>,
    /// The keys at the top right, from the keymap: `j k scroll · q escape ? close`.
    pub hint: String,
    pub key_width: f64,
    pub action_width: f64,
}

impl HelpModel {
    /// Groups the bindings by action, in the order of `Action::catalog()`.
    pub fn build(keymap: &Keymap, style: &Style) -> HelpModel {
        let mut lines: Vec<HelpLine> = Vec::new();
        for (trigger, action) in keymap.bindings() {
            let name = action.to_string();
            let key = trigger.to_string();
            match lines.iter_mut().find(|l| l.action == name) {
                Some(line) => {
                    line.keys.push_str("  ");
                    line.keys.push_str(&key);
                }
                None => lines.push(HelpLine {
                    keys: key,
                    action: name,
                    description: action.description().to_string(),
                }),
            }
        }
        let catalog = Action::catalog();
        let rank = |action: &str| {
            let base = action.split(':').next().unwrap_or(action);
            catalog
                .iter()
                .position(|(name, _)| name.split(':').next() == Some(base))
                .unwrap_or(usize::MAX)
        };
        lines.sort_by(|a, b| {
            rank(&a.action)
                .cmp(&rank(&b.action))
                .then(a.action.cmp(&b.action))
        });
        let widest = |f: &dyn Fn(&HelpLine) -> f64| lines.iter().map(f).fold(0.0_f64, f64::max);
        let key_width = widest(&|l| theme::text_width(&l.keys, &style.help_key))
            .ceil()
            .min(300.0);
        let action_width = widest(&|l| theme::text_width(&l.action, &style.help_action))
            .ceil()
            .min(240.0);
        HelpModel {
            hint: hint(&lines),
            lines,
            key_width,
            action_width,
        }
    }

    /// The height of the panel, in points.
    fn panel_height(&self, style: &Style) -> f64 {
        PAD * 2.0 + style.row_height * (self.lines.len() as f64 + 1.5)
    }

    /// The height of the content (the panel and its margins), in points.
    pub fn height(&self, style: &Style) -> f64 {
        self.panel_height(style) + 2.0 * MARGIN
    }
}

pub struct HelpIvars {
    content_height: Cell<f64>,
}

define_class!(
    // SAFETY:
    // - NSView has no subclass requirements.
    // - HelpView does not implement Drop.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BFHelpView"]
    #[ivars = HelpIvars]
    pub struct HelpView;

    impl HelpView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: NSRect) {
            let started = std::time::Instant::now();
            let bounds = self.bounds();
            app::with_ref(|s| draw(s, bounds, dirty));
            smooth::note_draw(started.elapsed());
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            list_view::handle_ns_key(event);
        }

        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            self.key_equivalent(event)
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            // A mouse scroll ends a smooth scroll at its target.
            smooth::finish();
            // SAFETY: NSResponder implements this method with this signature.
            let _: () = unsafe { msg_send![super(self), scrollWheel: event] };
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            app::dispatch(Action::Help);
        }

        #[unsafe(method(resizeWithOldSuperviewSize:))]
        fn resize_with_old_superview_size(&self, old: NSSize) {
            // SAFETY: NSView implements this method with this signature.
            let _: () = unsafe { msg_send![super(self), resizeWithOldSuperviewSize: old] };
            self.update_frame();
        }
    }
);

impl HelpView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(HelpIvars {
            content_height: Cell::new(0.0),
        });
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 100.0));
        // SAFETY: `initWithFrame:` is the designated initializer of NSView.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// Keys with `cmd` go to the keymap only when the overlay has the
    /// focus. A false result lets the main menu handle the key.
    fn key_equivalent(&self, event: &NSEvent) -> bool {
        if !event
            .modifierFlags()
            .contains(NSEventModifierFlags::Command)
        {
            return false;
        }
        let first = self
            .window()
            .and_then(|w| w.firstResponder())
            .is_some_and(|r| {
                std::ptr::eq(
                    Retained::as_ptr(&r).cast::<u8>(),
                    (self as *const Self).cast::<u8>(),
                )
            });
        first && list_view::handle_ns_key(event)
    }

    /// Sets the content height and updates the frame.
    pub fn set_content_height(&self, height: f64) {
        self.ivars().content_height.set(height);
        self.update_frame();
    }

    /// Width = the clip view width. Height = the content, at least the clip view.
    pub fn update_frame(&self) {
        // SAFETY: The superview is the clip view, which retains this view.
        let Some(clip) = (unsafe { self.superview() }) else {
            return;
        };
        let clip_size = clip.bounds().size;
        let height = self.ivars().content_height.get().max(clip_size.height);
        self.setFrameSize(NSSize::new(clip_size.width, height));
    }
}

/// The overlay views.
#[derive(Clone)]
pub struct HelpUi {
    pub scroll: Retained<NSScrollView>,
    pub view: Retained<HelpView>,
}

pub fn create(mtm: MainThreadMarker) -> HelpUi {
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 100.0));
    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    scroll.setDrawsBackground(false);
    scroll.setBorderType(NSBorderType::NoBorder);
    // The overlay is below the toolbar: no automatic insets, so that the
    // scroll range starts at 0.
    scroll.setAutomaticallyAdjustsContentInsets(false);
    let view = HelpView::new(mtm);
    scroll.setDocumentView(Some(&view));
    scroll.setHidden(true);
    HelpUi { scroll, view }
}

/// The hint text: one key of each scroll action and of each close action,
/// from the keymap. `q` always closes the overlay.
fn hint(lines: &[HelpLine]) -> String {
    // The shortest key of the action: `j` before `down`.
    let first_key = |action: &str| {
        lines
            .iter()
            .find(|l| l.action == action)
            .and_then(|l| l.keys.split("  ").min_by_key(|k| k.chars().count()))
            .map(str::to_string)
    };
    let scroll: Vec<String> = ["cursor_down", "cursor_up", "half_page_down", "half_page_up"]
        .iter()
        .filter_map(|a| first_key(a))
        .collect();
    let mut close = vec!["q".to_string()];
    close.extend(["cancel", "help"].iter().filter_map(|a| first_key(a)));
    close.dedup();
    if scroll.is_empty() {
        format!("{} close", close.join(" "))
    } else {
        format!("{} scroll · {} close", scroll.join(" "), close.join(" "))
    }
}

/// Draws the overlay. Only the lines in `dirty` draw text: a scroll shows
/// a strip of a few lines.
fn draw(s: &AppState, bounds: NSRect, dirty: NSRect) {
    let Some(model) = &s.help else { return };
    let in_dirty =
        |y: f64, height: f64| y < dirty.origin.y + dirty.size.height && y + height > dirty.origin.y;
    let st = &s.style;
    // Dim the list under the overlay.
    st.colors.list_bg.colorWithAlphaComponent(0.85).setFill();
    NSBezierPath::fillRect(bounds);

    let panel_w = (bounds.size.width - 2.0 * MARGIN).clamp(0.0, MAX_WIDTH);
    let left = ((bounds.size.width - panel_w) / 2.0).round();
    let panel = NSRect::new(
        NSPoint::new(left + 0.5, MARGIN + 0.5),
        NSSize::new(panel_w - 1.0, model.panel_height(st) - 1.0),
    );
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(panel, 10.0, 10.0);
    st.colors.panel_bg.setFill();
    path.fill();
    st.colors.separator.setStroke();
    path.stroke();

    let h = st.row_height;
    let line = st.line_height;
    let top = MARGIN + PAD;
    let right = left + panel_w - PAD;
    let text_dy = ((h - line) / 2.0).round();
    if in_dirty(top, h) {
        draw_title(model, st, (left, right, top + text_dy, panel_w));
    }

    let key_x = left + PAD;
    let action_x = key_x + model.key_width + GAP;
    let desc_x = action_x + model.action_width + GAP;
    for (i, item) in model.lines.iter().enumerate() {
        let y = top + h * (i as f64 + 1.5) + text_dy;
        if !in_dirty(y, line) {
            continue;
        }
        let rect = |x: f64, w: f64| NSRect::new(NSPoint::new(x, y), NSSize::new(w.max(0.0), line));
        draw_text(&item.keys, rect(key_x, model.key_width), &st.help_key);
        draw_text(
            &item.action,
            rect(action_x, model.action_width),
            &st.help_action,
        );
        draw_text(
            &item.description,
            rect(desc_x, right - desc_x),
            &st.help_desc,
        );
    }
}

/// The title on the left and the key hint on the right. `(left, right, y,
/// panel_w)`: the text edges, the text top, and the panel width.
fn draw_title(model: &HelpModel, st: &Style, (left, right, y, panel_w): (f64, f64, f64, f64)) {
    let line = st.line_height;
    let title_rect = NSRect::new(
        NSPoint::new(left + PAD, y),
        NSSize::new(panel_w / 2.0, line + 4.0),
    );
    draw_text("Key bindings", title_rect, &st.help_title);
    let hint = model.hint.as_str();
    let hint_w = (theme::text_width(hint, &st.help_desc).ceil() + 1.0).min(panel_w / 2.0);
    let hint_rect = NSRect::new(NSPoint::new(right - hint_w, y), NSSize::new(hint_w, line));
    draw_text(hint, hint_rect, &st.help_desc);
}
