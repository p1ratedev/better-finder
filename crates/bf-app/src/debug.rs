//! Developer tools: timing traces, `--debug-keys`, and `--debug-screenshot`.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use bf_core::KeyEvent;
use bf_core::keymap::{Key, KeyPress, NamedKey, Trigger};
use objc2::{AnyThread, msg_send, sel};
use objc2_app_kit::{NSApplication, NSBitmapImageFileType, NSBitmapImageRep, NSTextField};
use objc2_foundation::{MainThreadMarker, NSDictionary, NSString};

use crate::app;

static START: OnceLock<Instant> = OnceLock::new();
static TRACE: OnceLock<bool> = OnceLock::new();

/// Records the start time of the process. Call it first in `main`.
pub fn mark_start() {
    let _ = START.set(Instant::now());
}

/// The time since `mark_start`.
pub fn since_start() -> Duration {
    START.get().map(Instant::elapsed).unwrap_or_default()
}

/// True when `BETTER_FINDER_TRACE` is set to a value other than `0`.
pub fn trace_on() -> bool {
    *TRACE.get_or_init(|| {
        std::env::var_os("BETTER_FINDER_TRACE").is_some_and(|v| !v.is_empty() && v != "0")
    })
}

/// Prints to stderr when `BETTER_FINDER_TRACE=1`.
macro_rules! trace {
    ($($arg:tt)*) => {
        if $crate::debug::trace_on() {
            eprintln!($($arg)*);
        }
    };
}
pub(crate) use trace;

/// How a scripted key reaches the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    /// The normal path: the prompt field when the prompt is open, else the
    /// keymap.
    Normal,
    /// `~key`: the keymap, as an automatic key repeat.
    Repeat,
    /// `!key`: the keymap, also when the prompt is open (like a key that
    /// the Quick Look panel sends to its delegate).
    Keymap,
    /// `^key`: the normal path, in the same main-queue turn as the previous
    /// key. It does not wait for a load in progress.
    NoWait,
}

/// One step of a scripted run.
#[derive(Debug, Clone)]
pub enum Step {
    Key(KeyEvent, Feed),
    /// `@<ms>`: wait this time before the next step.
    Wait(Duration),
    /// `@load`: wait until the next directory load completes.
    WaitLoad,
    /// `@w<points>` or `@w<points>x<points>`: set the width (and the
    /// height) of the window content.
    Width(f64, Option<f64>),
    /// `@eject:<name>`: click the eject button of the sidebar row `name`.
    ClickEject(String),
    /// `@menu:<row>` or `@menu:bg`: build the context menu of a row (the
    /// cursor moves to it) or of the folder, and print its items to stdout.
    Menu(Option<usize>),
}

/// A scripted run: keys to feed, then an optional screenshot.
struct DebugRun {
    screenshot: Option<PathBuf>,
    keys: VecDeque<Step>,
    capture_scheduled: bool,
    step_scheduled: bool,
    /// A `@load` step waits.
    waiting_for_load: bool,
    done: bool,
    /// The most recent `@w` size. A tiling window manager can change the
    /// size again, so the capture sets it again just before the image.
    size: Option<(f64, Option<f64>)>,
}

thread_local! {
    static RUN: RefCell<Option<DebugRun>> = const { RefCell::new(None) };
}

