//! Smooth scroll: one animation driver for the list and the help overlay.
//!
//! - A display link (`-[NSView displayLinkWithTarget:selector:]`) calls
//!   `tick` once for each frame of the screen. `prepare` makes it after the
//!   first frame, so that the first animation does not wait for it. When
//!   the display link is not available, a 120 Hz timer calls `tick`.
//! - Each frame moves the scroll position of the clip view, and the y
//!   position of the cursor highlight of the list, from the start values
//!   to the targets. The curve is a cubic ease-out.
//! - The highlight moves in its own small view (`ListView::show_highlight`).
//!   So a frame redraws only that view and the rows that the scroll shows.
//!   The list draws no cursor row while a list animation runs.
//! - A jump of more than one screen (for example `G`) animates only the
//!   last screen of the distance. A frame then never redraws the full list.
//! - A new scroll during an animation starts from the current position (a
//!   retarget). `finish` puts the view at the target at once.
//! - The state is in its own `thread_local`, apart from `AppState`. The rule
//!   of `app` applies here too: borrow, compute, drop, then call AppKit.
//! - `BETTER_FINDER_TRACE=1` prints one line for each animation: the
//!   frames, the frame cost (tick and draws), and the start and target values.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{NSScrollView, NSView, NSWorkspace};
use objc2_foundation::{
    MainThreadMarker, NSObject, NSObjectProtocol, NSRunLoop, NSRunLoopCommonModes,
};

use crate::app;
use crate::debug::{self, trace};
use crate::list_view::ListView;

/// The view that an animation scrolls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    List,
    Help,
}

/// A scroll of the list: the target of the clip view and of the cursor.
#[derive(Clone, Debug)]
pub struct ListScroll {
    /// The target y of the clip view origin.
    pub to_y: f64,
    /// The cursor row before the action. The highlight starts there when
    /// no animation runs.
    pub from_row: usize,
    pub to_row: usize,
    pub row_height: f64,
    /// The height that the toolbar and the column header cover (for the
    /// trace: it converts y to a row).
    pub inset: f64,
    /// The action, for the trace.
    pub label: String,
}

struct Animation {
    target: Target,
    label: String,
    scroll: Retained<NSScrollView>,
    /// The list, for the highlight. `None` for the help overlay.
    list: Option<Retained<ListView>>,
    row_height: f64,
    inset: f64,
    /// The backing scale factor: the values round to whole pixels.
    scale: f64,
    from_y: f64,
    to_y: f64,
    from_hl: f64,
    to_hl: f64,
    /// The highlight y of the most recent frame.
    hl: f64,
    started: Instant,
    duration: f64,
    /// The target frame is on the screen. The next tick stops the driver.
    done: bool,
    /// The jump was longer than one screen: the animation starts one screen
    /// before the target.
    jumped: bool,
    stats: Stats,
}

/// The cost of the frames of one animation, and the values for the trace.
#[derive(Default)]
struct Stats {
    frames: u32,
    max: Duration,
    total: Duration,
    /// The work of the most recent tick. The draw time adds to it.
    tick: Option<Duration>,
    /// The draw time since the most recent tick.
    draw: Duration,
    first_y: f64,
    first_row: usize,
    to_row: usize,
}

impl Stats {
    /// Adds the frame of the most recent tick (its work and its draws).
    /// Before the first tick, the draws belong to the turn of the key press
    /// (for example the jump of `G`): they are a frame of their own.
    fn close_frame(&mut self) {
        let (tick, draw) = match self.tick.take() {
            Some(tick) => (tick, std::mem::take(&mut self.draw)),
            None if !self.draw.is_zero() => (Duration::ZERO, std::mem::take(&mut self.draw)),
            None => return,
        };
        let cost = tick + draw;
        if cost > Duration::from_micros(1500) {
            trace!(
                "scroll: slow frame {}: tick {} µs, draws {} µs",
                self.frames,
                tick.as_micros(),
                draw.as_micros()
            );
        }
        self.frames += 1;
        self.max = self.max.max(cost);
        self.total += cost;
    }
}

