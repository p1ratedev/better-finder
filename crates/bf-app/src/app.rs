//! The app delegate, the main menu, the app state, and the dispatch of actions.
//!
//! # State and re-entrancy
//!
//! All app state is in one `AppState` in a main-thread `thread_local`.
//! `with` and `with_ref` borrow it, and they return `None` when the state is
//! borrowed already. AppKit can call our classes synchronously from many of
//! its methods (`QLPreviewPanel reloadData`, `makeKeyAndOrderFront:`,
//! `makeFirstResponder:`, frame changes). So the rule is: borrow, compute,
//! drop the borrow, then call AppKit. An `Effect` list and a `UiPlan` carry
//! the AppKit work out of the borrow.

use std::cell::RefCell;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use bf_core::config::{Diagnostic, ThemeMode, default_config_template};
use bf_core::fs::{display_path, expand_path, home_dir};
use bf_core::pane::PaneOptions;
use bf_core::{
    Action, Config, Entry, EntryKind, KeyEvent, KeyOutcome, KeymapState, LoadRequest, LoadResult,
    OpenOutcome, Pane,
};
use dispatch2::{DispatchQueue, DispatchTime};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{ClassType, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAppearanceCustomization, NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
    NSEventModifierFlags, NSMenu, NSMenuItem, NSPasteboard, NSPasteboardTypeString, NSResponder,
    NSWindow, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSFileManager, NSNotification, NSObject, NSObjectProtocol, NSString,
    NSURL,
};

use crate::debug::{self, trace};
use crate::help::HelpModel;
use crate::icons;
use crate::list_view::ListView;
use crate::loader;
use crate::prompt::PromptMode;
use crate::quicklook;
use crate::status_bar::StatusBar;
use crate::theme::{self, Style};
use crate::watcher;
use crate::window::{self, Ui};

/// The options of a normal app start, from the command line.
pub struct RunOptions {
    pub path: Option<String>,
    pub screenshot: Option<PathBuf>,
    pub keys: Vec<debug::Step>,
}

/// A status bar message. Info messages fade after 3 s. Errors stay until
/// the next action.
pub struct Message {
    pub text: String,
    pub error: bool,
    born: Instant,
    id: u64,
}

const FADE_START: f64 = 3.0;
const FADE_TIME: f64 = 0.4;

impl Message {
    /// 1.0 while the message is fresh, then down to 0.0 during the fade.
    pub fn alpha(&self) -> f64 {
        if self.error {
            return 1.0;
        }
        let age = self.born.elapsed().as_secs_f64();
        (1.0 - (age - FADE_START) / FADE_TIME).clamp(0.0, 1.0)
    }
}

/// What the views showed after the most recent `sync_ui`.
#[derive(Default)]
struct Rendered {
    valid: bool,
    cwd: PathBuf,
    cursor: usize,
    len: usize,
    current: Option<PathBuf>,
    row_height: f64,
}

/// All state of the app. Main thread only.
pub struct AppState {
    pub config: Config,
    pub style: Style,
    pub pane: Pane,
    pub keys: KeymapState,
    pub message: Option<Message>,
    /// True when a load takes more than 100 ms.
    pub loading_visible: bool,
    /// The help overlay, when it is visible.
    pub help: Option<HelpModel>,
    pub ui: Ui,
    config_files: Vec<PathBuf>,
    prompt: Option<PromptMode>,
    /// (directory, name): put the cursor on `name` after the next load of `directory`.
    pending_select: Option<(PathBuf, String)>,
    message_id: u64,
    load_generation: u64,
    load_started: Option<Instant>,
    /// The list content changed (entries, filter, sort, hidden, style).
    content_dirty: bool,
    /// A user action: scroll so that the cursor is visible, also when it
    /// did not move. An automatic refresh does not scroll.
    reveal_cursor: bool,
    rendered: Rendered,
    window_shown: bool,
    /// `NSApplication` keeps only a weak reference to its delegate.
    _delegate: Retained<AppDelegate>,
}

thread_local! {
    static STATE: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

/// Borrows the state mutably. `None` when another borrow is active.
pub fn with<R>(f: impl FnOnce(&mut AppState) -> R) -> Option<R> {
    STATE.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    })
}

/// Borrows the state for reading. `None` when a mutable borrow is active.
pub fn with_ref<R>(f: impl FnOnce(&AppState) -> R) -> Option<R> {
    STATE.with(|cell| {
        let guard = cell.try_borrow().ok()?;
        guard.as_ref().map(f)
    })
}

/// Runs `work` on the main queue after `delay`.
pub fn after(delay: Duration, work: impl FnOnce() + Send + 'static) {
    match DispatchTime::try_from(delay) {
        Ok(when) => {
            let _ = DispatchQueue::main().after(when, work);
        }
        Err(()) => DispatchQueue::main().exec_async(work),
    }
}

/// Runs `work` on the main queue, after the current event.
pub fn after_now(work: impl FnOnce() + Send + 'static) {
    DispatchQueue::main().exec_async(work);
}

// ---- App delegate and menu ----

define_class!(
    // SAFETY:
    // - NSObject has no subclass requirements.
    // - AppDelegate does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BFAppDelegate"]
    pub struct AppDelegate;

    // SAFETY: `NSObjectProtocol` has no safety requirements. The signatures match.
    unsafe impl NSObjectProtocol for AppDelegate {}

    // SAFETY: `NSApplicationDelegate` has no safety requirements. The signatures match.
    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn application_did_finish_launching(&self, _notification: &NSNotification) {
            let app = NSApplication::sharedApplication(self.mtm());
            // An app that starts from a terminal must ask for activation.
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn application_should_terminate_after_last_window_closed(
            &self,
            _sender: &NSApplication,
        ) -> bool {
            true
        }
    }

    impl AppDelegate {
        #[unsafe(method(openConfig:))]
        fn open_config_menu(&self, _sender: Option<&AnyObject>) {
            dispatch(Action::OpenConfig);
        }

        #[unsafe(method(reloadConfig:))]
        fn reload_config_menu(&self, _sender: Option<&AnyObject>) {
            dispatch(Action::ReloadConfig);
        }
    }
);

impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: `init` is the designated initializer of NSObject.
        unsafe { msg_send![super(this), init] }
    }
}

/// Adds one item to `menu`. An app action has no key equivalent: the
/// keymap is the only source of keys for app actions.
fn add_item(
    mtm: MainThreadMarker,
    menu: &NSMenu,
    title: &str,
    action: Sel,
    key: &str,
    target: Option<&AnyObject>,
) -> Retained<NSMenuItem> {
    // SAFETY: `action` is a valid action selector of the target or of the
    // responder chain.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            Some(action),
            &NSString::from_str(key),
        )
    };
    if let Some(target) = target {
        // SAFETY: The target is the app delegate, which lives as long as the app.
        unsafe { item.setTarget(Some(target)) };
    }
    menu.addItem(&item);
    item
}

fn submenu(mtm: MainThreadMarker, main: &NSMenu, title: &str) -> Retained<NSMenu> {
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
    let item = NSMenuItem::new(mtm);
    item.setSubmenu(Some(&menu));
    main.addItem(&item);
    menu
}

fn build_menu(mtm: MainThreadMarker, app: &NSApplication, delegate: &AppDelegate) {
    let main = NSMenu::new(mtm);
    let target: &AnyObject = delegate;

    let app_menu = submenu(mtm, &main, "Better Finder");
    add_item(
        mtm,
        &app_menu,
        "About Better Finder",
        sel!(orderFrontStandardAboutPanel:),
        "",
        None,
    );
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    add_item(
        mtm,
        &app_menu,
        "Settings\u{2026}",
        sel!(openConfig:),
        "",
        Some(target),
    );
    add_item(
        mtm,
        &app_menu,
        "Reload Config",
        sel!(reloadConfig:),
        "",
        Some(target),
    );
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    add_item(mtm, &app_menu, "Hide Better Finder", sel!(hide:), "h", None);
    let others = add_item(
        mtm,
        &app_menu,
        "Hide Others",
        sel!(hideOtherApplications:),
        "h",
        None,
    );
    others
        .setKeyEquivalentModifierMask(NSEventModifierFlags::Command | NSEventModifierFlags::Option);
    add_item(
        mtm,
        &app_menu,
        "Show All",
        sel!(unhideAllApplications:),
        "",
        None,
    );
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    add_item(
        mtm,
        &app_menu,
        "Quit Better Finder",
        sel!(terminate:),
        "",
        None,
    );

    // The prompt text field needs the standard edit commands and keys.
    let edit = submenu(mtm, &main, "Edit");
    add_item(mtm, &edit, "Undo", sel!(undo:), "z", None);
    add_item(mtm, &edit, "Redo", sel!(redo:), "Z", None);
    edit.addItem(&NSMenuItem::separatorItem(mtm));
    add_item(mtm, &edit, "Cut", sel!(cut:), "x", None);
    add_item(mtm, &edit, "Copy", sel!(copy:), "c", None);
    add_item(mtm, &edit, "Paste", sel!(paste:), "v", None);
    add_item(mtm, &edit, "Select All", sel!(selectAll:), "a", None);

    let window_menu = submenu(mtm, &main, "Window");
    add_item(
        mtm,
        &window_menu,
        "Minimize",
        sel!(performMiniaturize:),
        "m",
        None,
    );
    add_item(mtm, &window_menu, "Zoom", sel!(performZoom:), "", None);

    app.setMainMenu(Some(&main));
    app.setWindowsMenu(Some(&window_menu));
}

// ---- Start ----

/// Starts the app. Returns when the app quits.
pub fn run(options: RunOptions) -> ExitCode {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("better-finder: the app must start on the main thread");
        return ExitCode::FAILURE;
    };
    let LoadResult {
        config,
        diagnostics,
        files,
    } = Config::load();
    let (start, select, start_error) = resolve_start(options.path.as_deref(), &config);
    let pane_options = PaneOptions {
        show_hidden: config.show_hidden,
        sort: config.sort,
        filter_mode: config.filter_mode,
    };
    let (pane, request) = Pane::new(start.clone(), pane_options);
    // Start the first read now. It runs while AppKit starts.
    let generation = request.generation;
    let first_load = loader::spawn_first(request, config.sort);
    let load_started = Instant::now();

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let delegate = AppDelegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    let scripted = options.screenshot.is_some() || !options.keys.is_empty();
    let window = window::create_window(mtm, &config, !scripted);
    window::apply_appearance(&window, config.theme);
    let dark = theme::is_dark(&window.effectiveAppearance());
    let (style, warnings) = Style::new(&config, dark);
    let ui = window::create_views(mtm, &window);
    window.setTitle(&NSString::from_str(&display_path(&start)));
    window::apply_look(&ui, &style, config.titlebar_style);

    let mut state = AppState {
        config,
        style,
        pane,
        keys: KeymapState::default(),
        message: None,
        loading_visible: false,
        help: None,
        ui,
        config_files: files,
        prompt: None,
        pending_select: select.map(|name| (start, name)),
        message_id: 0,
        load_generation: generation,
        load_started: Some(load_started),
        content_dirty: true,
        reveal_cursor: true,
        rendered: Rendered::default(),
        window_shown: false,
        _delegate: delegate.clone(),
    };
    if let Some(text) = start_error.or_else(|| config_problem(&diagnostics, &warnings)) {
        state.set_message(text, true);
    }
    STATE.with(|cell| *cell.borrow_mut() = Some(state));

    after(Duration::from_millis(100), move || {
        loading_check(generation)
    });
    watcher::start();
    watch_config_files();
    // Show the window after the first load. When the load is slow, show
    // it after 150 ms with `Loading…`.
    after(Duration::from_millis(150), show_window);
    debug::init(options.screenshot, options.keys);
    // AppKit setup takes much longer than the read of a normal directory.
    // When the result is there, apply it and draw before the run loop starts.
    if let Some((request, result, read_time)) = first_load.try_take() {
        finish_load(request, result, read_time);
    }
    // The first frame does not need the menu.
    build_menu(mtm, &app, &delegate);

    app.run();
    ExitCode::SUCCESS
}