/// Parses `--debug-keys` text: triggers in keymap syntax, separated by spaces.
/// A trigger with `>` gives one key event for each key press. `@<ms>` waits,
/// `@load` waits for a load. The prefixes `~`, `!`, `^` select a `Feed`.
pub fn parse_keys(text: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    for token in text.split_whitespace() {
        if let Some(name) = token.strip_prefix("@eject:") {
            steps.push(Step::ClickEject(name.to_string()));
            continue;
        }
        if let Some(target) = token.strip_prefix("@menu:") {
            let row = match target {
                "bg" => None,
                row => Some(
                    row.parse()
                        .map_err(|_| format!("--debug-keys: {token}: use a row number or bg"))?,
                ),
            };
            steps.push(Step::Menu(row));
            continue;
        }
        if token == "@load" {
            steps.push(Step::WaitLoad);
            continue;
        }
        if let Some(size) = token.strip_prefix("@w") {
            let bad = || format!("--debug-keys: {token}: bad size");
            let (width, height) = match size.split_once('x') {
                Some((w, h)) => (w, Some(h.parse::<f64>().map_err(|_| bad())?)),
                None => (size, None),
            };
            let width: f64 = width.parse().map_err(|_| bad())?;
            steps.push(Step::Width(width, height));
            continue;
        }
        if let Some(ms) = token.strip_prefix('@').filter(|ms| !ms.is_empty()) {
            let ms: u64 = ms
                .parse()
                .map_err(|_| format!("--debug-keys: {token}: bad wait time"))?;
            steps.push(Step::Wait(Duration::from_millis(ms)));
            continue;
        }
        let (feed, text) = match token.split_at_checked(1) {
            Some(("~", rest)) if !rest.is_empty() => (Feed::Repeat, rest),
            Some(("!", rest)) if !rest.is_empty() => (Feed::Keymap, rest),
            Some(("^", rest)) if !rest.is_empty() => (Feed::NoWait, rest),
            _ => (Feed::Normal, token),
        };
        let trigger: Trigger = text
            .parse()
            .map_err(|e| format!("--debug-keys: {token}: {e}"))?;
        steps.extend(
            trigger
                .0
                .into_iter()
                .map(|press| Step::Key(key_event(press), feed)),
        );
    }
    Ok(steps)
}

/// The key event that the UI reports for a normalized key press.
fn key_event(press: KeyPress) -> KeyEvent {
    let mods = press.mods;
    match press.key {
        Key::Char(c) if c.is_uppercase() => {
            let base = c.to_lowercase().next().unwrap_or(c);
            KeyEvent {
                base: Key::Char(base),
                shifted: Some(c),
                mods: bf_core::keymap::Mods {
                    shift: true,
                    ..mods
                },
            }
        }
        Key::Char(c) => KeyEvent {
            base: Key::Char(c),
            shifted: mods.shift.then_some(c),
            mods,
        },
        Key::Named(_) => KeyEvent {
            base: press.key,
            shifted: None,
            mods,
        },
    }
}

/// Starts a scripted run. Nothing happens when both arguments are empty.
pub fn init(screenshot: Option<PathBuf>, keys: Vec<Step>) {
    if screenshot.is_none() && keys.is_empty() {
        return;
    }
    let has_screenshot = screenshot.is_some();
    RUN.with(|run| {
        *run.borrow_mut() = Some(DebugRun {
            screenshot,
            keys: keys.into(),
            capture_scheduled: false,
            step_scheduled: false,
            waiting_for_load: false,
            done: false,
            size: None,
        });
    });
    if has_screenshot {
        // A safety net: take the screenshot after 15 s, also when a load hangs.
        app::after(Duration::from_secs(15), || {
            eprintln!("better-finder: debug run timed out; the screenshot shows the current state");
            capture();
        });
    }
}

/// The app calls this after each finished directory load.
pub fn on_load_finished() {
    RUN.with(|run| {
        if let Ok(mut guard) = run.try_borrow_mut() {
            if let Some(d) = guard.as_mut() {
                d.waiting_for_load = false;
            }
        }
    });
    schedule_step();
}

fn pop_step(only_no_wait: bool) -> Option<Step> {
    RUN.with(|run| {
        let mut guard = run.try_borrow_mut().ok()?;
        let d = guard.as_mut()?;
        if only_no_wait && !matches!(d.keys.front(), Some(Step::Key(_, Feed::NoWait))) {
            return None;
        }
        d.keys.pop_front()
    })
}

