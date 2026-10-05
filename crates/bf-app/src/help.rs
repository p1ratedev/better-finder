//! The key binding help overlay: all bindings from `Keymap::bindings()`,
//! grouped by action. `Escape` or `?` closes it.

use std::cell::Cell;

use bf_core::{Action, Keymap};
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSBezierPath, NSBorderType, NSEvent, NSResponder, NSScrollView, NSView};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize};

use crate::app::{self, AppState};
use crate::list_view::draw_text;
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

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let bounds = self.bounds();
            app::with_ref(|s| draw(s, bounds));
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
    let view = HelpView::new(mtm);
    scroll.setDocumentView(Some(&view));
    scroll.setHidden(true);
    HelpUi { scroll, view }
}

fn draw(s: &AppState, bounds: NSRect) {
    let Some(model) = &s.help else { return };
    let st = &s.style;
    // Dim the list under the overlay.
    theme::color_alpha(st.theme.background, 0.85).setFill();
    NSBezierPath::fillRect(bounds);

    let panel_w = (bounds.size.width - 2.0 * MARGIN).clamp(0.0, MAX_WIDTH);
    let left = ((bounds.size.width - panel_w) / 2.0).round();
    let panel = NSRect::new(
        NSPoint::new(left + 0.5, MARGIN + 0.5),
        NSSize::new(panel_w - 1.0, model.panel_height(st) - 1.0),
    );
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(panel, 10.0, 10.0);
    st.panel_bg.setFill();
    path.fill();
    st.border.setStroke();
    path.stroke();

    let h = st.row_height;
    let line = st.line_height;
    let top = MARGIN + PAD;
    let right = left + panel_w - PAD;
    let text_dy = ((h - line) / 2.0).round();
    let title_rect = NSRect::new(
        NSPoint::new(left + PAD, top + text_dy),
        NSSize::new(panel_w / 2.0, line + 4.0),
    );
    draw_text("Key bindings", title_rect, &st.help_title);
    let hint = "Escape or ? closes this list";
    let hint_w = theme::text_width(hint, &st.help_desc).ceil() + 1.0;
    let hint_rect = NSRect::new(
        NSPoint::new(right - hint_w, top + text_dy),
        NSSize::new(hint_w, line),
    );
    draw_text(hint, hint_rect, &st.help_desc);

    let key_x = left + PAD;
    let action_x = key_x + model.key_width + GAP;
    let desc_x = action_x + model.action_width + GAP;
    for (i, item) in model.lines.iter().enumerate() {
        let y = top + h * (i as f64 + 1.5) + text_dy;
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
