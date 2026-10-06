//! The file list: a custom `NSView` inside an `NSScrollView`.
//!
//! - It draws only the rows in the dirty rect, with the Finder metrics.
//! - The scroll view draws the background. The list content scrolls under
//!   the toolbar and the column header: `inset_top` is the height that they
//!   cover, and the scroll code keeps the cursor below it.
//! - It converts key events to `bf_core::KeyEvent` and sends them to the keymap.
//! - It handles the mouse.
//! - It is the controller of the Quick Look panel (it is in the responder chain).
//! - It hosts the inline rename field.

use std::cell::Cell;
use std::time::{Instant, SystemTime};

use bf_core::keymap::{Key, Mods, NamedKey};
use bf_core::{Entry, KeyEvent};
use objc2::rc::Retained;
use objc2::{ClassType, DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSCompositingOperation, NSEvent, NSEventModifierFlags, NSResponder,
    NSStringDrawing, NSView,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString};
use objc2_quick_look_ui::QLPreviewPanel;

use crate::app::{self, AppState};
use crate::columns;
use crate::debug::trace;
use crate::icons;
use crate::quicklook;
use crate::theme::{
    Attrs, ColumnLayout, LEFT_PAD, RowState, SELECTION_INSET, SELECTION_RADIUS, Style,
};

pub struct ListIvars {
    row_height: Cell<f64>,
    rows: Cell<usize>,
    /// The height at the top that the toolbar and the column header cover.
    inset_top: Cell<f64>,
}

thread_local! {
    static FIRST_FRAME: Cell<bool> = const { Cell::new(false) };
}

define_class!(
    // SAFETY:
    // - NSView has no subclass requirements.
    // - ListView does not implement Drop.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BFListView"]
    #[ivars = ListIvars]
    pub struct ListView;

    impl ListView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            // AppKit is in the middle of a first responder change. Run the
            // app logic later, not inside this call.
            app::after_now(app::list_did_focus);
            // The cursor row changes from the inactive to the active color.
            self.setNeedsDisplay(true);
            true
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            self.setNeedsDisplay(true);
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: NSRect) {
            self.draw(dirty);
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            handle_ns_key(event);
        }

        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            self.key_equivalent(event)
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            if let Some(window) = self.window() {
                window.makeFirstResponder(Some(self.as_responder()));
            }
            let row_height = self.ivars().row_height.get();
            if point.y >= 0.0 && row_height > 0.0 {
                app::click_row((point.y / row_height) as usize, event.clickCount() >= 2);
            }
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            // A scroll cancels the inline rename, like in Finder.
            app::after_now(app::cancel_inline_rename);
            // SAFETY: NSResponder implements this method with this signature.
            let _: () = unsafe { msg_send![super(self), scrollWheel: event] };
        }

        #[unsafe(method(resizeWithOldSuperviewSize:))]
        fn resize_with_old_superview_size(&self, old: NSSize) {
            // SAFETY: NSView implements this method with this signature.
            let _: () = unsafe { msg_send![super(self), resizeWithOldSuperviewSize: old] };
            self.update_frame();
        }

        #[unsafe(method(acceptsPreviewPanelControl:))]
        fn accepts_preview_panel_control(&self, _panel: Option<&QLPreviewPanel>) -> bool {
            true
        }

        #[unsafe(method(beginPreviewPanelControl:))]
        fn begin_preview_panel_control(&self, panel: Option<&QLPreviewPanel>) {
            quicklook::begin_control(panel);
        }

        #[unsafe(method(endPreviewPanelControl:))]
        fn end_preview_panel_control(&self, panel: Option<&QLPreviewPanel>) {
            quicklook::end_control(panel);
        }
    }
);

