//! Developer tools: timing traces, `--debug-keys`, and `--debug-screenshot`.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use bf_core::KeyEvent;
use bf_core::keymap::{Key, KeyPress, NamedKey, Trigger};
use objc2::{msg_send, sel};
use objc2_app_kit::{NSApplication, NSBitmapImageFileType, NSTextField};
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

/// One step of a scripted run.
#[derive(Debug, Clone, Copy)]
pub enum Step {
    Key(KeyEvent),
    /// `@<ms>`: wait this time before the next step.
    Wait(Duration),
}

/// A scripted run: keys to feed, then an optional screenshot.
struct DebugRun {
    screenshot: Option<PathBuf>,
    keys: VecDeque<Step>,
    capture_scheduled: bool,
    step_scheduled: bool,
    done: bool,
}

thread_local! {
    static RUN: RefCell<Option<DebugRun>> = const { RefCell::new(None) };
}

/// Parses `--debug-keys` text: triggers in keymap syntax, separated by spaces.
/// A trigger with `>` gives one key event for each key press. `@<ms>` waits.
pub fn parse_keys(text: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    for token in text.split_whitespace() {
        if let Some(ms) = token.strip_prefix('@').filter(|ms| !ms.is_empty()) {
            let ms: u64 = ms
                .parse()
                .map_err(|_| format!("--debug-keys: {token}: bad wait time"))?;
            steps.push(Step::Wait(Duration::from_millis(ms)));
            continue;
        }
        let trigger: Trigger = token
            .parse()
            .map_err(|e| format!("--debug-keys: {token}: {e}"))?;
        steps.extend(
            trigger
                .0
                .into_iter()
                .map(|press| Step::Key(key_event(press))),
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
            done: false,
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
    schedule_step();
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
    if app::with_ref(|s| s.pane.is_loading()).unwrap_or(true) {
        return;
    }
    let next = RUN.with(|run| {
        run.try_borrow_mut()
            .ok()
            .and_then(|mut r| r.as_mut().and_then(|d| d.keys.pop_front()))
    });
    match next {
        Some(Step::Key(event)) => {
            trace!("debug-keys: {:?} {:?}", event.base, event.mods);
            match app::prompt_field() {
                Some(field) => prompt_key(&field, &event),
                None => {
                    app::handle_key(event);
                }
            }
            schedule_step();
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
    let Some(path) = RUN.with(|run| {
        let mut guard = run.try_borrow_mut().ok()?;
        let d = guard.as_mut()?;
        if d.done {
            return None;
        }
        d.done = true;
        d.screenshot.clone()
    }) else {
        return;
    };
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let result = render_png(mtm).and_then(|bytes| {
        std::fs::write(&path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
    });
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
