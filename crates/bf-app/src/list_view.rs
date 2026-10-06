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
//! - During a smooth scroll, a small subview (`HighlightView`) shows the
//!   cursor highlight at the animated position, and the list draws no
//!   cursor row. The subview fills the highlight and draws the rows under
//!   it again in the selection colors, clipped to the highlight. So the
//!   text under the moving highlight stays readable, and a frame redraws
//!   only the subview and the rows that the scroll shows.

use std::cell::{Cell, OnceCell};
use std::time::{Instant, SystemTime};

use bf_core::keymap::{Key, Mods, NamedKey};
use bf_core::viewport::Viewport;
use bf_core::{Entry, KeyEvent};
use objc2::rc::Retained;
use objc2::{ClassType, DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSEvent, NSEventModifierFlags, NSResponder, NSStringDrawing, NSView,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString};
use objc2_quick_look_ui::QLPreviewPanel;

use crate::app::{self, AppState};
use crate::columns;
use crate::debug::trace;
use crate::icons;
use crate::quicklook;
use crate::smooth;
use crate::theme::{
    Attrs, ColumnLayout, LEFT_PAD, RowState, SELECTION_INSET, SELECTION_RADIUS, Style,
};

/// The view of the list in rows.
#[derive(Clone, Copy, Debug)]
pub struct ViewRows {
    /// The first visible row below the covered top. It is a fraction of a
    /// row after a mouse scroll.
    pub top: f64,
    /// The visible height below the covered top, in rows.
    pub height: f64,
    pub row_height: f64,
    /// The height that the toolbar and the column header cover.
    pub inset: f64,
}

pub struct ListIvars {
    /// The cursor highlight during a smooth scroll. Made at the first use.
    highlight: OnceCell<Retained<HighlightView>>,
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
            // A mouse scroll ends a smooth scroll at its target.
            smooth::finish();
            // A scroll cancels the inline rename, like in Finder.
            app::after_now(app::cancel_inline_rename);
            // SAFETY: NSResponder implements this method with this signature.
            let _: () = unsafe { msg_send![super(self), scrollWheel: event] };
        }

        #[unsafe(method(resizeWithOldSuperviewSize:))]
        fn resize_with_old_superview_size(&self, old: NSSize) {
            self.superview_resized(old);
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

pub struct HighlightIvars {
    /// The list has the focus in the key window: the active color.
    active: Cell<bool>,
}

define_class!(
    // SAFETY:
    // - NSView has no subclass requirements.
    // - HighlightView does not implement Drop.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BFHighlightView"]
    #[ivars = HighlightIvars]
    pub struct HighlightView;

    impl HighlightView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let started = Instant::now();
            let bounds = self.bounds();
            let active = self.ivars().active.get();
            let scale = self.window().map_or(2.0, |w| w.backingScaleFactor());
            let rows = app::with_ref(|s| draw_highlight(s, bounds, active, scale));
            smooth::note_draw(started.elapsed());
            trace!(
                "draw: highlight, {} rows in {} µs",
                rows.unwrap_or(0),
                started.elapsed().as_micros()
            );
        }
    }
);

impl HighlightView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(HighlightIvars {
            active: Cell::new(true),
        });
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 20.0));
        // SAFETY: `initWithFrame:` is the designated initializer of NSView.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

impl ListView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ListIvars {
            highlight: OnceCell::new(),
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

    /// The clip view changed its size (a window resize, or a bar that shows
    /// or hides). A smooth scroll ends at its target.
    fn superview_resized(&self, old: NSSize) {
        // SAFETY: The superview is the clip view, which retains this view.
        let changed = unsafe { self.superview() }.is_some_and(|clip| clip.bounds().size != old);
        if changed {
            smooth::finish();
        }
        // SAFETY: NSView implements this method with this signature.
        let _: () = unsafe { msg_send![super(self), resizeWithOldSuperviewSize: old] };
        self.update_frame();
    }

