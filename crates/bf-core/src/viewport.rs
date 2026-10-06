//! Scroll arithmetic for the list and the help overlay, with the vim rules.
//!
//! All values are in rows. `top` is the first visible row. It can be a
//! fraction of a row, for example after a scroll with the mouse.
//!
//! - `half_page_down`: `n = max(1, visible_rows / 2)`. The view scrolls
//!   down by `min(n, rows below the view)`. The cursor moves down by `n`,
//!   clamped to the last row. So the cursor keeps its row on the screen
//!   until the view reaches the end. `half_page_up` is the same upward.
//! - `page_down` and `page_up`: the same rule with `n = max(1, visible_rows - 2)`.
//! - `cursor_top` and `cursor_bottom`: the view and the cursor go to the
//!   start or the end.
//! - `cursor_down` and `cursor_up`: the cursor moves one row. The view
//!   scrolls only when the cursor reaches the margin of 2 rows.

use crate::Action;

/// The margin between the cursor and the edge of the view for line moves.
pub const MARGIN: usize = 2;

/// A move of the view (and of the cursor in the list).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    LineDown,
    LineUp,
    HalfPageDown,
    HalfPageUp,
    PageDown,
    PageUp,
    Top,
    Bottom,
}

impl Motion {
    /// The motion of a cursor action. `None` for all other actions.
    pub fn from_action(action: &Action) -> Option<Motion> {
        Some(match action {
            Action::CursorDown => Motion::LineDown,
            Action::CursorUp => Motion::LineUp,
            Action::HalfPageDown => Motion::HalfPageDown,
            Action::HalfPageUp => Motion::HalfPageUp,
            Action::PageDown => Motion::PageDown,
            Action::PageUp => Motion::PageUp,
            Action::CursorTop => Motion::Top,
            Action::CursorBottom => Motion::Bottom,
            _ => return None,
        })
    }

    /// True for the motions that animate. The line moves do not animate,
    /// so that `j`, `k`, and key repeat stay instant.
    pub fn animates(self) -> bool {
        !matches!(self, Motion::LineDown | Motion::LineUp)
    }

    /// True for the motions toward the end.
    fn is_down(self) -> bool {
        matches!(
            self,
            Motion::LineDown | Motion::HalfPageDown | Motion::PageDown | Motion::Bottom
        )
    }

    /// The number of rows of one step, for a view of `height` rows.
    /// `Top` and `Bottom` have no step (0).
    pub fn step(self, height: f64) -> usize {
        let rows = visible_rows(height);
        match self {
            Motion::LineDown | Motion::LineUp => 1,
            Motion::HalfPageDown | Motion::HalfPageUp => (rows / 2).max(1),
            Motion::PageDown | Motion::PageUp => rows.saturating_sub(2).max(1),
            Motion::Top | Motion::Bottom => 0,
        }
    }
}

/// The number of full rows in a view of `height` rows.
fn visible_rows(height: f64) -> usize {
    if height.is_finite() && height > 0.0 {
        // A small tolerance: 29.9999999 rows are 30 rows.
        (height + 1e-6).floor() as usize
    } else {
        0
    }
}

/// The view position and the cursor of the list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    pub cursor: usize,
    pub top: f64,
}

/// A list of `len` rows in a view of `height` rows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub len: usize,
    pub height: f64,
}

impl Viewport {
    /// The largest `top`: the last row is at the bottom of the view. It is
    /// 0 when the list is shorter than the view.
    pub fn max_top(&self) -> f64 {
        (self.len as f64 - self.height.max(0.0)).max(0.0)
    }

    /// The margin for line moves: 2 rows, or 0 when the view has fewer
    /// than 5 rows.
    pub fn margin(&self) -> usize {
        if visible_rows(self.height) >= 5 {
            MARGIN
        } else {
            0
        }
    }