thread_local! {
    static ANIM: RefCell<Option<Animation>> = const { RefCell::new(None) };
    /// The display link (a `CADisplayLink`). It is paused when no animation runs.
    static LINK: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    /// The 120 Hz fallback timer runs.
    static TIMER_ON: Cell<bool> = const { Cell::new(false) };
}

define_class!(
    // SAFETY:
    // - NSObject has no subclass requirements.
    // - Driver does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BFScrollDriver"]
    struct Driver;

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for Driver {}

    impl Driver {
        #[unsafe(method(tick:))]
        fn tick_link(&self, _link: &AnyObject) {
            tick();
        }
    }
);

impl Driver {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: `init` is the designated initializer of NSObject.
        unsafe { msg_send![super(this), init] }
    }
}

/// The cubic ease-out curve.
fn ease_out(t: f64) -> f64 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

fn round_px(value: f64, scale: f64) -> f64 {
    (value * scale).round() / scale
}

/// The start of a jump of more than `span` points: `span` before the
/// target. A shorter jump starts at `from`.
fn capped_start(from: f64, to: f64, span: f64) -> f64 {
    let span = span.max(0.0);
    if (to - from).abs() <= span {
        from
    } else if to > from {
        to - span
    } else {
        to + span
    }
}

/// True when the macOS setting Reduce Motion is on.
fn reduce_motion() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

/// The current y of the clip view origin.
fn clip_y(scroll: &NSScrollView) -> f64 {
    scroll.contentView().bounds().origin.y
}

fn set_clip_y(scroll: &NSScrollView, y: f64) {
    let clip = scroll.contentView();
    let origin = clip.bounds().origin;
    if (origin.y - y).abs() > 0.001 {
        clip.scrollToPoint(objc2_foundation::NSPoint::new(origin.x, y));
        scroll.reflectScrolledClipView(&clip);
    }
}

/// True while an animation runs.
pub fn is_running() -> bool {
    ANIM.with(|a| a.try_borrow().map(|a| a.is_some()).unwrap_or(false))
}

/// The target y of the clip view of the animation of `target`, while it runs.
pub fn target_y(target: Target) -> Option<f64> {
    ANIM.with(|a| {
        let a = a.try_borrow().ok()?;
        a.as_ref().filter(|a| a.target == target).map(|a| a.to_y)
    })
}

/// The y of the cursor highlight in the list (document coordinates), while
/// a list animation runs. The list then draws no cursor row: the highlight
/// view shows the cursor.
pub fn list_highlight() -> Option<f64> {
    ANIM.with(|a| {
        let a = a.try_borrow().ok()?;
        a.as_ref()
            .filter(|a| a.target == Target::List)
            .map(|a| a.hl)
    })
}

/// A view drew during an animation: the time adds to the current frame.
pub fn note_draw(cost: Duration) {
    ANIM.with(|a| {
        if let Ok(mut a) = a.try_borrow_mut() {
            if let Some(a) = a.as_mut() {
                a.stats.draw += cost;
            }
        }
    });
}

/// Makes the display link (paused), so that the first animation does not
/// wait for it. The app calls this after the first frame.
pub fn prepare(view: &NSView) {
    if LINK.with(|l| l.borrow().is_none()) {
        make_link(view);
    }
}