/// The start directory, a name to select in it, and an error message.
/// - `PATH` on the command line: a directory, or a file (its parent opens
///   with the cursor on the file).
/// - Else `start-directory` from the config, else the home directory.
fn resolve_start(arg: Option<&str>, config: &Config) -> (PathBuf, Option<String>, Option<String>) {
    let mut error = None;
    if let Some(arg) = arg {
        let cwd = std::env::current_dir().unwrap_or_else(|_| home_dir());
        let path = expand_path(arg, &cwd);
        match std::fs::metadata(&path) {
            Ok(meta) if meta.is_dir() => return (path, None, None),
            Ok(_) => {
                if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
                    let name = name.to_string_lossy().into_owned();
                    return (parent.to_path_buf(), Some(name), None);
                }
            }
            Err(_) => error = Some(format!("No such folder: {}", display_path(&path))),
        }
    }
    let start = config
        .start_directory
        .as_ref()
        .map(|p| expand_path(&p.to_string_lossy(), &home_dir()))
        .unwrap_or_else(home_dir);
    (start, None, error)
}

fn show_window() {
    let shown = with(|s| {
        if s.window_shown {
            return None;
        }
        s.window_shown = true;
        Some(s.ui.clone())
    });
    let Some(Some(ui)) = shown else { return };
    ui.window.makeKeyAndOrderFront(None);
    ui.window.makeFirstResponder(Some(list_responder(&ui.list)));
    ui.window.displayIfNeeded();
    // Layer-backed views draw when Core Animation commits. Before the run
    // loop starts, nothing commits, so push the first frame now.
    // SAFETY: `+[CATransaction flush]` takes no arguments and returns void.
    unsafe {
        let _: () = msg_send![objc2::class!(CATransaction), flush];
    }
}

fn list_responder(list: &ListView) -> &NSResponder {
    list.as_super().as_super()
}

/// A short text for config problems: the first one and the count of the others.
fn config_problem(diagnostics: &[Diagnostic], warnings: &[String]) -> Option<String> {
    let first = diagnostics
        .first()
        .map(|d| {
            let file = d
                .file
                .as_deref()
                .and_then(Path::file_name)
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "config".to_string());
            format!("{file}:{}: {}", d.line, d.message)
        })
        .or_else(|| warnings.first().cloned())?;
    let total = diagnostics.len() + warnings.len();
    Some(if total > 1 {
        format!("{first} (+{} more)", total - 1)
    } else {
        first
    })
}

// ---- Keys and actions ----

/// Sends one key event to the keymap. True when the key had a use.
pub fn handle_key(event: KeyEvent) -> bool {
    let fed = with(|s| {
        let had_pending = !s.keys.pending().is_empty();
        (s.keys.feed(&s.config.keymap, &event), had_pending)
    });
    let Some((outcome, had_pending)) = fed else {
        return false;
    };
    match outcome {
        KeyOutcome::Action(action) => {
            dispatch(action);
            true
        }
        KeyOutcome::Pending => {
            redraw_status();
            true
        }
        KeyOutcome::Unbound => {
            if had_pending {
                redraw_status();
            }
            had_pending
        }
    }
}

/// Work for AppKit that must run after the state borrow ends.
enum Effect {
    OpenFile(PathBuf),
    Reveal(PathBuf),
    QuickLookToggle,
    QuickLookClose,
    OpenPrompt {
        label: &'static str,
        text: String,
        select: (usize, usize),
    },
    ClosePrompt,
    /// Show or hide the help overlay, as `AppState::help` says.
    Help,
    ReloadConfig,
    WatchConfig,
    Quit,
}

/// Does one action, then updates the views.
pub fn dispatch(action: Action) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let ql_visible = quicklook::is_visible(mtm);
    let effects = with(|s| s.perform(action, ql_visible)).unwrap_or_default();
    run_effects(mtm, effects);
    sync_ui();
}

fn run_effects(mtm: MainThreadMarker, effects: Vec<Effect>) {
    for effect in effects {
        match effect {
            Effect::OpenFile(path) => open_file(&path),
            Effect::Reveal(path) => {
                if let Some(url) = NSURL::from_file_path(&path) {
                    let urls = NSArray::from_retained_slice(&[url]);
                    NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&urls);
                }
            }
            Effect::QuickLookToggle => quicklook::toggle(mtm),
            Effect::QuickLookClose => quicklook::close(mtm),
            Effect::OpenPrompt {
                label,
                text,
                select,
            } => {
                let Some((ui, caret)) = with_ref(|s| (s.ui.clone(), s.style.foreground.clone()))
                else {
                    continue;
                };
                ui.root.set_prompt_visible(true);
                ui.prompt.open(&ui.window, label, &text, select, &caret);
            }
            Effect::ClosePrompt => {
                let Some(ui) = with_ref(|s| s.ui.clone()) else {
                    continue;
                };
                ui.window.makeFirstResponder(Some(list_responder(&ui.list)));
                ui.root.set_prompt_visible(false);
            }
            Effect::Help => {
                let Some((ui, height)) =
                    with_ref(|s| (s.ui.clone(), s.help.as_ref().map(|m| m.height(&s.style))))
                else {
                    continue;
                };
                ui.root.set_help(height.is_some(), height.unwrap_or(0.0));
                ui.help.view.setNeedsDisplay(true);
            }
            Effect::ReloadConfig => reload_config(),
            Effect::WatchConfig => watch_config_files(),
            Effect::Quit => NSApplication::sharedApplication(mtm).terminate(None),
        }
    }
}