fn feed_key(event: KeyEvent, feed: Feed) {
    trace!("debug-keys: {:?} {:?} {:?}", event.base, event.mods, feed);
    match (feed, app::prompt_field()) {
        (Feed::Repeat, _) => {
            app::handle_key(event, true);
        }
        (Feed::Keymap, _) => {
            app::handle_key(event, false);
        }
        (Feed::Normal | Feed::NoWait, None) => {
            // Like the real key path: the sidebar gets the key first when
            // it has the focus.
            if !(app::sidebar_focused() && crate::sidebar::handle_key(&event)) {
                app::handle_key(event, false);
            }
        }
        (Feed::Normal | Feed::NoWait, Some(field)) => prompt_key(&field, &event),
    }
}

/// Runs `step` on the main queue, once. Each key is one main-queue turn,
/// like real key events, so deferred work (prompt accept) runs in between.
fn schedule_step() {
    let schedule = RUN.with(|run| {
        let Ok(mut guard) = run.try_borrow_mut() else {
            return false;
        };
        let Some(d) = guard.as_mut() else {
            return false;
        };
        !std::mem::replace(&mut d.step_scheduled, true)
    });
    if schedule {
        // A short gap, like real typing. Background work (a Tab completion)
        // can complete before the next key.
        app::after(Duration::from_millis(15), step);
    }
}

/// Feeds the next key. It waits while a directory load is in progress:
/// `on_load_finished` continues the run.
fn step() {
    RUN.with(|run| {
        if let Ok(mut guard) = run.try_borrow_mut() {
            if let Some(d) = guard.as_mut() {
                d.step_scheduled = false;
            }
        }
    });
    let waiting = RUN.with(|run| {
        run.try_borrow()
            .ok()
            .and_then(|r| r.as_ref().map(|d| d.waiting_for_load))
            .unwrap_or(false)
    });
    if waiting || app::with_ref(|s| s.pane.is_loading()).unwrap_or(true) {
        return;
    }
    match pop_step(false) {
        Some(Step::Key(event, feed)) => {
            feed_key(event, feed);
            // `^` keys follow in the same turn, before a load can complete.
            while let Some(Step::Key(event, feed)) = pop_step(true) {
                feed_key(event, feed);
            }
            schedule_step();
            return;
        }
        Some(Step::Width(width, height)) => {
            RUN.with(|run| {
                if let Ok(mut guard) = run.try_borrow_mut() {
                    if let Some(d) = guard.as_mut() {
                        d.size = Some((width, height));
                    }
                }
            });
            if let Some(window) = app::with_ref(|s| s.ui.window.clone()) {
                set_content_size(&window, width, height);
                trace!(
                    "debug-keys: window {:?}, screen {:?}",
                    window.frame(),
                    window.screen().map(|s| s.visibleFrame())
                );
            }
            schedule_step();
            return;
        }
        Some(Step::ClickEject(name)) => {
            if !crate::sidebar::click_eject_button(&name) {
                eprintln!("better-finder: --debug-keys: no eject button for {name}");
            }
            schedule_step();
            return;
        }
        Some(Step::Menu(row)) => {
            crate::context_menu::debug_print(row);
            schedule_step();
            return;
        }
        Some(Step::WaitLoad) => {
            RUN.with(|run| {
                if let Ok(mut guard) = run.try_borrow_mut() {
                    if let Some(d) = guard.as_mut() {
                        d.waiting_for_load = true;
                    }
                }
            });
            return;
        }
        Some(Step::Wait(time)) => {
            RUN.with(|run| {
                if let Ok(mut guard) = run.try_borrow_mut() {
                    if let Some(d) = guard.as_mut() {
                        d.step_scheduled = true;
                    }
                }
            });
            app::after(time, step);
            return;
        }
        None => {}
    }
    let capture_now = RUN.with(|run| {
        let Ok(mut guard) = run.try_borrow_mut() else {
            return false;
        };
        let Some(d) = guard.as_mut() else {
            return false;
        };
        if d.screenshot.is_none() || d.capture_scheduled {
            return false;
        }
        d.capture_scheduled = true;
        true
    });
    if capture_now {
        // Wait a short time so that layout and the next frame complete.
        app::after(Duration::from_millis(250), capture);
    }
}

