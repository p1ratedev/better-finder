//! The "Go to Folder" panel: a floating panel at the top center of the
//! content, like the Finder panel. A 13 pt field, and up to 8 folders that
//! match the text below it.
//!
//! - `Tab` completes (the selected match, or the common prefix).
//! - `Up` and `Down` select a match. `Enter` goes to the selected match, or
//!   to the text.
//! - A background thread lists the matches (a directory read).
//!
//! The same view without a field (`create_list`) is the match list of the
//! location field: the filter field with a path.

use std::cell::OnceCell;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSFocusRingType, NSFont, NSResponder, NSShadow, NSTextField, NSView,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString};

use crate::app::{self, AppState};
use crate::icons;
use crate::list_view::draw_text;
use crate::prompt::PromptDelegate;

/// The most matches the panel shows.
pub const MAX_MATCHES: usize = 8;
/// The height of the field part and of one match row.
pub const FIELD_HEIGHT: f64 = 44.0;
pub const ROW_HEIGHT: f64 = 24.0;
const PAD: f64 = 12.0;
/// The x of the text in the field and in the match rows.
pub const TEXT_X: f64 = PAD + 26.0;
const RADIUS: f64 = 12.0;

#[derive(Default)]
pub struct PanelIvars {
    field: OnceCell<Retained<NSTextField>>,
}

define_class!(
    // SAFETY:
    // - NSView has no subclass requirements.
    // - GotoPanel does not implement Drop.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BFGotoPanel"]
    #[ivars = PanelIvars]
    pub struct GotoPanel;

    impl GotoPanel {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let bounds = self.bounds();
            let with_field = self.field().is_some();
            app::with_ref(|s| draw(s, bounds, with_field));
        }

        #[unsafe(method(resizeSubviewsWithOldSize:))]
        fn resize_subviews_with_old_size(&self, _old: NSSize) {
            self.layout_field();
        }
    }
);

impl GotoPanel {
    pub fn field(&self) -> Option<&Retained<NSTextField>> {
        self.ivars().field.get()
    }

    fn layout_field(&self) {
        let Some(field) = self.ivars().field.get() else {
            return;
        };
        let width = self.bounds().size.width;
        let field_h = field.intrinsicContentSize().height.ceil().max(18.0);
        let x = TEXT_X;
        field.setFrame(NSRect::new(
            NSPoint::new(x, ((FIELD_HEIGHT - field_h) / 2.0).round()),
            NSSize::new((width - x - PAD).max(0.0), field_h),
        ));
    }
}

/// Makes the panel (hidden). `delegate` handles the field text.
pub fn create(mtm: MainThreadMarker, delegate: &Retained<PromptDelegate>) -> Retained<GotoPanel> {
    let panel: Retained<GotoPanel> = {
        let this = GotoPanel::alloc(mtm).set_ivars(PanelIvars::default());
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(500.0, FIELD_HEIGHT));
        // SAFETY: `initWithFrame:` is the designated initializer of NSView.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    };
    let field = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
    field.setBezeled(false);
    field.setBordered(false);
    field.setDrawsBackground(false);
    field.setFocusRingType(NSFocusRingType::None);
    field.setFont(Some(&NSFont::systemFontOfSize(13.0)));
    field.setPlaceholderString(Some(&NSString::from_str("Enter a path")));
    // SAFETY: The prompt UI keeps the delegate alive as long as the field.
    unsafe { field.setDelegate(Some(ProtocolObject::from_ref(&**delegate))) };
    panel.addSubview(&field);
    let _ = panel.ivars().field.set(field);
    panel.layout_field();
    panel.setHidden(true);
    panel
}

/// Makes the match list of the location field (hidden): the panel without
/// a field.
pub fn create_list(mtm: MainThreadMarker) -> Retained<GotoPanel> {
    let list: Retained<GotoPanel> = {
        let this = GotoPanel::alloc(mtm).set_ivars(PanelIvars::default());
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(320.0, ROW_HEIGHT));
        // SAFETY: `initWithFrame:` is the designated initializer of NSView.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    };
    list.setHidden(true);
    list
}

/// The height of the match list for `count` matches. 0 hides the list.
pub fn list_height(count: usize) -> f64 {
    match count.min(MAX_MATCHES) {
        0 => 0.0,
        rows => rows as f64 * ROW_HEIGHT + 8.0,
    }
}

/// The panel height for `count` matches.
pub fn height(count: usize) -> f64 {
    let rows = count.min(MAX_MATCHES) as f64;
    FIELD_HEIGHT
        + if rows > 0.0 {
            rows * ROW_HEIGHT + 8.0
        } else {
            0.0
        }
}

/// Draws the panel. `with_field`: the "go to" panel (the field on top);
/// else the match list of the location field.
fn draw(s: &AppState, bounds: NSRect, with_field: bool) {
    let st = &s.style;
    let panel = NSRect::new(
        NSPoint::new(0.5, 0.5),
        NSSize::new(bounds.size.width - 1.0, bounds.size.height - 1.0),
    );
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(panel, RADIUS, RADIUS);
    st.colors.panel_bg.setFill();
    path.fill();
    st.colors.separator.setStroke();
    path.stroke();
    let goto = s.goto_matches();
    let rows_top = if with_field {
        let icon_rect = NSRect::new(
            NSPoint::new(PAD, ((FIELD_HEIGHT - 18.0) / 2.0).round()),
            NSSize::new(18.0, 18.0),
        );
        if let Some(icon) = icons::folder_icon() {
            icons::draw_in(&icon, icon_rect);
        }
        if goto.0.is_empty() {
            return;
        }
        st.colors.separator.setFill();
        NSBezierPath::fillRect(NSRect::new(
            NSPoint::new(0.0, FIELD_HEIGHT),
            NSSize::new(bounds.size.width, 1.0),
        ));
        FIELD_HEIGHT
    } else {
        0.0
    };
    let line = st.line_height;
    for (i, name) in goto.0.iter().take(MAX_MATCHES).enumerate() {
        let y = rows_top + 4.0 + i as f64 * ROW_HEIGHT;
        let selected = goto.1 == Some(i);
        if selected {
            st.colors.selection_bg.setFill();
            let rect = NSRect::new(
                NSPoint::new(6.0, y),
                NSSize::new(bounds.size.width - 12.0, ROW_HEIGHT),
            );
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, 5.0, 5.0).fill();
        }
        if let Some(icon) = icons::folder_icon() {
            icons::draw_in(
                &icon,
                NSRect::new(
                    NSPoint::new(PAD, y + ((ROW_HEIGHT - 16.0) / 2.0).round()),
                    NSSize::new(16.0, 16.0),
                ),
            );
        }
        let attrs = if selected {
            &st.selected.name
        } else {
            &st.normal.name
        };
        let x = TEXT_X;
        draw_text(
            name,
            NSRect::new(
                NSPoint::new(x, y + ((ROW_HEIGHT - line) / 2.0).round()),
                NSSize::new((bounds.size.width - x - PAD).max(0.0), line),
            ),
            attrs,
        );
    }
}

/// Puts a soft shadow under the panel.
pub fn set_shadow(panel: &GotoPanel) {
    let shadow = NSShadow::new();
    shadow.setShadowBlurRadius(16.0);
    shadow.setShadowOffset(NSSize::new(0.0, -4.0));
    panel.setShadow(Some(&shadow));
}