fn open_file(path: &Path) {
    let opened =
        NSURL::from_file_path(path).is_some_and(|url| NSWorkspace::sharedWorkspace().openURL(&url));
    if !opened {
        show_error(format!("Cannot open \"{}\"", file_name(path)));
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| display_path(path))
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The UTF-16 length of the name without its extension, like Finder
/// selects for a rename. A plain directory selects the full name.
fn stem_utf16_len(entry: &Entry) -> usize {
    let name = &entry.name;
    if entry.kind == EntryKind::Directory && !entry.is_package {
        return utf16_len(name);
    }
    match name.rfind('.') {
        Some(i) if i > 0 => utf16_len(&name[..i]),
        _ => utf16_len(name),
    }
}

impl AppState {
    fn set_message(&mut self, text: String, error: bool) {
        self.message_id += 1;
        let id = self.message_id;
        self.message = Some(Message {
            text,
            error,
            born: Instant::now(),
            id,
        });
        if !error {
            // Redraw a few times during the fade, then remove the message.
            for ms in [3000, 3100, 3200, 3300, 3400, 3500] {
                after(Duration::from_millis(ms), move || message_tick(id));
            }
        }
    }

    fn set_info(&mut self, text: impl Into<String>) {
        self.set_message(text.into(), false);
    }

    fn set_error(&mut self, text: impl Into<String>) {
        self.set_message(text.into(), true);
    }

    /// Visible rows minus one, at least one.
    fn page_rows(&self) -> isize {
        let height = self.ui.scroll.contentSize().height;
        let rows = (height / self.style.row_height).floor().max(0.0) as isize;
        (rows - 1).max(1)
    }

    fn start_load(&mut self, request: LoadRequest) {
        let generation = request.generation;
        self.load_generation = generation;
        self.load_started = Some(Instant::now());
        loader::spawn(request, self.pane.sort());
        after(Duration::from_millis(100), move || {
            loading_check(generation)
        });
    }

    fn open_prompt(
        &mut self,
        mode: PromptMode,
        text: String,
        select: (usize, usize),
        fx: &mut Vec<Effect>,
    ) {
        let label = mode.label();
        self.prompt = Some(mode);
        if self.help.take().is_some() {
            fx.push(Effect::Help);
        }
        fx.push(Effect::OpenPrompt {
            label,
            text,
            select,
        });
    }

    fn close_prompt(&mut self, fx: &mut Vec<Effect>) {
        self.prompt = None;
        fx.push(Effect::ClosePrompt);
    }

    fn current_or_cwd(&self) -> PathBuf {
        self.pane
            .current()
            .map(|e| e.path.clone())
            .unwrap_or_else(|| self.pane.cwd().to_path_buf())
    }

    fn perform(&mut self, action: Action, ql_visible: bool) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.reveal_cursor = true;
        if self.message.as_ref().is_some_and(|m| m.error) {
            self.message = None;
        }
        let page = self.page_rows();
        let half = (page / 2).max(1);
        match action {
            Action::CursorUp => self.pane.move_cursor(-1),
            Action::CursorDown => self.pane.move_cursor(1),
            Action::CursorTop => self.pane.cursor_top(),
            Action::CursorBottom => self.pane.cursor_bottom(),
            Action::PageUp => self.pane.move_cursor(-page),
            Action::PageDown => self.pane.move_cursor(page),
            Action::HalfPageUp => self.pane.move_cursor(-half),
            Action::HalfPageDown => self.pane.move_cursor(half),
            Action::Open => match self.pane.open_current() {
                OpenOutcome::Navigate(request) => self.start_load(request),
                OpenOutcome::OpenFile(path) => fx.push(Effect::OpenFile(path)),
                OpenOutcome::Nothing => {}
            },
            Action::Parent => {
                if let Some(request) = self.pane.parent() {
                    self.start_load(request);
                }
            }
            Action::Back => {
                if let Some(request) = self.pane.back() {
                    self.start_load(request);
                }
            }
            Action::Forward => {
                if let Some(request) = self.pane.forward() {
                    self.start_load(request);
                }
            }
            Action::Goto(text) => {
                let path = expand_path(&text, self.pane.cwd());
                let request = self.pane.navigate_to(path);
                self.start_load(request);
            }
            Action::GotoPrompt => {
                self.open_prompt(PromptMode::Goto, String::new(), (0, 0), &mut fx)
            }
            Action::QuickLook => {
                if ql_visible || self.pane.current().is_some() {
                    fx.push(Effect::QuickLookToggle);
                }
            }
            Action::ToggleHidden => {
                self.pane.toggle_hidden();
                self.content_dirty = true;
            }
            Action::Filter => {
                let text = self.pane.filter().to_string();
                let select = (0, utf16_len(&text));
                self.open_prompt(PromptMode::Filter, text, select, &mut fx);
            }
            Action::Cancel => {
                if ql_visible {
                    fx.push(Effect::QuickLookClose);
                } else if self.help.take().is_some() {
                    fx.push(Effect::Help);
                } else if !self.pane.filter().is_empty() {
                    self.pane.clear_filter();
                    self.content_dirty = true;
                } else {
                    self.message = None;
                }
            }
            Action::Sort(key) => {
                let mut sort = self.pane.sort();
                sort.key = key;
                self.pane.set_sort(sort);
                self.content_dirty = true;
            }
            Action::ToggleSortReverse => {
                let mut sort = self.pane.sort();
                sort.reverse = !sort.reverse;
                self.pane.set_sort(sort);
                self.content_dirty = true;
            }
            Action::Refresh => {
                let request = self.pane.refresh();
                self.start_load(request);
            }
            Action::Help => {
                self.help = match self.help {
                    Some(_) => None,
                    None => Some(HelpModel::build(&self.config.keymap, &self.style)),
                };
                fx.push(Effect::Help);
            }
            Action::Rename => {
                if let Some(entry) = self.pane.current() {
                    let name = entry.name.clone();
                    let select = (0, stem_utf16_len(entry));
                    let mode = PromptMode::Rename {
                        original: name.clone(),
                    };
                    self.open_prompt(mode, name, select, &mut fx);
                }
            }
            Action::NewFolder => {
                let text = "untitled folder".to_string();
                let select = (0, utf16_len(&text));
                self.open_prompt(PromptMode::NewFolder, text, select, &mut fx);
            }
            Action::Trash => self.trash(),
            Action::CopyPath => {
                let path = self.current_or_cwd();
                let text = path.to_string_lossy().into_owned();
                if copy_to_pasteboard(&text) {
                    self.set_info(format!("Copied \"{}\"", display_path(&path)));
                } else {
                    self.set_error("Cannot copy to the pasteboard");
                }
            }
            Action::CopyName => {
                let name = file_name(&self.current_or_cwd());
                if copy_to_pasteboard(&name) {
                    self.set_info(format!("Copied \"{name}\""));
                } else {
                    self.set_error("Cannot copy to the pasteboard");
                }
            }
            Action::OpenTerminal => {
                let terminal = self.config.terminal.clone();
                let args = [
                    OsString::from("-a"),
                    OsString::from(&terminal),
                    self.pane.cwd().into(),
                ];
                if let Err(err) = spawn_open(
                    &args,
                    format!("Cannot open the terminal app \"{terminal}\""),
                ) {
                    self.set_error(format!("Cannot run open: {err}"));
                }
            }
            Action::RevealInFinder => fx.push(Effect::Reveal(self.current_or_cwd())),
            Action::OpenConfig => self.open_config(&mut fx),
            Action::ReloadConfig => fx.push(Effect::ReloadConfig),
            Action::Quit => fx.push(Effect::Quit),
            Action::Unbind => {}
        }
        fx
    }

    fn trash(&mut self) {
        let Some(entry) = self.pane.current() else {
            return;
        };
        let (name, path) = (entry.name.clone(), entry.path.clone());
        let Some(url) = NSURL::from_file_path(&path) else {
            self.set_error(format!("Cannot move \"{name}\" to Trash"));
            return;
        };
        let manager = NSFileManager::defaultManager();
        match manager.trashItemAtURL_resultingItemURL_error(&url, None) {
            Ok(()) => {
                self.set_info(format!("Moved \"{name}\" to Trash"));
                let request = self.pane.refresh();
                self.start_load(request);
            }
            Err(err) => {
                let reason = err.localizedDescription();
                self.set_error(format!("Cannot move \"{name}\" to Trash: {reason}"));
            }
        }
    }

    fn open_config(&mut self, fx: &mut Vec<Effect>) {
        let path = Config::default_path();
        if !path.exists() {
            let created = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&path, default_config_template()));
            match created {
                Ok(()) => {
                    self.set_info(format!("Created {}", display_path(&path)));
                    fx.push(Effect::WatchConfig);
                }
                Err(err) => {
                    self.set_error(format!("Cannot create {}: {err}", display_path(&path)));
                    return;
                }
            }
        }
        let args = [OsString::from("-t"), path.clone().into_os_string()];
        if let Err(err) = spawn_open(&args, format!("Cannot open {}", display_path(&path))) {
            self.set_error(format!("Cannot run open: {err}"));
        }
    }

    /// Applies the text of the prompt (the user pressed Enter).
    fn accept_prompt(&mut self, text: String) -> Vec<Effect> {
        let mut fx = Vec::new();
        let Some(mode) = self.prompt.clone() else {
            return fx;
        };
        if self.message.as_ref().is_some_and(|m| m.error) {
            self.message = None;
        }
        let cwd = self.pane.cwd().to_path_buf();
        match mode {
            PromptMode::Filter => self.close_prompt(&mut fx),
            PromptMode::Goto => {
                let input = text.trim();
                if input.is_empty() {
                    self.close_prompt(&mut fx);
                    return fx;
                }
                let path = expand_path(input, &cwd);
                match std::fs::metadata(&path) {
                    Ok(meta) if meta.is_dir() => {
                        self.close_prompt(&mut fx);
                        let request = self.pane.navigate_to(path);
                        self.start_load(request);
                    }
                    Ok(_) => {
                        let parent = path.parent().map(Path::to_path_buf);
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                        if let (Some(parent), Some(name)) = (parent, name) {
                            self.close_prompt(&mut fx);
                            self.pending_select = Some((parent.clone(), name));
                            let request = self.pane.navigate_to(parent);
                            self.start_load(request);
                        }
                    }
                    Err(_) => self.set_error("No such folder"),
                }
            }
            PromptMode::Rename { original } => {
                if text == original {
                    self.close_prompt(&mut fx);
                    return fx;
                }
                if let Err(problem) = check_new_name(&cwd, &text, Some(&original)) {
                    self.set_error(problem);
                    return fx;
                }
                match std::fs::rename(cwd.join(&original), cwd.join(&text)) {
                    Ok(()) => {
                        self.close_prompt(&mut fx);
                        self.pending_select = Some((cwd, text));
                        let request = self.pane.refresh();
                        self.start_load(request);
                    }
                    Err(err) => self.set_error(format!("Cannot rename \"{original}\": {err}")),
                }
            }
            PromptMode::NewFolder => {
                if let Err(problem) = check_new_name(&cwd, &text, None) {
                    self.set_error(problem);
                    return fx;
                }
                match std::fs::create_dir(cwd.join(&text)) {
                    Ok(()) => {
                        self.close_prompt(&mut fx);
                        self.pending_select = Some((cwd, text));
                        let request = self.pane.refresh();
                        self.start_load(request);
                    }
                    Err(err) => self.set_error(format!("Cannot create \"{text}\": {err}")),
                }
            }
        }
        fx
    }
}