    /// Applies `motion` to `from`. The result has a valid cursor and a
    /// `top` in `0..=max_top`. An empty list gives cursor 0 and top 0.
    pub fn apply(&self, from: Position, motion: Motion) -> Position {
        if self.len == 0 {
            return Position {
                cursor: 0,
                top: 0.0,
            };
        }
        let last = self.len - 1;
        let max_top = self.max_top();
        let cursor = from.cursor.min(last);
        let top = clamp_top(from.top, max_top);
        match motion {
            Motion::Top => Position {
                cursor: 0,
                top: 0.0,
            },
            Motion::Bottom => Position {
                cursor: last,
                top: max_top,
            },
            Motion::LineDown | Motion::LineUp => {
                let cursor = if motion.is_down() {
                    (cursor + 1).min(last)
                } else {
                    cursor.saturating_sub(1)
                };
                Position {
                    cursor,
                    top: self.reveal(top, cursor),
                }
            }
            _ => {
                let n = motion.step(self.height);
                let (cursor, top) = if motion.is_down() {
                    let scroll = (n as f64).min(max_top - top).max(0.0);
                    ((cursor + n).min(last), top + scroll)
                } else {
                    let scroll = (n as f64).min(top).max(0.0);
                    (cursor.saturating_sub(n), top - scroll)
                };
                Position {
                    cursor,
                    top: self.keep_visible(top, cursor),
                }
            }
        }
    }

    /// The `top` that shows the `cursor` row with the margin. It does not
    /// change `top` when the cursor is already in the area inside the margin.
    pub fn reveal(&self, top: f64, cursor: usize) -> f64 {
        if self.len == 0 {
            return 0.0;
        }
        let row = cursor.min(self.len - 1) as f64;
        let margin = self.margin() as f64;
        let height = self.height.max(0.0);
        let mut top = top;
        if row - margin < top {
            top = row - margin;
        } else if row + 1.0 + margin > top + height {
            top = row + 1.0 + margin - height;
        }
        clamp_top(top, self.max_top())
    }

    /// The `top` that shows the `cursor` row, without a margin. It does not
    /// change `top` when the cursor row is fully visible.
    fn keep_visible(&self, top: f64, cursor: usize) -> f64 {
        let row = cursor as f64;
        let height = self.height.max(0.0);
        let mut top = top;
        if row < top {
            top = row;
        } else if row + 1.0 > top + height {
            top = row + 1.0 - height;
        }
        clamp_top(top, self.max_top())
    }
}

/// The new `top` of a view without a cursor (the help overlay). `content`
/// and `height` are in rows. The line moves scroll by one row.
pub fn scroll_view(top: f64, content: f64, height: f64, motion: Motion) -> f64 {
    let max_top = (content - height.max(0.0)).max(0.0);
    let top = clamp_top(top, max_top);
    let n = motion.step(height) as f64;
    let target = match motion {
        Motion::Top => 0.0,
        Motion::Bottom => max_top,
        m if m.is_down() => top + n,
        _ => top - n,
    };
    clamp_top(target, max_top)
}