    /// The view in rows. See `ViewRows`.
    pub fn view_rows(&self) -> Option<ViewRows> {
        let scroll = self.enclosingScrollView()?;
        let bounds = scroll.contentView().bounds();
        let iv = self.ivars();
        let (h, inset) = (iv.row_height.get(), iv.inset_top.get());
        if h <= 0.0 {
            return None;
        }
        Some(ViewRows {
            top: (bounds.origin.y + inset) / h,
            height: ((bounds.size.height - inset) / h).max(0.0),
            row_height: h,
            inset,
        })
    }

    /// Shows the cursor highlight of a smooth scroll at `y` (document
    /// coordinates), or hides it (`None`). Only the highlight view redraws:
    /// the move of a subview does not redraw the list.
    pub fn show_highlight(&self, y: Option<f64>) {
        let Some(y) = y else {
            if let Some(view) = self.ivars().highlight.get() {
                if !view.isHidden() {
                    view.setHidden(true);
                }
            }
            return;
        };
        let view = self.ivars().highlight.get_or_init(|| {
            let view = HighlightView::new(self.mtm());
            self.addSubview(&view);
            view
        });
        let h = self.ivars().row_height.get();
        let width = self.bounds().size.width;
        view.ivars().active.set(self.is_active());
        view.setFrame(NSRect::new(NSPoint::new(0.0, y), NSSize::new(width, h)));
        // The bounds use the document coordinates of the list, so the rows
        // draw at their own y.
        view.setBoundsOrigin(NSPoint::new(0.0, y));
        view.setHidden(false);
        view.setNeedsDisplay(true);
    }

    /// Redraws the band of one row height at `y` (document coordinates).
    pub fn redraw_band(&self, y: f64, height: f64) {
        let width = self.bounds().size.width;
        self.setNeedsDisplayInRect(NSRect::new(
            NSPoint::new(0.0, y),
            NSSize::new(width, height),
        ));
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
    /// margin of 2 rows (`Viewport::reveal`).
    pub fn scroll_to_row(&self, row: usize) {
        let Some(scroll) = self.enclosingScrollView() else {
            return;
        };
        let Some(view) = self.view_rows() else {
            return;
        };
        let clip = scroll.contentView();
        let bounds = clip.bounds();
        let (h, inset) = (view.row_height, view.inset);
        let viewport = Viewport {
            len: self.ivars().rows.get(),
            height: view.height,
        };
        let top = view.top;
        let new_top = viewport.reveal(top, row);
        if (new_top - top).abs() < 1e-6 {
            return;
        }
        let max_y = (self.frame().size.height - bounds.size.height).max(-inset);
        let y = (new_top * h - inset).clamp(-inset, max_y);
        if (y - bounds.origin.y).abs() > 0.001 {
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
        let needs = |rect| self.needsToDrawRect(rect);
        let input = ListDraw {
            bounds,
            free,
            dirty,
            active: self.is_active(),
            animating: smooth::list_highlight().is_some(),
            scale: self.window().map_or(2.0, |w| w.backingScaleFactor()),
            needs: &needs,
        };
        let drawn = app::with_ref(|s| draw_list(s, &input)).flatten();
        smooth::note_draw(started.elapsed());
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

/// The inputs of one draw of the list.
struct ListDraw<'a> {
    bounds: NSRect,
    /// The visible part below the covered top.
    free: NSRect,
    dirty: NSRect,
    /// The list has the focus in the key window.
    active: bool,
    /// A smooth scroll runs: the highlight view shows the cursor.
    animating: bool,
    /// The backing scale factor, for the icon bitmaps.
    scale: f64,
    /// `needsToDrawRect:`. The dirty region can be smaller than the `dirty`
    /// rect (for example the highlight and the strip that a scroll shows).
    needs: &'a dyn Fn(NSRect) -> bool,
}

/// Draws the rows in the dirty region. Returns the number of rows, or
/// `None` before the first load.
fn draw_list(s: &AppState, input: &ListDraw<'_>) -> Option<usize> {
    let ListDraw {
        bounds,
        free,
        dirty,
        active,
        animating,
        scale,
        needs,
    } = *input;
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
        scale,
    };
    let row_rect =
        |row: usize| NSRect::new(NSPoint::new(0.0, row as f64 * h), NSSize::new(width, h));
    // During a smooth scroll, the highlight view shows the cursor, and all
    // rows draw in the normal colors.
    let cursor = (!animating).then(|| s.pane.cursor());
    let mut drawn = 0;
    for row in first..end {
        if !needs(row_rect(row)) {
            continue;
        }
        let Some(entry) = s.pane.visible_entry(row) else {
            break;
        };
        draw_row(&frame, entry, row, cursor == Some(row));
        drawn += 1;
    }
    Some(drawn)
}

/// Draws the cursor highlight of a smooth scroll: `bounds` is the frame of
/// the highlight view in the document coordinates of the list. It fills the
/// highlight and draws the rows under it in the selection colors, clipped
/// to the highlight. Returns the number of rows.
fn draw_highlight(s: &AppState, bounds: NSRect, active: bool, scale: f64) -> usize {
    if !s.pane.is_loaded() {
        return 0;
    }
    let st = &s.style;
    let h = st.row_height;
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
        scale,
    };
    let y = bounds.origin.y;
    let rect = highlight_rect(width, y, h);
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
        rect,
        SELECTION_RADIUS,
        SELECTION_RADIUS,
    )
    .addClip();
    fill_highlight(&frame, rect);
    let len = s.pane.visible_len();
    let top_row = (y / h).floor().max(0.0) as usize;
    let bottom_row = (((y + h) / h).ceil().max(0.0) as usize).min(len);
    let mut drawn = 0;
    for row in top_row..bottom_row {
        if let Some(entry) = s.pane.visible_entry(row) {
            draw_row_content(&frame, entry, row, frame.cursor_state);
            drawn += 1;
        }
    }
    drawn
}