/// Checks a name for rename and new folder. `original` is the old name of
/// a rename: a change of case only is not a conflict.
fn check_new_name(dir: &Path, name: &str, original: Option<&str>) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("The name cannot be empty".to_string());
    }
    if name.contains('/') {
        return Err("The name cannot contain \"/\"".to_string());
    }
    if name == "." || name == ".." {
        return Err(format!("The name \"{name}\" is reserved"));
    }
    let case_change = original.is_some_and(|o| o.to_lowercase() == name.to_lowercase());
    if !case_change && std::fs::symlink_metadata(dir.join(name)).is_ok() {
        return Err(format!("\"{name}\" already exists"));
    }
    Ok(())
}

fn copy_to_pasteboard(text: &str) -> bool {
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    // SAFETY: `NSPasteboardTypeString` is an immutable static from AppKit.
    let kind = unsafe { NSPasteboardTypeString };
    pasteboard.setString_forType(&NSString::from_str(text), kind)
}

/// Runs `/usr/bin/open` with `args` and does not wait. A thread waits for
/// the exit and shows `failure` when `open` fails.
fn spawn_open(args: &[OsString], failure: String) -> io::Result<()> {
    let mut child = Command::new("/usr/bin/open")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        if child.wait().is_ok_and(|status| !status.success()) {
            after_now(move || show_error(failure));
        }
    });
    Ok(())
}

