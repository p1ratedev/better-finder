//! The file list: a custom `NSView` inside an `NSScrollView`.
//!
//! - It draws only the rows in the dirty rect.
//! - It converts key events to `bf_core::KeyEvent` and sends them to the keymap.
//! - It handles the mouse.
//! - It is the controller of the Quick Look panel (it is in the responder chain).

use std::cell::Cell;
use std::time::{Instant, SystemTime};

use bf_core::config::Column;
use bf_core::format::{format_time, human_size};
use bf_core::keymap::{Key, Mods, NamedKey};
use bf_core::{Entry, EntryKind, KeyEvent};
use objc2::rc::Retained;
use objc2::{ClassType, DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSCompositingOperation, NSEvent, NSEventModifierFlags, NSResponder,
    NSStringDrawing, NSView,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString};
use objc2_quick_look_ui::QLPreviewPanel;

use crate::app::{self, AppState};
use crate::debug::trace;
use crate::icons;
use crate::quicklook;
use crate::theme::{Attrs, COLUMN_GAP, ColumnSpec, LEFT_PAD, RIGHT_PAD, Style};

pub struct ListIvars {
    row_height: Cell<f64>,
    rows: Cell<usize>,
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
            row_height: Cell::new(22.0),
            rows: Cell::new(0),
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

    /// Height = rows x row height, but at least the height of the clip view.
    /// Width = the width of the clip view.
    pub fn update_frame(&self) {
        // SAFETY: The superview is the clip view, which retains this view.
        let Some(clip) = (unsafe { self.superview() }) else {
            return;
        };
        let clip_size = clip.bounds().size;
        let iv = self.ivars();
        let content = iv.rows.get() as f64 * iv.row_height.get();
        let size = NSSize::new(clip_size.width, content.max(clip_size.height));
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

    /// Scrolls so that `row` is visible with a margin of 2 rows.
    pub fn scroll_to_row(&self, row: usize) {
        let iv = self.ivars();
        let (rows, h) = (iv.rows.get(), iv.row_height.get());
        if rows == 0 {
            self.scrollRectToVisible(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0)));
            return;
        }
        let row = row.min(rows - 1);
        let visible = self.visibleRect().size.height;
        let (first, last) = if visible >= 5.0 * h {
            (row.saturating_sub(2), (row + 3).min(rows))
        } else {
            (row, row + 1)
        };
        let rect = NSRect::new(
            NSPoint::new(0.0, first as f64 * h),
            NSSize::new(1.0, (last - first) as f64 * h),
        );
        self.scrollRectToVisible(rect);
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
        let visible = self.visibleRect();
        let top = rect.origin.y + rect.size.height;
        if top <= visible.origin.y || rect.origin.y >= visible.origin.y + visible.size.height {
            return None;
        }
        let window = self.window()?;
        Some(window.convertRectToScreen(self.convertRect_toView(rect, None)))
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

    fn is_first_responder(&self) -> bool {
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

    fn draw(&self, dirty: NSRect) {
        let started = Instant::now();
        let bounds = self.bounds();
        let visible = self.visibleRect();
        let drawn = app::with_ref(|s| draw_list(s, bounds, visible, dirty)).flatten();
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
/// the first load.
fn draw_list(s: &AppState, bounds: NSRect, visible: NSRect, dirty: NSRect) -> Option<usize> {
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
        let y = (visible.origin.y + (visible.size.height - st.line_height) / 2.0).round();
        let rect = NSRect::new(
            NSPoint::new(visible.origin.x, y),
            NSSize::new(visible.size.width, st.line_height),
        );
        draw_text(text, rect, &st.empty);
        return Some(0);
    }
    let h = st.row_height;
    let first = (dirty.origin.y / h).floor().max(0.0) as usize;
    let end = (((dirty.origin.y + dirty.size.height) / h).ceil().max(0.0) as usize).min(len);
    let width = bounds.size.width;
    let frame = RowFrame {
        st,
        width,
        columns: st.visible_columns(width),
        now: SystemTime::now(),
        path_icons: s.cwd_is_local(),
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
    columns: &'a [ColumnSpec],
    now: SystemTime,
    /// Ask for path-specific icons (only on a local volume).
    path_icons: bool,
}

fn draw_row(frame: &RowFrame<'_>, entry: &Entry, row: usize, selected: bool) {
    let RowFrame {
        st,
        width,
        columns,
        now,
        path_icons,
    } = *frame;
    let h = st.row_height;
    let y = row as f64 * h;
    if selected {
        st.selection_bg.setFill();
        let rect = NSRect::new(
            NSPoint::new(6.0, y + 1.0),
            NSSize::new((width - 12.0).max(0.0), (h - 2.0).max(0.0)),
        );
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, 5.0, 5.0).fill();
    }
    if st.show_icons {
        if let Some(icon) = icons::icon_for(entry, path_icons) {
            let size = st.icon_size;
            let rect = NSRect::new(
                NSPoint::new(LEFT_PAD, y + ((h - size) / 2.0).round()),
                NSSize::new(size, size),
            );
            let alpha = if entry.hidden { 0.55 } else { 1.0 };
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
    let text_y = y + ((h - st.line_height) / 2.0).round();
    let mut right = width - RIGHT_PAD;
    for col in columns.iter().rev() {
        let text = column_text(col.column, entry, &st.date_format, now);
        let rect = NSRect::new(
            NSPoint::new(right - col.width, text_y),
            NSSize::new(col.width, st.line_height),
        );
        let attrs = if selected {
            &st.column_selected
        } else {
            &st.column
        };
        draw_text(&text, rect, attrs);
        right -= col.width + COLUMN_GAP;
    }
    let x = st.name_x();
    let rect = NSRect::new(
        NSPoint::new(x, text_y),
        NSSize::new((right - x).max(0.0), st.line_height),
    );
    draw_text(&entry.name, rect, st.name.for_entry(entry, selected));
}

fn column_text(column: Column, entry: &Entry, date_format: &str, now: SystemTime) -> String {
    match column {
        Column::Size => {
            if entry.is_dir || entry.is_package || entry.kind == EntryKind::Directory {
                "--".to_string()
            } else {
                human_size(entry.size)
            }
        }
        Column::Modified => entry
            .modified
            .map(|t| format_time(t, date_format, now))
            .unwrap_or_else(|| "--".to_string()),
        Column::Extension => {
            if entry.kind == EntryKind::Directory && !entry.is_package {
                String::new()
            } else {
                entry.extension().to_string()
            }
        }
    }
}

/// Draws one line of text. The paragraph style in `attrs` truncates it.
pub fn draw_text(text: &str, rect: NSRect, attrs: &Attrs) {
    if rect.size.width <= 0.0 {
        return;
    }
    // SAFETY: `attrs` has only string keys with the correct value types.
    unsafe { NSString::from_str(text).drawInRect_withAttributes(rect, Some(attrs)) };
}