/// Scrolls the list to `plan`. With a `duration` (and Reduce Motion off),
/// the view and the highlight animate; else they go to the target at once.
pub fn scroll_list(
    list: &ListView,
    scroll: &NSScrollView,
    plan: ListScroll,
    duration: Option<Duration>,
) {
    let h = plan.row_height;
    let to_hl = plan.to_row as f64 * h;
    let duration = duration.filter(|_| !reduce_motion());
    if target_y(Target::Help).is_some() {
        finish();
    }
    // A retarget starts from the current animated position.
    let old = take_running();
    if let Some(old) = &old {
        log(old, "retarget");
    }
    let from_y = clip_y(scroll);
    let from_hl = old.as_ref().map_or(plan.from_row as f64 * h, |a| a.hl);
    let moves = (from_y - plan.to_y).abs() >= 0.01 || (from_hl - to_hl).abs() >= 0.01;
    let Some(duration) = duration.filter(|_| moves) else {
        if moves {
            trace!(
                "scroll: list {} instant, offset {:.0} -> {:.0}, top row {:.1} -> {:.1}, cursor row {} -> {}",
                plan.label,
                from_y,
                plan.to_y,
                (from_y + plan.inset) / h,
                (plan.to_y + plan.inset) / h,
                plan.from_row,
                plan.to_row
            );
        }
        set_clip_y(scroll, plan.to_y);
        list.show_highlight(None);
        list.redraw_band(from_hl, h);
        list.redraw_band(to_hl, h);
        if old.is_some() {
            stop_driver();
        }
        return;
    };
    if old.is_none() {
        // The list drew the cursor row with the highlight: draw it again
        // without it. The highlight view takes over.
        list.redraw_band(from_hl, h);
    }
    // A jump of more than one screen animates only the last screen. The
    // highlight keeps its place on the screen at the jump.
    let span = scroll.contentView().bounds().size.height;
    let start_y = capped_start(from_y, plan.to_y, span);
    let start_hl = from_hl + (start_y - from_y);
    let scale = list.window().map(|w| w.backingScaleFactor()).unwrap_or(2.0);
    let (first_y, first_row) = old.as_ref().map_or((from_y, plan.from_row), |a| {
        (a.stats.first_y, a.stats.first_row)
    });
    let anim = Animation {
        target: Target::List,
        label: plan.label,
        scroll: scroll.retain(),
        list: Some(list.retain()),
        row_height: h,
        inset: plan.inset,
        scale,
        from_y: start_y,
        to_y: plan.to_y,
        from_hl: start_hl,
        to_hl,
        hl: start_hl,
        started: Instant::now(),
        duration: duration.as_secs_f64(),
        done: false,
        jumped: start_y != from_y,
        stats: Stats {
            first_y,
            first_row,
            to_row: plan.to_row,
            ..Stats::default()
        },
    };
    set_clip_y(scroll, start_y);
    list.show_highlight(Some(start_hl));
    start(anim, list);
}

/// Scrolls the help overlay to `to_y`, animated with a `duration` (and
/// Reduce Motion off), else at once.
pub fn scroll_help(scroll: &NSScrollView, to_y: f64, duration: Option<Duration>, label: &str) {
    let duration = duration.filter(|_| !reduce_motion());
    if target_y(Target::List).is_some() {
        finish();
    }
    let old = take_running();
    if let Some(old) = &old {
        log(old, "retarget");
    }
    let from_y = clip_y(scroll);
    let moves = (from_y - to_y).abs() >= 0.01;
    let view = scroll.documentView();
    let (Some(duration), Some(view)) = (duration.filter(|_| moves), view) else {
        if moves {
            trace!("scroll: help {label} instant, offset {from_y:.0} -> {to_y:.0}");
        }
        set_clip_y(scroll, to_y);
        if old.is_some() {
            stop_driver();
        }
        return;
    };
    let span = scroll.contentView().bounds().size.height;
    let start_y = capped_start(from_y, to_y, span);
    let scale = view.window().map(|w| w.backingScaleFactor()).unwrap_or(2.0);
    let anim = Animation {
        target: Target::Help,
        label: label.to_string(),
        scroll: scroll.retain(),
        list: None,
        row_height: 0.0,
        inset: 0.0,
        scale,
        from_y: start_y,
        to_y,
        from_hl: 0.0,
        to_hl: 0.0,
        hl: 0.0,
        started: Instant::now(),
        duration: duration.as_secs_f64(),
        done: false,
        jumped: start_y != from_y,
        stats: Stats {
            first_y: old.as_ref().map_or(from_y, |a| a.stats.first_y),
            ..Stats::default()
        },
    };
    set_clip_y(scroll, start_y);
    start(anim, &view);
}