fn show_error(text: String) {
    if with(|s| s.set_error(text)).is_some() {
        redraw_status();
    }
}

fn redraw_status() {
    if let Some(status) = with_ref(|s| s.ui.status.clone()) {
        status.setNeedsDisplay(true);
    }
}

fn message_tick(id: u64) {
    let redraw = with(|s| match &s.message {
        Some(m) if m.id == id => {
            if m.alpha() <= 0.0 {
                s.message = None;
            }
            true
        }
        _ => false,
    });
    if redraw == Some(true) {
        redraw_status();
    }
}

fn loading_check(generation: u64) {
    let show = with(|s| {
        let show = s.pane.is_loading() && s.load_generation == generation;
        if show {
            s.loading_visible = true;
        }
        show
    });
    if show == Some(true) {
        redraw_status();
    }
}

// ---- Directory loads ----

/// The loader calls this on the main queue when a read completes.
pub fn finish_load(request: LoadRequest, result: io::Result<Vec<Entry>>, read_time: Duration) {
    let started = Instant::now();
    let count = result.as_ref().map(Vec::len).unwrap_or(0);
    let outcome = with(|s| {
        let was_loaded = s.pane.is_loaded();
        let old_cwd = s.pane.cwd().to_path_buf();
        if !s.pane.finish_load(&request, result) {
            return None;
        }
        s.loading_visible = false;
        let total = s.load_started.take().map(|t| t.elapsed());
        let failed = s.pane.error().map(str::to_string);
        if let Some(error) = failed.clone() {
            s.pane.clear_error();
            s.set_error(error);
        }
        let cwd = s.pane.cwd().to_path_buf();
        if let Some((dir, name)) = s.pending_select.take() {
            if dir == cwd && failed.is_none() {
                s.pane.select_name(&name);
            }
        }
        s.content_dirty = true;
        let dir_changed = (!was_loaded && s.pane.is_loaded()) || cwd != old_cwd;
        Some((cwd, dir_changed, total))
    });
    let Some(Some((cwd, dir_changed, total))) = outcome else {
        return;
    };
    if dir_changed {
        icons::clear_paths();
        watcher::watch_directory(&cwd);
    }
    sync_ui();
    trace!(
        "load {}: {count} entries in {:.1} ms (read and sort {:.1} ms, main thread {} µs)",
        display_path(&cwd),
        total.unwrap_or(read_time).as_secs_f64() * 1000.0,
        read_time.as_secs_f64() * 1000.0,
        started.elapsed().as_micros(),
    );
    show_window();
    debug::on_load_finished();
}

/// The watcher calls this (debounced) when the current directory changes on disk.
pub fn auto_refresh() {
    let state = with(|s| {
        if !s.config.auto_refresh || !s.pane.is_loaded() {
            return false;
        }
        if s.pane.is_loading() {
            // Do not replace a load in progress. Try again later.
            return true;
        }
        let request = s.pane.refresh();
        s.start_load(request);
        false
    });
    if state == Some(true) {
        after(Duration::from_millis(150), auto_refresh);
    }
}

// ---- Mouse and focus ----

/// A click on a row. A double click opens the entry.
pub fn click_row(row: usize, double: bool) {
    let hit = with(|s| {
        if row >= s.pane.visible_len() {
            return false;
        }
        s.pane.set_cursor(row);
        if s.message.as_ref().is_some_and(|m| m.error) {
            s.message = None;
        }
        true
    });
    if hit != Some(true) {
        return;
    }
    sync_ui();
    if double {
        dispatch(Action::Open);
    }
}