impl ListView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ListIvars {
            row_height: Cell::new(20.0),
            rows: Cell::new(0),
            inset_top: Cell::new(0.0),
        });
        // SAFETY: `initWithFrame:` is the designated initializer of NSView.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn as_responder(&self) -> &NSResponder {
        self.as_super().as_super()
    }

    /// Sets the number of rows and the row height, then updates the frame.
    pub fn set_metrics(&self, rows: usize, row_height: f64) {
        self.ivars().rows.set(rows);
        self.ivars().row_height.set(row_height);
        self.update_frame();
    }

    /// Sets the height at the top that other views cover.
    pub fn set_inset_top(&self, inset: f64) {
        if self.ivars().inset_top.replace(inset) != inset {
            self.update_frame();
            self.setNeedsDisplay(true);
        }
    }

    /// Height = rows x row height, but at least the free height of the clip
    /// view. Width = the width of the clip view.
    pub fn update_frame(&self) {
        // SAFETY: The superview is the clip view, which retains this view.
        let Some(clip) = (unsafe { self.superview() }) else {
            return;
        };
        let clip_size = clip.bounds().size;
        let iv = self.ivars();
        let content = iv.rows.get() as f64 * iv.row_height.get();
        let free = (clip_size.height - iv.inset_top.get()).max(0.0);
        let size = NSSize::new(clip_size.width, content.max(free));
        let current = self.frame().size;
        if current.width != size.width || current.height != size.height {
            self.setFrameSize(size);
        }
    }

    pub fn row_rect(&self, row: usize) -> NSRect {
        let h = self.ivars().row_height.get();
        NSRect::new(
            NSPoint::new(0.0, row as f64 * h),
            NSSize::new(self.bounds().size.width, h),
        )
    }

    /// The part of the visible rect that the toolbar and the header do not
    /// cover. The clip view bounds use the document coordinates; at the top
    /// of the scroll range, their origin is `-inset_top`.
    fn free_rect(&self) -> NSRect {
        let visible = self.visibleRect();
        let Some(scroll) = self.enclosingScrollView() else {
            return visible;
        };
        let clip = scroll.contentView().bounds();
        let top = (clip.origin.y + self.ivars().inset_top.get()).max(visible.origin.y);
        let bottom = visible.origin.y + visible.size.height;
        NSRect::new(
            NSPoint::new(visible.origin.x, top),
            NSSize::new(visible.size.width, (bottom - top).max(0.0)),
        )
    }

    /// Scrolls so that `row` is visible below the covered top, with a
    /// margin of 2 rows.
    pub fn scroll_to_row(&self, row: usize) {
        let Some(scroll) = self.enclosingScrollView() else {
            return;
        };
        let clip = scroll.contentView();
        let bounds = clip.bounds();
        let iv = self.ivars();
        let (rows, h, inset) = (iv.rows.get(), iv.row_height.get(), iv.inset_top.get());
        let free_height = bounds.size.height - inset;
        let mut y = bounds.origin.y;
        if rows == 0 {
            y = -inset;
        } else {
            let row = row.min(rows - 1) as f64;
            let margin = if free_height >= 5.0 * h { 2.0 * h } else { 0.0 };
            let top = row * h - margin;
            let bottom = (row + 1.0) * h + margin;
            if top < bounds.origin.y + inset {
                y = top - inset;
            } else if bottom > bounds.origin.y + bounds.size.height {
                y = bottom - bounds.size.height;
            }
        }
        let max_y = (self.frame().size.height - bounds.size.height).max(-inset);
        let y = y.clamp(-inset, max_y);
        if y != bounds.origin.y {
            clip.scrollToPoint(NSPoint::new(bounds.origin.x, y));
            scroll.reflectScrolledClipView(&clip);
        }
    }

    /// The screen rect of the icon (or of the row, without icons) of `row`.
    /// `None` when the row is not visible.
    pub fn screen_rect_of_row(&self, row: usize, icon: Option<f64>) -> Option<NSRect> {
        let h = self.ivars().row_height.get();
        let y = row as f64 * h;
        let rect = match icon {
            Some(size) => NSRect::new(
                NSPoint::new(LEFT_PAD, y + ((h - size) / 2.0).round()),
                NSSize::new(size, size),
            ),
            None => self.row_rect(row),
        };
        let visible = self.free_rect();
        let top = rect.origin.y + rect.size.height;
        if top <= visible.origin.y || rect.origin.y >= visible.origin.y + visible.size.height {
            return None;
        }
        let window = self.window()?;
        Some(window.convertRectToScreen(self.convertRect_toView(rect, None)))
    }

    /// The frame of the inline rename field over the name of `row`.
    pub fn name_field_frame(&self, row: usize, layout: &ColumnLayout, field_height: f64) -> NSRect {
        let h = self.ivars().row_height.get();
        let y = row as f64 * h + ((h - field_height) / 2.0).round();
        let x = layout.name_x - 3.0;
        NSRect::new(
            NSPoint::new(x, y),
            NSSize::new((layout.name_end - x + 3.0).max(40.0), field_height),
        )
    }

    /// AppKit sends key equivalents to all views in the window. Act only
    /// for the first responder, and only for keys with cmd. A false result
    /// lets the main menu handle the key.
    fn key_equivalent(&self, event: &NSEvent) -> bool {
        if !event
            .modifierFlags()
            .contains(NSEventModifierFlags::Command)
        {
            return false;
        }
        self.is_first_responder() && handle_ns_key(event)
    }

    pub fn is_first_responder(&self) -> bool {
        let Some(window) = self.window() else {
            return false;
        };
        let Some(first) = window.firstResponder() else {
            return false;
        };
        std::ptr::eq(
            Retained::as_ptr(&first).cast::<u8>(),
            (self as *const Self).cast::<u8>(),
        )
    }

    /// True when the list has the keyboard focus in the key window. The
    /// cursor row then uses the active selection color.
    fn is_active(&self) -> bool {
        self.window().is_some_and(|w| w.isKeyWindow()) && self.is_first_responder()
    }

    fn draw(&self, dirty: NSRect) {
        let started = Instant::now();
        let bounds = self.bounds();
        let free = self.free_rect();
        let active = self.is_active();
        let drawn = app::with_ref(|s| draw_list(s, bounds, free, dirty, active)).flatten();
        let Some(rows) = drawn else { return };
        // The draw put paths in the icon queue. Get their icons after the frame.
        icons::schedule_resolve();
        trace!("draw: {rows} rows in {} µs", started.elapsed().as_micros());
        if !FIRST_FRAME.with(|f| f.replace(true)) {
            trace!(
                "startup: first frame after {} ms",
                crate::debug::since_start().as_millis()
            );
        }
    }
}