/// Sets the content size of `window`. `None` keeps the height.
fn set_content_size(window: &objc2_app_kit::NSWindow, width: f64, height: Option<f64>) {
    let height =
        height.unwrap_or_else(|| window.contentRectForFrameRect(window.frame()).size.height);
    window.setContentSize(objc2_foundation::NSSize::new(width, height));
}

/// Sends a key to the prompt field through its field editor, like the
/// keyboard does: text goes to `insertText:`, named keys become commands
/// (`insertNewline:`, `cancelOperation:`, ...), which reach the prompt
/// delegate through `control:textView:doCommandBySelector:`.
fn prompt_key(field: &NSTextField, event: &KeyEvent) {
    let Some(editor) = field.currentEditor() else {
        return;
    };
    let command = match event.base {
        Key::Named(NamedKey::Enter) => Some(sel!(insertNewline:)),
        Key::Named(NamedKey::Escape) => Some(sel!(cancelOperation:)),
        Key::Named(NamedKey::Tab) => Some(sel!(insertTab:)),
        Key::Named(NamedKey::Backspace) => Some(sel!(deleteBackward:)),
        Key::Named(NamedKey::Up) => Some(sel!(moveUp:)),
        Key::Named(NamedKey::Down) => Some(sel!(moveDown:)),
        _ => None,
    };
    if let Some(command) = command {
        // SAFETY: `doCommandBySelector:` takes a selector and returns void.
        let _: () = unsafe { msg_send![&*editor, doCommandBySelector: command] };
        return;
    }
    let text = match event.base {
        Key::Named(NamedKey::Space) => ' ',
        Key::Char(c) => event.shifted.unwrap_or(c),
        Key::Named(_) => return,
    };
    let text = NSString::from_str(&text.to_string());
    // SAFETY: `insertText:` takes a string and returns void.
    let _: () = unsafe { msg_send![&*editor, insertText: &*text] };
}

/// Renders the window to the PNG file and quits.
fn capture() {
    let Some((path, size)) = RUN.with(|run| {
        let mut guard = run.try_borrow_mut().ok()?;
        let d = guard.as_mut()?;
        if d.done {
            return None;
        }
        d.done = true;
        d.screenshot.clone().map(|p| (p, d.size))
    }) else {
        return;
    };
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    // `BETTER_FINDER_SCREENCAPTURE=1` captures the real window with
    // `screencapture` (glass and layer colors render). It needs the screen
    // recording permission of the terminal. `BETTER_FINDER_SCREENCAPTURE=own`
    // asks the window server for the image of the window. An app can get
    // the image of its own window without a permission. The default renders
    // the views offscreen, without a permission (no glass).
    let mode = std::env::var("BETTER_FINDER_SCREENCAPTURE").ok();
    if let (Some((width, height)), Some(window)) = (size, app::with_ref(|s| s.ui.window.clone())) {
        set_content_size(&window, width, height);
    }
    let result = if mode.as_deref() == Some("own") {
        own_window_png().and_then(|bytes| {
            std::fs::write(&path, bytes)
                .map_err(|e| format!("cannot write {}: {e}", path.display()))
        })
    } else if mode.is_some() {
        screen_capture(&path)
    } else {
        render_png(mtm).and_then(|bytes| {
            std::fs::write(&path, bytes)
                .map_err(|e| format!("cannot write {}: {e}", path.display()))
        })
    };
    match result {
        Ok(()) => {
            eprintln!("screenshot: {}", path.display());
            NSApplication::sharedApplication(mtm).terminate(None);
        }
        Err(e) => {
            eprintln!("better-finder: screenshot failed: {e}");
            std::process::exit(1);
        }
    }
}