/// The list became the first responder. A click on the list closes the
/// prompt: the filter stays, the other prompts cancel.
pub fn list_did_focus() {
    let Some(ui) = with_ref(|s| s.ui.clone()) else {
        return;
    };
    let list_is_first = ui.window.firstResponder().is_some_and(|r| {
        std::ptr::eq(
            Retained::as_ptr(&r).cast::<u8>(),
            Retained::as_ptr(&ui.list).cast::<u8>(),
        )
    });
    if !list_is_first {
        return;
    }
    let effects = with(|s| {
        let mut fx = Vec::new();
        if s.prompt.is_some() {
            s.close_prompt(&mut fx);
        }
        fx
    });
    if let (Some(fx), Some(mtm)) = (effects, MainThreadMarker::new()) {
        if !fx.is_empty() {
            run_effects(mtm, fx);
            sync_ui();
        }
    }
}

// ---- Prompt callbacks ----

/// The prompt text field, when the prompt is open. For `--debug-keys`.
pub fn prompt_field() -> Option<Retained<objc2_app_kit::NSTextField>> {
    with_ref(|s| s.prompt.is_some().then(|| s.ui.prompt.field.clone())).flatten()
}

/// The text of the prompt changed. The filter updates the list live.
pub fn prompt_changed(text: String) {
    let changed = with(|s| {
        if s.prompt != Some(PromptMode::Filter) {
            return false;
        }
        s.pane.set_filter(&text);
        s.content_dirty = true;
        s.reveal_cursor = true;
        true
    });
    if changed == Some(true) {
        sync_ui();
    }
}

pub fn prompt_accept(text: String) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let effects = with(|s| s.accept_prompt(text)).unwrap_or_default();
    run_effects(mtm, effects);
    sync_ui();
}

pub fn prompt_cancel() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let effects = with(|s| {
        let mut fx = Vec::new();
        match s.prompt.clone() {
            None => return fx,
            Some(PromptMode::Filter) => {
                s.pane.clear_filter();
                s.content_dirty = true;
            }
            Some(_) => {}
        }
        if s.message.as_ref().is_some_and(|m| m.error) {
            s.message = None;
        }
        s.close_prompt(&mut fx);
        fx
    })
    .unwrap_or_default();
    run_effects(mtm, effects);
    sync_ui();
}

/// Up and Down in the filter prompt move the list cursor.
pub fn prompt_move(delta: isize) -> bool {
    let moved = with(|s| {
        if s.prompt != Some(PromptMode::Filter) {
            return false;
        }
        s.pane.move_cursor(delta);
        true
    })
    .unwrap_or(false);
    if moved {
        sync_ui();
    }
    moved
}

/// Tab in the "go to" prompt completes the directory name. The directory
/// read runs on a background thread.
pub fn prompt_complete(text: String) -> bool {
    let cwd =
        with_ref(|s| (s.prompt == Some(PromptMode::Goto)).then(|| s.pane.cwd().to_path_buf()));
    let Some(Some(cwd)) = cwd else { return false };
    let (dir_text, prefix) = match text.rfind('/') {
        Some(i) => (text[..=i].to_string(), text[i + 1..].to_string()),
        None if text == "~" => ("~/".to_string(), String::new()),
        None => (String::new(), text.clone()),
    };
    let dir = if dir_text.is_empty() {
        cwd
    } else {
        expand_path(&dir_text, &cwd)
    };
    std::thread::spawn(move || {
        let names = matching_directories(&dir, &prefix);
        if let Some(completion) = complete_name(&names, &prefix) {
            let new_text = format!("{dir_text}{completion}");
            after_now(move || prompt_complete_result(text, new_text));
        }
    });
    true
}

/// The names of the subdirectories of `dir` that start with `prefix`
/// (case-insensitive). Hidden names only when `prefix` starts with `.`.
fn matching_directories(dir: &Path, prefix: &str) -> Vec<String> {
    let lower = prefix.to_lowercase();
    let hidden = prefix.starts_with('.');
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    read.flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if (name.starts_with('.') && !hidden) || !name.to_lowercase().starts_with(&lower) {
                return None;
            }
            let is_dir = std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir());
            is_dir.then_some(name)
        })
        .collect()
}

