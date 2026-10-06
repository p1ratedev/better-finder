//! The column header: the column titles above the list, like Finder.
//!
//! - It stays pinned below the toolbar. The list scrolls under it.
//! - The sort column uses the strong text color and shows a chevron.
//! - A click on the sort column reverses the order. A click on another
//!   column sorts by it. `Kind` sorts by extension.

use bf_core::config::Column;
use bf_core::sort::SortKey;
use objc2::rc::Retained;
use objc2::{MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSBezierPath, NSEvent, NSResponder, NSView};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize};

use crate::app::{self, AppState};
use crate::columns;
use crate::icons;
use crate::list_view::draw_text;
use crate::theme::{self, COLUMN_GAP, ColumnLayout, LEFT_PAD};

/// A header cell: the sort key that a click selects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Name,
    Column(Column),
}

impl Target {
    pub fn sort_key(self) -> Option<SortKey> {
        Some(match self {
            Target::Name => SortKey::Name,
            Target::Column(Column::Size) => SortKey::Size,
            Target::Column(Column::Kind | Column::Extension) => SortKey::Extension,
            Target::Column(Column::Modified) => SortKey::Modified,
            Target::Column(Column::Created) => SortKey::Created,
        })
    }
}

/// The header cell under `x`.
pub fn target_at(layout: &ColumnLayout, x: f64) -> Target {
    for (spec, cx) in layout.columns.iter().rev() {
        if x >= cx - COLUMN_GAP / 2.0 {
            return Target::Column(spec.column);
        }
    }
    Target::Name
}

define_class!(
    // SAFETY:
    // - NSView has no subclass requirements.
    // - HeaderView does not implement Drop.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BFHeaderView"]
    pub struct HeaderView;

    impl HeaderView {
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
        fn mouse_down(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            let width = self.bounds().size.width;
            let target = app::with_ref(|s| target_at(&s.style.layout(width), point.x));
            if let Some(target) = target {
                app::header_click(target);
            }
        }
    }
);

impl HeaderView {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 24.0));
        // SAFETY: `initWithFrame:` is the designated initializer of NSView.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

fn draw(s: &AppState, bounds: NSRect) {
    let st = &s.style;
    st.colors.list_bg.setFill();
    NSBezierPath::fillRect(bounds);
    st.colors.separator.setFill();
    NSBezierPath::fillRect(NSRect::new(
        NSPoint::new(0.0, bounds.size.height - 1.0),
        NSSize::new(bounds.size.width, 1.0),
    ));

    let layout = st.layout(bounds.size.width);
    let sort = s.pane.sort();
    let line = st.small_line_height;
    let y = ((bounds.size.height - 1.0 - line) / 2.0).round();
    let chevron_size = (line * 0.55).round();

    let mut cells: Vec<(Target, f64, f64, bool)> = vec![(
        Target::Name,
        LEFT_PAD,
        (layout.name_end - LEFT_PAD).max(0.0),
        false,
    )];
    for (spec, x) in &layout.columns {
        cells.push((
            Target::Column(spec.column),
            *x,
            spec.width,
            spec.right_aligned,
        ));
    }
    for (target, x, width, right_aligned) in cells {
        let title = match target {
            Target::Name => "Name",
            Target::Column(c) => columns::title(c),
        };
        let sorted = match target {
            Target::Column(Column::Kind) => {
                sort.key == SortKey::Extension
                    && !layout
                        .columns
                        .iter()
                        .any(|(c, _)| c.column == Column::Extension)
            }
            other => other.sort_key() == Some(sort.key),
        };
        let attrs = if sorted { &st.header_sort } else { &st.header };
        let text_w = theme::text_width(title, attrs).ceil() + 1.0;
        let chevron_w = if sorted { chevron_size + 4.0 } else { 0.0 };
        let total = (text_w + chevron_w).min(width);
        let start = if right_aligned { x + width - total } else { x };
        draw_text(
            title,
            NSRect::new(
                NSPoint::new(start, y),
                NSSize::new((total - chevron_w).max(0.0), line),
            ),
            attrs,
        );
        if sorted {
            // Dates sort newest first: that is the "down" direction.
            let newest_first = matches!(sort.key, SortKey::Modified | SortKey::Created);
            let down = sort.reverse != newest_first;
            let name = if down { "chevron.down" } else { "chevron.up" };
            if let Some(image) = icons::symbol(name, chevron_size, &st.colors.header_sort_fg) {
                let rect = NSRect::new(
                    NSPoint::new(start + total - chevron_w + 4.0, y),
                    NSSize::new(chevron_size, line),
                );
                icons::draw_centered(&image, rect);
            }
        }
        if target != Target::Name {
            // A thin separator line before the column.
            st.colors.separator.setFill();
            let sx = (x - COLUMN_GAP / 2.0).round();
            NSBezierPath::fillRect(NSRect::new(
                NSPoint::new(sx, 5.0),
                NSSize::new(1.0, (bounds.size.height - 11.0).max(0.0)),
            ));
        }
    }
}