/// Ends the animation at its target at once. Nothing happens when no
/// animation runs.
pub fn finish() {
    let Some(anim) = take_running() else {
        return;
    };
    set_clip_y(&anim.scroll, anim.to_y);
    end_highlight(&anim);
    log(&anim, "ended at once");
    stop_driver();
}

/// The list animation ended: the highlight view hides, and the list draws
/// the cursor row with the highlight again.
fn end_highlight(anim: &Animation) {
    if let Some(list) = &anim.list {
        list.show_highlight(None);
        list.redraw_band(anim.hl, anim.row_height);
        list.redraw_band(anim.to_hl, anim.row_height);
    }
}

/// Removes the running animation from the state.
fn take_running() -> Option<Animation> {
    let mut anim = ANIM.with(|a| a.try_borrow_mut().ok()?.take())?;
    anim.stats.close_frame();
    Some(anim)
}

fn start(anim: Animation, view: &NSView) {
    ANIM.with(|a| {
        if let Ok(mut a) = a.try_borrow_mut() {
            *a = Some(anim);
        }
    });
    start_driver(view);
}

/// One frame.
fn tick() {
    let started = Instant::now();
    enum Step {
        Idle,
        Stop(Box<Animation>),
        Frame(FrameViews, f64, f64),
    }
    let step = ANIM.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return Step::Idle;
        };
        let Some(a) = guard.as_mut() else {
            return Step::Idle;
        };
        a.stats.close_frame();
        if a.done {
            return guard.take().map_or(Step::Idle, |a| Step::Stop(Box::new(a)));
        }
        let t = if a.duration > 0.0 {
            a.started.elapsed().as_secs_f64() / a.duration
        } else {
            1.0
        };
        let (y, hl) = if t >= 1.0 {
            a.done = true;
            (a.to_y, a.to_hl)
        } else {
            let e = ease_out(t);
            (
                round_px(a.from_y + (a.to_y - a.from_y) * e, a.scale),
                round_px(a.from_hl + (a.to_hl - a.from_hl) * e, a.scale),
            )
        };
        a.hl = hl;
        Step::Frame(
            FrameViews {
                scroll: a.scroll.clone(),
                list: a.list.clone(),
            },
            y,
            hl,
        )
    });
    match step {
        Step::Idle => stop_driver(),
        Step::Stop(anim) => {
            end_highlight(&anim);
            log(&anim, "done");
            stop_driver();
        }
        Step::Frame(views, y, hl) => {
            // Out of the state borrow: AppKit can call back.
            set_clip_y(&views.scroll, y);
            if let Some(list) = &views.list {
                list.show_highlight(Some(hl));
            }
            let cost = started.elapsed();
            ANIM.with(|a| {
                if let Ok(mut a) = a.try_borrow_mut() {
                    if let Some(a) = a.as_mut() {
                        a.stats.tick = Some(cost);
                    }
                }
            });
        }
    }
}

/// The views of one frame, out of the state borrow.
struct FrameViews {
    scroll: Retained<NSScrollView>,
    list: Option<Retained<ListView>>,
}

fn log(anim: &Animation, how: &str) {
    if !debug::trace_on() {
        return;
    }
    let s = &anim.stats;
    let avg = if s.frames > 0 {
        s.total.as_secs_f64() * 1000.0 / f64::from(s.frames)
    } else {
        0.0
    };
    let elapsed = anim.started.elapsed().as_secs_f64() * 1000.0;
    let (target, rows) = match anim.target {
        Target::List => {
            let h = anim.row_height.max(1.0);
            (
                "list",
                format!(
                    ", top row {:.1} -> {:.1}, cursor row {} -> {}",
                    (s.first_y + anim.inset) / h,
                    (anim.to_y + anim.inset) / h,
                    s.first_row,
                    s.to_row
                ),
            )
        }
        Target::Help => ("help", String::new()),
    };
    let jump = if anim.jumped {
        format!(" (jump to {:.0} first)", anim.from_y)
    } else {
        String::new()
    };
    trace!(
        "scroll: {target} {} {how}: {} frames in {elapsed:.0} ms, frame cost max {:.3} ms avg {avg:.3} ms, offset {:.0} -> {:.0}{jump}{rows}",
        anim.label,
        s.frames,
        s.max.as_secs_f64() * 1000.0,
        s.first_y,
        anim.to_y,
    );
}