/// Converts a key-down `NSEvent` and sends it to the keymap. True when the
/// key had a use (an action, a pending sequence, or the end of a sequence).
pub fn handle_ns_key(event: &NSEvent) -> bool {
    let repeat = event.isARepeat();
    key_event_from(event).is_some_and(|key| app::handle_key(key, repeat))
}

/// Builds a `KeyEvent` from a key-down `NSEvent`.
pub fn key_event_from(event: &NSEvent) -> Option<KeyEvent> {
    let flags = event.modifierFlags();
    let mods = Mods {
        cmd: flags.contains(NSEventModifierFlags::Command),
        ctrl: flags.contains(NSEventModifierFlags::Control),
        alt: flags.contains(NSEventModifierFlags::Option),
        shift: flags.contains(NSEventModifierFlags::Shift),
    };
    if let Some(named) = named_key(event.keyCode()) {
        return Some(KeyEvent {
            base: Key::Named(named),
            shifted: None,
            mods,
        });
    }
    let chars = event.charactersByApplyingModifiers(NSEventModifierFlags::empty())?;
    let base = first_char(&chars)?;
    // Function keys without a name use the private use area. Ignore them.
    if ('\u{F700}'..='\u{F8FF}').contains(&base) || base.is_control() {
        return None;
    }
    let shifted = if mods.shift {
        event
            .charactersIgnoringModifiers()
            .and_then(|s| first_char(&s))
    } else {
        None
    };
    Some(KeyEvent {
        base: Key::Char(base),
        shifted,
        mods,
    })
}

fn first_char(s: &NSString) -> Option<char> {
    s.to_string().chars().next()
}

/// The named key for a macOS virtual key code.
fn named_key(code: u16) -> Option<NamedKey> {
    use NamedKey::*;
    Some(match code {
        36 | 76 => Enter,
        53 => Escape,
        49 => Space,
        48 => Tab,
        51 => Backspace,
        117 => Delete,
        126 => Up,
        125 => Down,
        123 => Left,
        124 => Right,
        115 => Home,
        119 => End,
        116 => PageUp,
        121 => PageDown,
        122 => F(1),
        120 => F(2),
        99 => F(3),
        118 => F(4),
        96 => F(5),
        97 => F(6),
        98 => F(7),
        100 => F(8),
        101 => F(9),
        109 => F(10),
        103 => F(11),
        111 => F(12),
        105 => F(13),
        107 => F(14),
        113 => F(15),
        106 => F(16),
        64 => F(17),
        79 => F(18),
        80 => F(19),
        90 => F(20),
        _ => return None,
    })
}