/// The rounded rectangle of the cursor highlight at `y`.
fn highlight_rect(width: f64, y: f64, h: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(SELECTION_INSET, y),
        NSSize::new((width - 2.0 * SELECTION_INSET).max(0.0), h),
    )
}

/// Fills the cursor highlight in the color of the cursor state.
fn fill_highlight(frame: &RowFrame<'_>, rect: NSRect) {
    let fill = match frame.cursor_state {
        RowState::Selected => &frame.st.colors.selection_bg,
        _ => &frame.st.colors.selection_bg_inactive,
    };
    fill.setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
        rect,
        SELECTION_RADIUS,
        SELECTION_RADIUS,
    )
    .fill();
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
    /// The backing scale factor of the window, for the icon bitmaps.
    scale: f64,
}

fn draw_row(frame: &RowFrame<'_>, entry: &Entry, row: usize, is_cursor: bool) {
    let h = frame.st.row_height;
    let y = row as f64 * h;
    if is_cursor {
        fill_highlight(frame, highlight_rect(frame.width, y, h));
        draw_row_content(frame, entry, row, frame.cursor_state);
    } else {
        draw_row_content(frame, entry, row, RowState::Normal);
    }
}

/// Draws the icon and the text of one row, without a background.
fn draw_row_content(frame: &RowFrame<'_>, entry: &Entry, row: usize, state: RowState) {
    let RowFrame {
        st,
        layout,
        now,
        path_icons,
        ..
    } = *frame;
    let h = st.row_height;
    let y = row as f64 * h;
    if st.show_icons {
        if let Some(icon) = icons::icon_for(entry, path_icons) {
            let size = st.icon_size;
            let rect = NSRect::new(
                NSPoint::new(LEFT_PAD, y + ((h - size) / 2.0).round()),
                NSSize::new(size, size),
            );
            let alpha = if entry.hidden { 0.5 } else { 1.0 };
            icons::draw_icon(&icon, rect, alpha, frame.scale);
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