/// Makes the display link and adds it to the main run loop, paused.
/// Returns false when the view has no display link (before macOS 14).
fn make_link(view: &NSView) -> bool {
    let started = Instant::now();
    let selector = sel!(displayLinkWithTarget:selector:);
    // SAFETY: `respondsToSelector:` takes a selector and returns a BOOL.
    let has_link: bool = unsafe { msg_send![view, respondsToSelector: selector] };
    if !has_link {
        return false;
    }
    let driver = Driver::new(view.mtm());
    // SAFETY: The driver has the `tick:` method, which takes the display
    // link. The display link keeps a strong reference to its target.
    let link: Option<Retained<AnyObject>> =
        unsafe { msg_send![view, displayLinkWithTarget: &*driver, selector: sel!(tick:)] };
    let Some(link) = link else {
        return false;
    };
    // SAFETY: `setPaused:` takes a BOOL. `addToRunLoop:forMode:` takes a run
    // loop and a mode string. Both return void. The common modes include
    // the event tracking mode.
    unsafe {
        let _: () = msg_send![&*link, setPaused: true];
        let _: () = msg_send![
            &*link,
            addToRunLoop: &*NSRunLoop::mainRunLoop(),
            forMode: NSRunLoopCommonModes
        ];
    }
    trace!(
        "scroll: display link ready in {} µs",
        started.elapsed().as_micros()
    );
    LINK.with(|l| *l.borrow_mut() = Some(link));
    true
}

/// Starts the display link, or the fallback timer.
fn start_driver(view: &NSView) {
    if LINK.with(|l| l.borrow().is_none()) && !make_link(view) {
        if !TIMER_ON.with(|t| t.replace(true)) {
            trace!("scroll: no display link, 120 Hz timer");
            app::after(Duration::from_micros(8_333), timer_tick);
        }
        return;
    }
    let link = LINK.with(|l| l.borrow().clone());
    if let Some(link) = link {
        // SAFETY: `setPaused:` takes a BOOL and returns void.
        let _: () = unsafe { msg_send![&*link, setPaused: false] };
    }
}

fn timer_tick() {
    tick();
    if is_running() {
        app::after(Duration::from_micros(8_333), timer_tick);
    } else {
        TIMER_ON.with(|t| t.set(false));
    }
}

fn stop_driver() {
    if is_running() {
        return;
    }
    let link = LINK.with(|l| l.borrow().clone());
    if let Some(link) = link {
        // SAFETY: `setPaused:` takes a BOOL and returns void.
        let _: () = unsafe { msg_send![&*link, setPaused: true] };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_out_curve() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert_eq!(ease_out(2.0), 1.0);
        assert_eq!(ease_out(-1.0), 0.0);
        assert!((ease_out(0.5) - 0.875).abs() < 1e-12);
        // Fast at the start, slow at the end.
        assert!(ease_out(0.1) > 0.1 && ease_out(0.9) < 1.0);
    }

    #[test]
    fn pixel_rounding() {
        assert_eq!(round_px(10.3, 2.0), 10.5);
        assert_eq!(round_px(10.2, 2.0), 10.0);
        assert_eq!(round_px(10.4, 1.0), 10.0);
    }

    #[test]
    fn long_jumps_start_one_screen_before_the_target() {
        assert_eq!(capped_start(100.0, 300.0, 400.0), 100.0);
        assert_eq!(capped_start(300.0, 100.0, 400.0), 300.0);
        assert_eq!(capped_start(0.0, 5000.0, 400.0), 4600.0);
        assert_eq!(capped_start(5000.0, -76.0, 400.0), 324.0);
        assert_eq!(capped_start(10.0, 20.0, -5.0), 20.0);
    }
}