fn clamp_top(top: f64, max_top: f64) -> f64 {
    if top.is_finite() {
        top.clamp(0.0, max_top.max(0.0))
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(cursor: usize, top: f64) -> Position {
        Position { cursor, top }
    }

    /// 300 rows in a view of 30 rows (like a fixture folder).
    const LIST: Viewport = Viewport {
        len: 300,
        height: 30.0,
    };

    #[test]
    fn steps() {
        assert_eq!(Motion::HalfPageDown.step(30.0), 15);
        assert_eq!(Motion::PageDown.step(30.0), 28);
        assert_eq!(Motion::LineDown.step(30.0), 1);
        // A part of a row does not count.
        assert_eq!(Motion::HalfPageDown.step(31.9), 15);
        assert_eq!(Motion::PageUp.step(31.9), 29);
        // Float noise does not lose a row.
        assert_eq!(Motion::PageDown.step(29.999_999_9), 28);
        // n is at least 1.
        for height in [0.0, 0.5, 1.0, 2.0, 3.0, f64::NAN, -4.0] {
            assert_eq!(Motion::HalfPageDown.step(height), 1, "{height}");
            assert_eq!(Motion::PageDown.step(height), 1, "{height}");
        }
        assert_eq!(Motion::Top.step(30.0), 0);
    }

    #[test]
    fn half_page_down_from_the_middle_keeps_the_screen_row() {
        let to = LIST.apply(pos(110, 100.0), Motion::HalfPageDown);
        assert_eq!(to, pos(125, 115.0));
        // The cursor is 10 rows below the top, before and after.
        assert_eq!(to.cursor as f64 - to.top, 10.0);
        let back = LIST.apply(to, Motion::HalfPageUp);
        assert_eq!(back, pos(110, 100.0));
    }

    #[test]
    fn page_down_and_up() {
        assert_eq!(
            LIST.apply(pos(110, 100.0), Motion::PageDown),
            pos(138, 128.0)
        );
        assert_eq!(LIST.apply(pos(138, 128.0), Motion::PageUp), pos(110, 100.0));
    }

    #[test]
    fn a_fraction_of_a_row_stays() {
        let to = LIST.apply(pos(110, 100.5), Motion::HalfPageDown);
        assert_eq!(to, pos(125, 115.5));
    }

    #[test]
    fn at_the_end_the_view_stops_and_the_cursor_moves() {
        // The view can scroll 5 more rows.
        let to = LIST.apply(pos(280, 265.0), Motion::HalfPageDown);
        assert_eq!(to, pos(295, 270.0));
        // The view is at the end. The cursor clamps to the last row.
        let to = LIST.apply(to, Motion::HalfPageDown);
        assert_eq!(to, pos(299, 270.0));
        // The last row: nothing changes.
        assert_eq!(LIST.apply(to, Motion::HalfPageDown), to);
        assert_eq!(LIST.apply(to, Motion::PageDown), to);
    }

    #[test]
    fn at_the_start_the_view_stops_and_the_cursor_moves() {
        let to = LIST.apply(pos(10, 3.0), Motion::HalfPageUp);
        assert_eq!(to, pos(0, 0.0));
        let to = LIST.apply(pos(5, 0.0), Motion::HalfPageUp);
        assert_eq!(to, pos(0, 0.0));
        // The first row: nothing changes.
        assert_eq!(LIST.apply(pos(0, 0.0), Motion::PageUp), pos(0, 0.0));
    }

    #[test]
    fn top_and_bottom() {
        assert_eq!(LIST.apply(pos(150, 140.0), Motion::Top), pos(0, 0.0));
        assert_eq!(LIST.apply(pos(150, 140.0), Motion::Bottom), pos(299, 270.0));
    }

    #[test]
    fn list_shorter_than_the_view() {
        let short = Viewport {
            len: 10,
            height: 30.0,
        };
        assert_eq!(short.max_top(), 0.0);
        assert_eq!(short.apply(pos(0, 0.0), Motion::HalfPageDown), pos(9, 0.0));
        assert_eq!(short.apply(pos(2, 0.0), Motion::PageDown), pos(9, 0.0));
        assert_eq!(short.apply(pos(9, 0.0), Motion::HalfPageUp), pos(0, 0.0));
        assert_eq!(short.apply(pos(4, 0.0), Motion::Bottom), pos(9, 0.0));
        // A top from a larger list clamps to 0.
        assert_eq!(short.apply(pos(4, 12.0), Motion::LineDown), pos(5, 0.0));
    }

    #[test]
    fn short_list_and_short_view() {
        let one = Viewport {
            len: 1,
            height: 30.0,
        };
        for motion in [
            Motion::LineDown,
            Motion::LineUp,
            Motion::HalfPageDown,
            Motion::PageUp,
            Motion::Top,
            Motion::Bottom,
        ] {
            assert_eq!(one.apply(pos(0, 0.0), motion), pos(0, 0.0));
        }
        // A view of 1 row: n is 1, and the cursor stays visible.
        let tiny = Viewport {
            len: 50,
            height: 1.0,
        };
        assert_eq!(
            tiny.apply(pos(10, 10.0), Motion::HalfPageDown),
            pos(11, 11.0)
        );
        assert_eq!(tiny.apply(pos(10, 10.0), Motion::PageUp), pos(9, 9.0));
    }

    #[test]
    fn empty_list() {
        let empty = Viewport {
            len: 0,
            height: 30.0,
        };
        assert_eq!(empty.apply(pos(5, 7.0), Motion::PageDown), pos(0, 0.0));
        assert_eq!(empty.reveal(7.0, 3), 0.0);
    }

    #[test]
    fn a_cursor_outside_the_list_clamps() {
        assert_eq!(LIST.apply(pos(900, 0.0), Motion::LineUp).cursor, 298);
        // The view scrolls to show the clamped cursor at the bottom.
        assert_eq!(
            LIST.apply(pos(900, 0.0), Motion::HalfPageUp),
            pos(284, 255.0)
        );
    }

    #[test]
    fn a_cursor_outside_the_view_becomes_visible() {
        // The mouse scrolled the view away from the cursor.
        let to = LIST.apply(pos(10, 200.0), Motion::HalfPageDown);
        assert_eq!(to.cursor, 25);
        assert!(to.top <= 25.0 && 25.0 < to.top + 30.0, "{to:?}");
    }

    #[test]
    fn line_moves_scroll_one_row_at_the_margin() {
        // The cursor is 3 rows above the bottom edge: no scroll.
        assert_eq!(
            LIST.apply(pos(126, 100.0), Motion::LineDown),
            pos(127, 100.0)
        );
        // The next row is in the margin: the view scrolls one row.
        assert_eq!(
            LIST.apply(pos(127, 100.0), Motion::LineDown),
            pos(128, 101.0)
        );
        // The same at the top edge.
        assert_eq!(LIST.apply(pos(103, 100.0), Motion::LineUp), pos(102, 100.0));
        assert_eq!(LIST.apply(pos(102, 100.0), Motion::LineUp), pos(101, 99.0));
        // No margin at the ends of the list.
        assert_eq!(LIST.apply(pos(1, 0.0), Motion::LineUp), pos(0, 0.0));
        assert_eq!(
            LIST.apply(pos(298, 270.0), Motion::LineDown),
            pos(299, 270.0)
        );
    }

    #[test]
    fn reveal_rules() {
        // Inside the margin area: no change.
        assert_eq!(LIST.reveal(100.0, 115), 100.0);
        // Above the view: the cursor goes 2 rows below the top.
        assert_eq!(LIST.reveal(100.0, 50), 48.0);
        // Below the view: the cursor goes 2 rows above the bottom.
        assert_eq!(LIST.reveal(100.0, 200), 173.0);
        // Clamped to the ends.
        assert_eq!(LIST.reveal(100.0, 0), 0.0);
        assert_eq!(LIST.reveal(100.0, 299), 270.0);
        // A view of fewer than 5 rows has no margin.
        let small = Viewport {
            len: 300,
            height: 4.0,
        };
        assert_eq!(small.margin(), 0);
        assert_eq!(small.reveal(10.0, 14), 11.0);
        assert_eq!(small.reveal(10.0, 13), 10.0);
    }

    #[test]
    fn view_without_a_cursor() {
        // The help overlay: 50.5 rows of content in a view of 20 rows.
        let (content, height) = (50.5, 20.0);
        assert_eq!(scroll_view(0.0, content, height, Motion::LineDown), 1.0);
        assert_eq!(
            scroll_view(0.0, content, height, Motion::HalfPageDown),
            10.0
        );
        assert_eq!(scroll_view(0.0, content, height, Motion::PageDown), 18.0);
        assert_eq!(scroll_view(25.0, content, height, Motion::PageDown), 30.5);
        assert_eq!(scroll_view(30.5, content, height, Motion::LineDown), 30.5);
        assert_eq!(scroll_view(3.0, content, height, Motion::HalfPageUp), 0.0);
        assert_eq!(scroll_view(12.0, content, height, Motion::Top), 0.0);
        assert_eq!(scroll_view(12.0, content, height, Motion::Bottom), 30.5);
        // Content shorter than the view does not scroll.
        assert_eq!(scroll_view(0.0, 5.0, height, Motion::Bottom), 0.0);
        assert_eq!(scroll_view(4.0, 5.0, height, Motion::LineDown), 0.0);
    }

    #[test]
    fn motions_of_actions() {
        assert_eq!(
            Motion::from_action(&Action::HalfPageDown),
            Some(Motion::HalfPageDown)
        );
        assert_eq!(Motion::from_action(&Action::CursorTop), Some(Motion::Top));
        assert_eq!(Motion::from_action(&Action::Open), None);
        assert!(!Motion::LineDown.animates());
        assert!(!Motion::LineUp.animates());
        assert!(Motion::PageUp.animates());
        assert!(Motion::Bottom.animates());
    }
}