/// Captures the window on the screen with `/usr/sbin/screencapture`.
fn screen_capture(path: &std::path::Path) -> Result<(), String> {
    let window = app::with_ref(|s| s.ui.window.clone()).ok_or("no app state")?;
    window.displayIfNeeded();
    // SAFETY: `+[CATransaction flush]` takes no arguments and returns void.
    unsafe {
        let _: () = objc2::msg_send![objc2::class!(CATransaction), flush];
    }
    let id = window.windowNumber();
    let status = std::process::Command::new("/usr/sbin/screencapture")
        .arg("-x")
        .arg("-o")
        .arg(format!("-l{id}"))
        .arg(path)
        .status()
        .map_err(|e| format!("cannot run screencapture: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("screencapture failed: {status}"))
    }
}

/// `CGRect`.
#[repr(C)]
#[derive(Clone, Copy)]
struct CgRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// A `CGImageRef`.
#[repr(transparent)]
struct CgImage(*mut std::ffi::c_void);

// SAFETY: A `CGImageRef` is a pointer to the opaque struct `CGImage`.
unsafe impl objc2::encode::Encode for CgImage {
    const ENCODING: objc2::encode::Encoding =
        objc2::encode::Encoding::Pointer(&objc2::encode::Encoding::Struct("CGImage", &[]));
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    /// The SDK marks this function as deprecated, but it still works. It
    /// gives the image of a window of this app without the screen
    /// recording permission.
    fn CGWindowListCreateImage(
        bounds: CgRect,
        options: u32,
        window: u32,
        image_options: u32,
    ) -> *mut std::ffi::c_void;
    fn CGImageRelease(image: *mut std::ffi::c_void);
}

/// The image of the window from the window server, as a PNG: glass and
/// layer colors render. No shadow.
fn own_window_png() -> Result<Vec<u8>, String> {
    const INCLUDING_WINDOW: u32 = 1 << 3;
    const BOUNDS_IGNORE_FRAMING: u32 = 1 << 0;
    let window = app::with_ref(|s| s.ui.window.clone()).ok_or("no app state")?;
    window.displayIfNeeded();
    // SAFETY: `+[CATransaction flush]` takes no arguments and returns void.
    unsafe {
        let _: () = objc2::msg_send![objc2::class!(CATransaction), flush];
    }
    let id = u32::try_from(window.windowNumber()).map_err(|_| "bad window number")?;
    // `CGRectNull`: the bounds of the window.
    let null = CgRect {
        x: f64::INFINITY,
        y: f64::INFINITY,
        width: 0.0,
        height: 0.0,
    };
    // SAFETY: The arguments are valid. The caller owns the image.
    let image =
        unsafe { CGWindowListCreateImage(null, INCLUDING_WINDOW, id, BOUNDS_IGNORE_FRAMING) };
    if image.is_null() {
        return Err("the window server gave no image".to_string());
    }
    // SAFETY: `initWithCGImage:` takes a valid `CGImageRef`. The bitmap
    // retains the image, so the release below is correct.
    let rep: Option<objc2::rc::Retained<NSBitmapImageRep>> =
        unsafe { msg_send![NSBitmapImageRep::alloc(), initWithCGImage: CgImage(image)] };
    // SAFETY: `image` comes from a create function and is released once.
    unsafe { CGImageRelease(image) };
    let rep = rep.ok_or("no bitmap")?;
    let props = NSDictionary::new();
    // SAFETY: An empty property dictionary is valid for the PNG type.
    let data =
        unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }
            .ok_or("PNG encoding failed")?;
    Ok(data.to_vec())
}

/// Draws the window frame view (title bar included) into a PNG.
fn render_png(_mtm: MainThreadMarker) -> Result<Vec<u8>, String> {
    let window = app::with_ref(|s| s.ui.window.clone()).ok_or("no app state")?;
    let content = window.contentView().ok_or("no content view")?;
    // The frame view draws the window background and the title bar.
    // SAFETY: The window retains its frame view.
    let view = unsafe { content.superview() }.unwrap_or(content);
    view.displayIfNeeded();
    let rect = view.bounds();
    let rep = view
        .bitmapImageRepForCachingDisplayInRect(rect)
        .ok_or("no bitmap")?;
    view.cacheDisplayInRect_toBitmapImageRep(rect, &rep);
    let props = NSDictionary::new();
    // SAFETY: An empty property dictionary is valid for the PNG type.
    let data =
        unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }
            .ok_or("PNG encoding failed")?;
    Ok(data.to_vec())
}