/// Draws the rows in `dirty`. Returns the number of rows, or `None` before
/// the first load. `free` is the visible part below the covered top.
fn draw_list(
    s: &AppState,
    bounds: NSRect,
    free: NSRect,
    dirty: NSRect,
    active: bool,
) -> Option<usize> {
    if !s.pane.is_loaded() {
        return None;
    }
    let st = &s.style;
    let len = s.pane.visible_len();
    if len == 0 {
        let text = if s.pane.filter().is_empty() {
            "Empty folder"
        } else {
            "No matches"
        };
        let y = (free.origin.y + (free.size.height - st.line_height) / 2.0).round();
        let rect = NSRect::new(
            NSPoint::new(free.origin.x, y),
            NSSize::new(free.size.width, st.line_height),
        );
        draw_text(text, rect, &st.empty);
        return Some(0);
    }
    let h = st.row_height;
    let first = (dirty.origin.y / h).floor().max(0.0) as usize;
    let end = (((dirty.origin.y + dirty.size.height) / h).ceil().max(0.0) as usize).min(len);
    let width = bounds.size.width;
    let layout = st.layout(width);
    let frame = RowFrame {
        st,
        width,
        layout: &layout,
        now: SystemTime::now(),
        path_icons: s.cwd_is_local(),
        cursor_state: if active {
            RowState::Selected
        } else {
            RowState::SelectedInactive
        },
    };
    let cursor = s.pane.cursor();
    for row in first..end {
        let Some(entry) = s.pane.visible_entry(row) else {
            break;
        };
        draw_row(&frame, entry, row, row == cursor);
    }
    Some(end.saturating_sub(first))
}

/// The values that are the same for all rows of one draw.
struct RowFrame<'a> {
    st: &'a Style,
    width: f64,
    layout: &'a ColumnLayout,
    now: SystemTime,
    /// Ask for path-specific icons (only on a local volume).
    path_icons: bool,
    /// The state of the cursor row.
    cursor_state: RowState,
}

fn draw_row(frame: &RowFrame<'_>, entry: &Entry, row: usize, is_cursor: bool) {
    let RowFrame {
        st,
        width,
        layout,
        now,
        path_icons,
        cursor_state,
    } = *frame;
    let h = st.row_height;
    let y = row as f64 * h;
    let state = if is_cursor {
        cursor_state
    } else {
        RowState::Normal
    };
    if is_cursor {
        let fill = match state {
            RowState::Selected => &st.colors.selection_bg,
            _ => &st.colors.selection_bg_inactive,
        };
        fill.setFill();
        let rect = NSRect::new(
            NSPoint::new(SELECTION_INSET, y),
            NSSize::new((width - 2.0 * SELECTION_INSET).max(0.0), h),
        );
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            rect,
            SELECTION_RADIUS,
            SELECTION_RADIUS,
        )
        .fill();
    }
    if st.show_icons {
        if let Some(icon) = icons::icon_for(entry, path_icons) {
            let size = st.icon_size;
            let rect = NSRect::new(
                NSPoint::new(LEFT_PAD, y + ((h - size) / 2.0).round()),
                NSSize::new(size, size),
            );
            let alpha = if entry.hidden { 0.5 } else { 1.0 };
            // SAFETY: A `None` hints dictionary is valid. The view is flipped,
            // so `respectFlipped` must be true.
            unsafe {
                icon.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                    rect,
                    NSRect::ZERO,
                    NSCompositingOperation::SourceOver,
                    alpha,
                    true,
                    None,
                );
            }
        }
    }
    let attrs = st.row(state);
    let text_y = y + ((h - st.line_height) / 2.0).round();
    for (spec, x) in &layout.columns {
        let text = columns::cell_text(spec.column, entry, &st.date_format, now);
        let rect = NSRect::new(
            NSPoint::new(*x, text_y),
            NSSize::new(spec.width, st.line_height),
        );
        let column_attrs = if spec.right_aligned {
            &attrs.right
        } else if spec.column == bf_core::config::Column::Kind {
            &attrs.middle
        } else {
            &attrs.left
        };
        draw_text(&text, rect, column_attrs);
    }
    let rect = NSRect::new(
        NSPoint::new(layout.name_x, text_y),
        NSSize::new((layout.name_end - layout.name_x).max(0.0), st.line_height),
    );
    draw_text(&entry.name, rect, attrs.for_entry(entry));
}

/// Draws one line of text. The paragraph style in `attrs` truncates it.
pub fn draw_text(text: &str, rect: NSRect, attrs: &Attrs) {
    if rect.size.width <= 0.0 {
        return;
    }
    // SAFETY: `attrs` has only string keys with the correct value types.
    unsafe { NSString::from_str(text).drawInRect_withAttributes(rect, Some(attrs)) };
}