/// One match gives the name and `/`. More matches give their longest common
/// prefix (case-insensitive), when it is longer than `prefix`.
fn complete_name(names: &[String], prefix: &str) -> Option<String> {
    match names {
        [] => None,
        [one] => Some(format!("{one}/")),
        [first, rest @ ..] => {
            let mut common: Vec<char> = first.chars().collect();
            for name in rest {
                let same = common
                    .iter()
                    .zip(name.chars())
                    .take_while(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
                    .count();
                common.truncate(same);
            }
            (common.len() > prefix.chars().count()).then(|| common.into_iter().collect())
        }
    }
}

fn prompt_complete_result(original: String, new_text: String) {
    let prompt = with_ref(|s| (s.prompt == Some(PromptMode::Goto)).then(|| s.ui.prompt.clone()));
    let Some(Some(prompt)) = prompt else { return };
    if prompt.text() == original {
        prompt.replace_text(&new_text);
    }
}

// ---- Config ----

fn watch_config_files() {
    let mut files = with_ref(|s| s.config_files.clone()).unwrap_or_default();
    let main = Config::default_path();
    if !files.contains(&main) {
        files.push(main);
    }
    watcher::watch_config(files);
}

/// Loads the config on a background thread, then applies it.
fn reload_config() {
    let spawned = std::thread::Builder::new()
        .name("bf-config".into())
        .spawn(|| {
            let result = Config::load();
            DispatchQueue::main().exec_async(move || apply_config(result));
        });
    if spawned.is_err() {
        apply_config(Config::load());
    }
}

/// The watcher calls this (debounced) when a config file changes.
pub fn auto_reload_config() {
    reload_config();
}

/// Applies a new config without a restart: theme, fonts, row height,
/// keymap, sort, hidden flag, filter mode, columns, opacity, title bar.
fn apply_config(result: LoadResult) {
    let LoadResult {
        config,
        diagnostics,
        files,
    } = result;
    let Some(window) = with_ref(|s| s.ui.window.clone()) else {
        return;
    };
    window::apply_appearance(&window, config.theme);
    let dark = theme::is_dark(&window.effectiveAppearance());
    let (style, warnings) = Style::new(&config, dark);
    let titlebar = config.titlebar_style;
    let ui = with(|s| {
        s.pane.set_show_hidden(config.show_hidden);
        s.pane.set_sort(config.sort);
        s.pane.set_filter_mode(config.filter_mode);
        s.keys.reset();
        s.config = config;
        s.config_files = files;
        s.style = style.clone();
        if s.help.is_some() {
            s.help = Some(HelpModel::build(&s.config.keymap, &s.style));
        }
        s.content_dirty = true;
        s.rendered.valid = false;
        match config_problem(&diagnostics, &warnings) {
            Some(problem) => s.set_error(problem),
            None => s.set_info("Config reloaded"),
        }
        s.ui.clone()
    });
    let Some(ui) = ui else { return };
    window::apply_look(&ui, &style, titlebar);
    watch_config_files();
    if let Some(mtm) = MainThreadMarker::new() {
        run_effects(mtm, vec![Effect::Help]);
    }
    sync_ui();
}

/// The system appearance changed. With `theme = auto`, apply the new theme.
pub fn appearance_changed() {
    let info = with_ref(|s| (s.ui.window.clone(), s.config.theme, s.style.dark));
    let Some((window, mode, was_dark)) = info else {
        return;
    };
    if mode != ThemeMode::Auto {
        return;
    }
    let dark = theme::is_dark(&window.effectiveAppearance());
    if dark == was_dark {
        return;
    }
    let updated = with(|s| {
        let (style, _) = Style::new(&s.config, dark);
        s.style = style.clone();
        if s.help.is_some() {
            s.help = Some(HelpModel::build(&s.config.keymap, &s.style));
        }
        s.content_dirty = true;
        s.rendered.valid = false;
        (s.ui.clone(), style, s.config.titlebar_style)
    });
    let Some((ui, style, titlebar)) = updated else {
        return;
    };
    window::apply_look(&ui, &style, titlebar);
    sync_ui();
}

// ---- View updates ----

enum Redraw {
    None,
    Rows(usize, usize),
    All,
}

/// The AppKit work to bring the views up to date with the state.
struct UiPlan {
    window: Retained<NSWindow>,
    list: Retained<ListView>,
    status: Retained<StatusBar>,
    metrics: Option<(usize, f64)>,
    redraw: Redraw,
    scroll_to: Option<usize>,
    title: Option<(String, PathBuf)>,
    quick_look_reload: bool,
}

impl AppState {
    fn plan_ui(&mut self) -> UiPlan {
        let cwd = self.pane.cwd().to_path_buf();
        let cursor = self.pane.cursor();
        let len = self.pane.visible_len();
        let current = self.pane.current().map(|e| e.path.clone());
        let row_height = self.style.row_height;
        let r = &self.rendered;
        let dir_changed = !r.valid || r.cwd != cwd;
        let layout_changed = dir_changed || r.len != len || r.row_height != row_height;
        let cursor_changed = r.cursor != cursor;
        let redraw = if self.content_dirty || layout_changed {
            Redraw::All
        } else if cursor_changed {
            Redraw::Rows(r.cursor, cursor)
        } else {
            Redraw::None
        };
        let scroll_to = (self.reveal_cursor || dir_changed || cursor_changed).then_some(cursor);
        let title = dir_changed.then(|| (display_path(&cwd), cwd.clone()));
        let quick_look_reload = dir_changed || r.current != current;
        self.content_dirty = false;
        self.reveal_cursor = false;
        self.rendered = Rendered {
            valid: true,
            cwd,
            cursor,
            len,
            current,
            row_height,
        };
        UiPlan {
            window: self.ui.window.clone(),
            list: self.ui.list.clone(),
            status: self.ui.status.clone(),
            metrics: layout_changed.then_some((len, row_height)),
            redraw,
            scroll_to,
            title,
            quick_look_reload,
        }
    }
}

impl UiPlan {
    fn apply(self) {
        if let Some((rows, row_height)) = self.metrics {
            self.list.set_metrics(rows, row_height);
        }
        match self.redraw {
            Redraw::All => self.list.setNeedsDisplay(true),
            Redraw::Rows(a, b) => {
                self.list.setNeedsDisplayInRect(self.list.row_rect(a));
                self.list.setNeedsDisplayInRect(self.list.row_rect(b));
            }
            Redraw::None => {}
        }
        if let Some(row) = self.scroll_to {
            self.list.scroll_to_row(row);
        }
        self.status.setNeedsDisplay(true);
        if let Some((title, path)) = self.title {
            self.window.setTitle(&NSString::from_str(&title));
            // The proxy icon costs a few ms. Set it after the frame.
            after_now(move || set_represented_url(&path));
        }
        if self.quick_look_reload {
            if let Some(mtm) = MainThreadMarker::new() {
                quicklook::reload_if_visible(mtm);
            }
        }
    }
}

/// Sets the proxy icon of the title bar, when `path` is still the directory.
fn set_represented_url(path: &Path) {
    let window = with_ref(|s| (s.pane.cwd() == path).then(|| s.ui.window.clone()));
    if let Some(Some(window)) = window {
        window.setRepresentedURL(NSURL::from_directory_path(path).as_deref());
    }
}

/// Brings the views up to date with the state. Call it after each change.
pub fn sync_ui() {
    if let Some(plan) = with(AppState::plan_ui) {
        plan.apply();
    }
}
