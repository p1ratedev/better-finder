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
use std::ffi::{OsStr, OsString};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use bf_core::config::{Diagnostic, ThemeMode, default_config_template};
use bf_core::filter::is_path_query;
use bf_core::fs::{display_path, expand_path, home_dir};
use bf_core::keymap::{Key, KeyPress, NamedKey};
use bf_core::pane::{LoadKind, PaneOptions};
use bf_core::viewport::{self, Motion, Position, Viewport};
use bf_core::{
    Action, Config, Entry, EntryKind, KeyEvent, KeyOutcome, Keymap, KeymapState, LoadRequest,
    LoadResult, OpenOutcome, Pane,
};
use dispatch2::{DispatchQueue, DispatchTime};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{ClassType, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAppearanceCustomization, NSApplication,
    NSApplicationActivationPolicy, NSApplicationDelegate, NSEventModifierFlags, NSMenu, NSMenuItem,
    NSPasteboard, NSPasteboardTypeString, NSPasteboardURLReadingFileURLsOnlyKey,
    NSPasteboardWriting, NSResponder, NSWindow, NSWindowDelegate, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSDictionary, NSFileManager, NSNotification, NSNumber, NSObject,
    NSObjectProtocol, NSString, NSURL,
};

use crate::columns;
use crate::context_menu;
use crate::debug::{self, trace};
use crate::editor;
use crate::file_ops::{self, TransferMode, TransferReport, VolumeInfo};
use crate::header::{self, HeaderView};
use crate::help::HelpModel;
use crate::icons;
use crate::list_view::ListView;
use crate::loader::{self, Delivery};
use crate::prompt::{self, PromptMode};
use crate::quicklook;
use crate::sidebar;
use crate::smooth::{self, ListScroll, Target};
use crate::status_bar::StatusBar;
use crate::theme::{self, Style};
use crate::volumes::{self, EjectOutcome, Volume};
use crate::watcher;
use crate::window::{self, Ui};

/// Where the text input of a prompt is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptPlace {
    /// The bottom prompt bar.
    Bar,
    /// The filter field in the toolbar.
    Search,
    /// The rename field over the name of the cursor row.
    Inline,
    /// The floating "go to" panel.
    Panel,
}

/// The matches of the "go to" panel.
#[derive(Default)]
struct GotoState {
    /// The folder part of the text, as typed (for example `~/Doc` gives `~/`).
    dir_text: String,
    matches: Vec<String>,
    selected: Option<usize>,
    /// The number of the most recent query. Old results are ignored.
    query: u64,
}

impl GotoState {
    /// Removes the matches. The query number stays, so that the results
    /// of old queries stay old.
    fn clear(&mut self) {
        self.dir_text.clear();
        self.matches.clear();
        self.selected = None;
    }
}

/// The options of a normal app start, from the command line.
pub struct RunOptions {
    pub path: Option<String>,
    pub screenshot: Option<PathBuf>,
    pub keys: Vec<debug::Step>,
    /// `--debug-appearance`: force light (`false`) or dark (`true`).
    pub appearance: Option<bool>,
}

/// A status bar message. Info messages fade after 3 s. Errors stay until
/// the next action. A progress message (`Ejecting "X"…`) does not fade: the
/// result of the task replaces it.
pub struct Message {
    pub text: String,
    pub error: bool,
    born: Instant,
    id: u64,
    /// A progress message: it does not fade.
    sticky: bool,
}

const FADE_START: f64 = 3.0;
const FADE_TIME: f64 = 0.4;

impl Message {
    /// 1.0 while the message is fresh, then down to 0.0 during the fade.
    pub fn alpha(&self) -> f64 {
        if self.error || self.sticky {
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
    history: (bool, bool),
    sort: Option<bf_core::sort::SortOptions>,
    /// The match list of the location field: its place and the number of rows.
    location: Option<(PromptPlace, usize)>,
}

/// The time after a busy message in which one more `eject` forces the eject.
const FORCE_WINDOW: Duration = Duration::from_secs(5);

/// The state of the eject flow (docs/UI.md, "Volumes and eject").
#[derive(Default)]
struct EjectState {
    /// The volume that ejects now: the mount point and the name.
    running: Option<(PathBuf, String)>,
    /// The volume that was busy, and the time of the busy message. One more
    /// `eject` of it within `FORCE_WINDOW` forces the eject.
    busy: Option<(PathBuf, Instant)>,
    /// The most recent volume that this app ejected. Its unmount
    /// notification shows no `was ejected` message. A volume list that has
    /// this mount point again (a new mount) clears it.
    ejected: Option<PathBuf>,
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
    prompt_place: PromptPlace,
    /// The volume of the current directory, for the path bar.
    pub volume: Option<VolumeInfo>,
    /// Open the inline rename on this path after the next load (new folder).
    pending_rename: Option<PathBuf>,
    goto: GotoState,
    /// The filter prompt has a path (the location field): `goto` has its
    /// matches, and `Enter` goes to the path.
    path_mode: bool,
    /// A page action: the scroll of the list for the next `sync_ui`.
    scroll_plan: Option<ListScroll>,
    /// (directory, name): put the cursor on `name` after the next load of `directory`.
    pending_select: Option<(PathBuf, String)>,
    message_id: u64,
    load_generation: u64,
    load_started: Option<Instant>,
    /// The kind of the load in progress.
    load_kind: LoadKind,
    /// The load in progress is an automatic refresh (from the watcher).
    load_auto: bool,
    /// Start one refresh when the load in progress completes.
    refresh_queued: bool,
    /// The queued refresh comes from the watcher.
    refresh_queued_auto: bool,
    /// The current directory is on a local volume.
    cwd_is_local: bool,
    /// When an automatic refresh changed the name under the cursor.
    list_changed_at: Option<Instant>,
    /// The number of the most recent "go to" check. `goto_checked` ignores old results.
    goto_check: u64,
    /// The list content changed (entries, filter, sort, hidden, style).
    content_dirty: bool,
    /// A user action: scroll so that the cursor is visible, also when it
    /// did not move. An automatic refresh does not scroll.
    reveal_cursor: bool,
    rendered: Rendered,
    window_shown: bool,
    /// The sidebar model that a thread builds at start.
    sidebar_start: Option<sidebar::PendingModel>,
    /// The mounted volumes that are not hidden. A background thread reads
    /// them at the start and after each mount, unmount, or rename.
    volumes: Vec<Volume>,
    eject: EjectState,
    /// The mount points of unmounted volumes. After the next load, the
    /// history forgets their folders (the load that leaves a volume adds
    /// its folder to the history).
    forget_after_load: Vec<PathBuf>,
    /// The target of the action in progress, when it is not the entry
    /// under the cursor: the background menu acts on the current folder.
    target: Option<PathBuf>,
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

/// Runs `work` on a new background thread, then `done` with its result on
/// the main queue.
pub fn in_background<T: Send + 'static>(
    name: &str,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) + Send + 'static,
) -> io::Result<()> {
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            let result = objc2::rc::autoreleasepool(|_| work());
            after_now(move || done(result));
        })?;
    Ok(())
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

    // SAFETY: `NSWindowDelegate` has no safety requirements. The signatures match.
    unsafe impl NSWindowDelegate for AppDelegate {
        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, _notification: &NSNotification) {
            // The cursor row changes between the active and the inactive color.
            redraw_list();
        }

        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _notification: &NSNotification) {
            redraw_list();
        }
    }

    impl AppDelegate {
        #[unsafe(method(systemColorsChanged:))]
        fn system_colors_changed(&self, _notification: &NSNotification) {
            // The accent color changed. The system colors are dynamic: a
            // redraw shows the new color. The folder icon bitmaps follow
            // the accent color: render them again.
            icons::clear_rasters();
            after_now(redraw_all);
        }

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

thread_local! {
    /// The menu items of app actions, for `sync_menu_keys`.
    static ACTION_ITEMS: RefCell<Vec<(Retained<NSMenuItem>, Action)>> =
        const { RefCell::new(Vec::new()) };
}

/// Adds one item to `menu`.
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
    let settings = add_item(
        mtm,
        &app_menu,
        "Settings\u{2026}",
        sel!(openConfig:),
        "",
        Some(target),
    );
    let reload = add_item(
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
    let quit = add_item(
        mtm,
        &app_menu,
        "Quit Better Finder",
        sel!(terminate:),
        "",
        None,
    );
    ACTION_ITEMS.with(|items| {
        *items.borrow_mut() = vec![
            (settings, Action::OpenConfig),
            (reload, Action::ReloadConfig),
            (quit, Action::Quit),
        ];
    });

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

/// Sets the key equivalents of the app action menu items from the keymap,
/// like Ghostty: the first binding of the action that is one key press with
/// `cmd`. No such binding gives no key equivalent. The menu then shows the
/// real keys, and the keys work while the prompt or Quick Look has focus.
/// The list view still gets a bound key first: the window precedes the menu.
fn sync_menu_keys(keymap: &Keymap) {
    let bindings = keymap.bindings();
    ACTION_ITEMS.with(|items| {
        for (item, action) in items.borrow().iter() {
            let key = bindings
                .iter()
                .filter(|(trigger, a)| a == action && trigger.0.len() == 1)
                .filter_map(|(trigger, _)| menu_key(trigger.0[0]))
                .next();
            let (key, mask) = key.unwrap_or((String::new(), NSEventModifierFlags::empty()));
            trace!("menu: {action} = {key:?} {mask:?}");
            item.setKeyEquivalent(&NSString::from_str(&key));
            item.setKeyEquivalentModifierMask(mask);
        }
    });
}

/// The menu key equivalent of a key press with `cmd`. An uppercase letter
/// implies shift in AppKit, so it needs no shift flag.
fn menu_key(press: KeyPress) -> Option<(String, NSEventModifierFlags)> {
    if !press.mods.cmd {
        return None;
    }
    let mut mask = NSEventModifierFlags::Command;
    if press.mods.ctrl {
        mask |= NSEventModifierFlags::Control;
    }
    if press.mods.alt {
        mask |= NSEventModifierFlags::Option;
    }
    if press.mods.shift {
        mask |= NSEventModifierFlags::Shift;
    }
    let key = match press.key {
        Key::Char(c) => c,
        Key::Named(named) => char::from_u32(match named {
            NamedKey::Enter => 0x0d,
            NamedKey::Escape => 0x1b,
            NamedKey::Space => 0x20,
            NamedKey::Tab => 0x09,
            NamedKey::Backspace => 0x08,
            NamedKey::Delete => 0xF728,
            NamedKey::Up => 0xF700,
            NamedKey::Down => 0xF701,
            NamedKey::Left => 0xF702,
            NamedKey::Right => 0xF703,
            NamedKey::Home => 0xF729,
            NamedKey::End => 0xF72B,
            NamedKey::PageUp => 0xF72C,
            NamedKey::PageDown => 0xF72D,
            NamedKey::F(n) => 0xF703 + u32::from(n),
        })?,
    };
    Some((key.to_string(), mask))
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
    // The sidebar model needs disk access too: build it on a thread now.
    let sidebar_start = config
        .sidebar
        .then(|| sidebar::spawn_build(config.sidebar_items.clone(), config.sidebar_locations));

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let delegate = AppDelegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    let scripted = options.screenshot.is_some() || !options.keys.is_empty();
    window::force_appearance(options.appearance);
    let window = window::create_window(mtm, &config);
    window::apply_appearance(&window, config.theme);
    let (style, warnings) = theme::style_for(&config, &window.effectiveAppearance());
    trace!(
        "startup: style after {} ms",
        debug::since_start().as_millis()
    );
    let ui = window::create_views(mtm, &window, &config, !scripted);
    window.setTitle(&NSString::from_str(&display_path(&start)));
    window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    window::apply_look(&ui, &style, &config);
    trace!(
        "startup: views after {} ms",
        debug::since_start().as_millis()
    );
    // SAFETY: The notification name is an immutable static. The delegate
    // lives as long as the app.
    unsafe {
        objc2_foundation::NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            &delegate,
            sel!(systemColorsChanged:),
            Some(objc2_app_kit::NSSystemColorsDidChangeNotification),
            None,
        );
    }

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
        prompt_place: PromptPlace::Bar,
        volume: None,
        pending_rename: None,
        goto: GotoState::default(),
        path_mode: false,
        scroll_plan: None,
        pending_select: select.map(|name| (start, name)),
        message_id: 0,
        load_generation: generation,
        load_started: Some(load_started),
        load_kind: LoadKind::Navigate,
        load_auto: false,
        refresh_queued: false,
        refresh_queued_auto: false,
        cwd_is_local: false,
        list_changed_at: None,
        goto_check: 0,
        content_dirty: true,
        reveal_cursor: true,
        rendered: Rendered::default(),
        window_shown: false,
        sidebar_start,
        volumes: Vec::new(),
        eject: EjectState::default(),
        forget_after_load: Vec::new(),
        target: None,
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
    volumes::observe(mtm);
    volumes::refresh();
    // Show the window after the first load. When the load is slow, show
    // it after 150 ms with `Loading…`.
    after(Duration::from_millis(150), show_window);
    debug::init(options.screenshot, options.keys);
    // AppKit setup takes much longer than the read of a normal directory.
    // When the result is there, apply it and draw before the run loop starts.
    if let Some(delivery) = first_load.try_take() {
        finish_load(delivery);
    }
    // The first frame does not need the menu.
    build_menu(mtm, &app, &delegate);
    if let Some(keymap) = with_ref(|s| s.config.keymap.clone()) {
        sync_menu_keys(&keymap);
    }
    context_menu::register_services(&app);

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
        let model = s.sidebar_start.take().and_then(|p| p.try_take());
        Some((s.ui.clone(), model))
    });
    let Some(Some((ui, model))) = shown else {
        return;
    };
    ui.window.makeKeyAndOrderFront(None);
    ui.window.makeFirstResponder(Some(list_responder(&ui.list)));
    ui.window.displayIfNeeded();
    // Layer-backed views draw when Core Animation commits. Before the run
    // loop starts, nothing commits, so push the first frame now.
    // SAFETY: `+[CATransaction flush]` takes no arguments and returns void.
    unsafe {
        let _: () = msg_send![objc2::class!(CATransaction), flush];
    }
    // The filter field and the sidebar rows come after the first frame:
    // they cost about 25 ms, and the layout does not change when they come.
    after_now(add_search_field);
    // The display link of the smooth scroll: make it now, so that the first
    // page action does not wait for it.
    after_now(prepare_smooth_scroll);
    if let Some(model) = model {
        after_now(move || sidebar::apply_model(model));
    }
}

fn prepare_smooth_scroll() {
    let list = with_ref(|s| s.config.smooth_scroll_time().map(|_| s.ui.list.clone()));
    if let Some(Some(list)) = list {
        smooth::prepare(&list);
    }
}

fn add_search_field() {
    let toolbar = with_ref(|s| s.config.toolbar.then(|| s.ui.toolbar.clone())).flatten();
    if let Some(toolbar) = toolbar {
        toolbar.add_search();
        sync_ui();
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

/// True for the actions that a held key (key repeat) can do: cursor moves.
fn repeat_allowed(action: &Action) -> bool {
    matches!(
        action,
        Action::CursorUp
            | Action::CursorDown
            | Action::CursorTop
            | Action::CursorBottom
            | Action::PageUp
            | Action::PageDown
            | Action::HalfPageUp
            | Action::HalfPageDown
    )
}

/// Sends one key event to the keymap. True when the key had a use.
/// `repeat` is true for the automatic repeats of a held key: they do only
/// cursor moves, so a held key never trashes or opens many items.
/// While the prompt is open, the keymap gets no keys (false). While the
/// help overlay is open, the keys go to the overlay (`help_key`).
pub fn handle_key(event: KeyEvent, repeat: bool) -> bool {
    if with_ref(|s| s.prompt.is_none() && s.help.is_some()) == Some(true) {
        return help_key(event, repeat);
    }
    let fed = with(|s| {
        if s.prompt.is_some() {
            return None;
        }
        let had_pending = !s.keys.pending().is_empty();
        Some((s.keys.feed(&s.config.keymap, &event), had_pending))
    });
    let Some(Some((outcome, had_pending))) = fed else {
        return false;
    };
    // A key other than a page action ends a smooth scroll at its target.
    // A page action during a scroll is a retarget.
    if !matches!(&outcome, KeyOutcome::Action(a) if is_page_action(a)) {
        smooth::finish();
    }
    match outcome {
        KeyOutcome::Action(action) => {
            if !repeat || repeat_allowed(&action) {
                dispatch(action);
            }
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

/// True for the actions that scroll with the smooth scroll rules.
fn is_page_action(action: &Action) -> bool {
    Motion::from_action(action).is_some_and(Motion::animates)
}

/// What a key does in the help overlay.
enum HelpKey {
    Close,
    Quit,
    Scroll(Motion, String),
    /// A key of a sequence (`g` of `g>g`).
    Pending,
    /// A bound key without a use in the overlay.
    Ignored,
    /// No binding: the key can go to the main menu.
    Unbound,
}

/// A key while the help overlay is open. The key goes through the user
/// keymap: the movement actions scroll the overlay, `cancel` and `help`
/// close it, and `quit` quits. The key `q` also closes it. The overlay
/// ignores the other actions. True when the key had a use.
fn help_key(event: KeyEvent, repeat: bool) -> bool {
    let what = with(|s| {
        let plain_q = event.base == Key::Char('q') && event.mods == Default::default();
        if plain_q && s.keys.pending().is_empty() {
            return HelpKey::Close;
        }
        let had_pending = !s.keys.pending().is_empty();
        match s.keys.feed(&s.config.keymap, &event) {
            KeyOutcome::Action(Action::Help | Action::Cancel) => HelpKey::Close,
            KeyOutcome::Action(Action::Quit) => HelpKey::Quit,
            KeyOutcome::Action(action) => match Motion::from_action(&action) {
                Some(motion) => HelpKey::Scroll(motion, action.to_string()),
                None => HelpKey::Ignored,
            },
            KeyOutcome::Pending => HelpKey::Pending,
            KeyOutcome::Unbound if had_pending => HelpKey::Pending,
            KeyOutcome::Unbound => HelpKey::Unbound,
        }
    });
    let Some(what) = what else {
        return false;
    };
    match what {
        HelpKey::Close => {
            if !repeat {
                dispatch(Action::Help);
            }
        }
        HelpKey::Quit => {
            if !repeat {
                dispatch(Action::Quit);
            }
        }
        HelpKey::Scroll(motion, label) => help_scroll(motion, &label),
        HelpKey::Pending => redraw_status(),
        HelpKey::Ignored => {}
        HelpKey::Unbound => return false,
    }
    true
}

/// Scrolls the help overlay with the smooth scroll rules. One step is one
/// line of the overlay. The line moves do not animate.
fn help_scroll(motion: Motion, label: &str) {
    let info = with_ref(|s| {
        (
            s.ui.help.clone(),
            s.style.row_height,
            s.config.smooth_scroll_time(),
        )
    });
    let Some((help, line, time)) = info else {
        return;
    };
    if line <= 0.0 {
        return;
    }
    let bounds = help.scroll.contentView().bounds();
    let content = help.view.frame().size.height;
    // During an animation, the next move starts from its target.
    let top = smooth::target_y(Target::Help).unwrap_or(bounds.origin.y);
    let new_top = viewport::scroll_view(
        top / line,
        content / line,
        bounds.size.height / line,
        motion,
    );
    let duration = time.filter(|_| motion.animates());
    smooth::scroll_help(&help.scroll, (new_top * line).round(), duration, label);
}

/// Work for AppKit that must run after the state borrow ends.
enum Effect {
    OpenFile(PathBuf),
    /// Open a file in `editor`, in a new window of the `terminal` app, when
    /// the file is a text file. Open other files in the default app.
    EditFile {
        path: PathBuf,
        editor: String,
        terminal: String,
    },
    Reveal(PathBuf),
    QuickLookToggle,
    QuickLookClose,
    OpenPrompt {
        place: PromptPlace,
        label: &'static str,
        text: String,
        select: (usize, usize),
    },
    ClosePrompt(PromptPlace),
    /// Show or hide the help overlay, as `AppState::help` says.
    Help,
    ToggleSidebar,
    /// Move the focus to the sidebar, or back to the list.
    FocusSidebar,
    /// Eject the volume at `mount` on a background thread.
    Eject {
        mount: PathBuf,
        name: String,
        force: bool,
    },
    /// Clear the text of the toolbar filter field.
    ClearSearch,
    /// Show the share menu for `path`, next to the list row `row`.
    Share {
        path: PathBuf,
        row: usize,
    },
    ReloadConfig,
    WatchConfig,
    Quit,
}

/// Does one action, then updates the views.
pub fn dispatch(action: Action) {
    dispatch_to(action, None);
}

/// Does one action on `target` instead of the entry under the cursor (the
/// background menu acts on the current folder). Only the actions that use
/// `current_or_cwd` see the target: `get_info`, `copy_path`, `copy_name`,
/// and `reveal_in_finder`.
pub fn dispatch_on(action: Action, target: PathBuf) {
    dispatch_to(action, Some(target));
}

fn dispatch_to(action: Action, target: Option<PathBuf>) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let ql_visible = quicklook::is_visible(mtm);
    trace!("action: {action}");
    let effects = with(|s| {
        s.target = target;
        let fx = s.perform(action, ql_visible);
        s.target = None;
        fx
    })
    .unwrap_or_default();
    run_effects(mtm, effects);
    sync_ui();
}

fn run_effects(mtm: MainThreadMarker, effects: Vec<Effect>) {
    for effect in effects {
        match effect {
            Effect::OpenFile(path) => open_file(&path),
            Effect::EditFile {
                path,
                editor,
                terminal,
            } => open_text_or_default(path, editor, terminal),
            Effect::Reveal(path) => {
                if let Some(url) = NSURL::from_file_path(&path) {
                    let urls = NSArray::from_retained_slice(&[url]);
                    NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&urls);
                }
            }
            Effect::QuickLookToggle => quicklook::toggle(mtm),
            Effect::QuickLookClose => quicklook::close(mtm),
            Effect::OpenPrompt {
                place,
                label,
                text,
                select,
            } => open_prompt_view(place, label, &text, select),
            Effect::ClosePrompt(place) => {
                let Some(ui) = with_ref(|s| s.ui.clone()) else {
                    continue;
                };
                ui.window.makeFirstResponder(Some(list_responder(&ui.list)));
                match place {
                    PromptPlace::Bar => ui.root.set_prompt_visible(false),
                    PromptPlace::Search => {}
                    PromptPlace::Inline => ui.prompt.inline.removeFromSuperview(),
                    PromptPlace::Panel => ui.root.set_goto(None),
                }
            }
            Effect::ToggleSidebar => {
                let Some(ui) = with_ref(|s| s.ui.clone()) else {
                    continue;
                };
                let collapse = !ui.sidebar_item.isCollapsed();
                if collapse
                    && ui.sidebar.outline.window().is_some_and(|w| {
                        w.firstResponder().is_some_and(|r| {
                            std::ptr::eq(
                                Retained::as_ptr(&r).cast::<u8>(),
                                Retained::as_ptr(&ui.sidebar.outline).cast::<u8>(),
                            )
                        })
                    })
                {
                    ui.window.makeFirstResponder(Some(list_responder(&ui.list)));
                }
                ui.sidebar_item.animator().setCollapsed(collapse);
                if !collapse {
                    rebuild_sidebar();
                }
            }
            Effect::FocusSidebar => focus_sidebar(),
            Effect::Eject { mount, name, force } => start_eject(mount, name, force),
            Effect::ClearSearch => {
                let field = with_ref(|s| s.ui.toolbar.search_field()).flatten();
                if let Some(field) = field {
                    field.setStringValue(&NSString::from_str(""));
                }
            }
            Effect::Share { path, row } => context_menu::share(&path, row),
            Effect::Help => {
                let Some((ui, height)) =
                    with_ref(|s| (s.ui.clone(), s.help.as_ref().map(|m| m.height(&s.style))))
                else {
                    continue;
                };
                show_help(&ui, height);
            }
            Effect::ReloadConfig => reload_config(),
            Effect::WatchConfig => watch_config_files(),
            Effect::Quit => NSApplication::sharedApplication(mtm).terminate(None),
        }
    }
}

/// Shows the help overlay with the content `height`, or hides it (`None`).
/// The overlay takes the keyboard focus while it is open.
fn show_help(ui: &Ui, height: Option<f64>) {
    let visible = height.is_some();
    let was_visible = !ui.help.scroll.isHidden();
    if visible != was_visible {
        smooth::finish();
    }
    // Read the focus before the overlay hides: AppKit gives the focus of a
    // hidden view to the window.
    let help_first = ui.window.firstResponder().is_some_and(|r| {
        std::ptr::eq(
            Retained::as_ptr(&r).cast::<u8>(),
            Retained::as_ptr(&ui.help.view).cast::<u8>(),
        )
    });
    ui.root.set_help(visible, height.unwrap_or(0.0));
    ui.help.view.setNeedsDisplay(true);
    let help_responder: &NSResponder = ui.help.view.as_super().as_super();
    if visible && !was_visible {
        // A new overlay starts at the top.
        let clip = ui.help.scroll.contentView();
        clip.scrollToPoint(objc2_foundation::NSPoint::new(0.0, 0.0));
        ui.help.scroll.reflectScrolledClipView(&clip);
    }
    if visible && !help_first {
        HELP_RETURN.with(|r| *r.borrow_mut() = ui.window.firstResponder());
        ui.window.makeFirstResponder(Some(help_responder));
    } else if !visible {
        let before = HELP_RETURN.with(|r| r.borrow_mut().take());
        if help_first {
            ui.window
                .makeFirstResponder(Some(focus_after_help(ui, before.as_deref())));
        }
    }
}

thread_local! {
    /// The responder that had the keyboard focus before the help overlay
    /// opened.
    static HELP_RETURN: RefCell<Option<Retained<NSResponder>>> = const { RefCell::new(None) };
}

/// The responder that gets the keyboard focus when the help overlay
/// closes: the sidebar when it had the focus before and it is visible,
/// else the list.
fn focus_after_help<'a>(ui: &'a Ui, before: Option<&NSResponder>) -> &'a NSResponder {
    let outline = &ui.sidebar.outline;
    let was_sidebar = before.is_some_and(|r| {
        std::ptr::eq(
            (r as *const NSResponder).cast::<u8>(),
            Retained::as_ptr(outline).cast::<u8>(),
        )
    });
    if was_sidebar && !outline.isHiddenOrHasHiddenAncestor() {
        trace!("help: the focus goes back to the sidebar");
        return outline.as_super().as_super().as_super().as_super();
    }
    trace!("help: the focus goes back to the list");
    list_responder(&ui.list)
}

/// Shows the text input of `place` and moves the keyboard focus to it.
fn open_prompt_view(place: PromptPlace, label: &str, text: &str, select: (usize, usize)) {
    let info = with_ref(|s| {
        let layout = s.style.layout(s.ui.list.bounds().size.width);
        let field_h = (s.style.line_height + 4.0).round();
        (
            s.ui.clone(),
            s.style.colors.name.clone(),
            s.pane.cursor(),
            layout,
            field_h,
        )
    });
    let Some((ui, caret, cursor, layout, field_h)) = info else {
        return;
    };
    match place {
        PromptPlace::Bar => {
            ui.root.set_prompt_visible(true);
            ui.prompt.set_label(label);
            prompt::focus_field(&ui.window, &ui.prompt.field, text, select, &caret);
        }
        PromptPlace::Search => {
            ui.toolbar.begin_search();
            if let Some(field) = ui.toolbar.search_field() {
                prompt::focus_field(&ui.window, &field, text, select, &caret);
            }
        }
        PromptPlace::Panel => {
            with(|s| s.goto.clear());
            ui.root.set_goto(Some(crate::goto_panel::height(0)));
            if let Some(field) = ui.goto.field() {
                prompt::focus_field(&ui.window, field, text, select, &caret);
            }
            goto_query(text.to_string());
        }
        PromptPlace::Inline => {
            ui.list.scroll_to_row(cursor);
            let frame = ui.list.name_field_frame(cursor, &layout, field_h);
            ui.prompt.inline.setFrame(frame);
            ui.list.addSubview(&ui.prompt.inline);
            prompt::focus_field(&ui.window, &ui.prompt.inline, text, select, &caret);
        }
    }
}

/// Moves the keyboard focus to the sidebar (it shows the sidebar when it
/// is hidden), or back to the list when the sidebar has it.
fn focus_sidebar() {
    let Some(ui) = with_ref(|s| s.ui.clone()) else {
        return;
    };
    let outline_first = ui.window.firstResponder().is_some_and(|r| {
        std::ptr::eq(
            Retained::as_ptr(&r).cast::<u8>(),
            Retained::as_ptr(&ui.sidebar.outline).cast::<u8>(),
        )
    });
    if outline_first {
        ui.window.makeFirstResponder(Some(list_responder(&ui.list)));
        return;
    }
    if ui.sidebar_item.isCollapsed() {
        ui.sidebar_item.setCollapsed(false);
        rebuild_sidebar();
    }
    let responder: &NSResponder = ui
        .sidebar
        .outline
        .as_super()
        .as_super()
        .as_super()
        .as_super();
    ui.window.makeFirstResponder(Some(responder));
}

/// True when the sidebar has the keyboard focus.
pub fn sidebar_focused() -> bool {
    with_ref(|s| sidebar_has_focus(&s.ui)).unwrap_or(false)
}

/// True when the sidebar is the first responder. It only reads AppKit
/// state, so it is safe during a borrow of the app state.
fn sidebar_has_focus(ui: &Ui) -> bool {
    ui.window.firstResponder().is_some_and(|r| {
        std::ptr::eq(
            Retained::as_ptr(&r).cast::<u8>(),
            Retained::as_ptr(&ui.sidebar.outline).cast::<u8>(),
        )
    })
}

/// Gives the keyboard focus back to the list.
pub fn focus_list() {
    if let Some(ui) = with_ref(|s| s.ui.clone()) {
        ui.window.makeFirstResponder(Some(list_responder(&ui.list)));
    }
}

/// A click on a sidebar row, or `enter` in the sidebar: go to `path` and
/// give the focus back to the list.
pub fn sidebar_open(path: PathBuf) {
    focus_list();
    dispatch(Action::Goto(path.to_string_lossy().into_owned()));
}

/// Selects the sidebar row of the current folder.
pub fn sync_sidebar_selection() {
    if let Some(cwd) = with_ref(|s| s.pane.cwd().to_path_buf()) {
        sidebar::select_path(&cwd);
    }
}

/// Builds the sidebar model again (volumes changed, or config reload).
pub fn rebuild_sidebar() {
    if let Some((items, locations)) =
        with_ref(|s| (s.config.sidebar_items.clone(), s.config.sidebar_locations))
    {
        // `spawn_build` applies the model on the main queue when it is done.
        let _pending = sidebar::spawn_build(items, locations);
    }
}

/// A click on a column title: the sort column reverses the order, another
/// column sorts by it in the normal direction.
pub fn header_click(target: header::Target) {
    let Some(key) = target.sort_key() else {
        return;
    };
    let changed = with(|s| {
        let mut sort = s.pane.sort();
        if sort.key == key {
            sort.reverse = !sort.reverse;
        } else {
            sort.key = key;
            sort.reverse = false;
        }
        s.pane.set_sort(sort);
        s.content_dirty = true;
        s.reveal_cursor = true;
    });
    if changed.is_some() {
        sync_ui();
    }
}

/// Keeps the cursor row in view (for example after the toolbar changed).
pub fn reveal_cursor() {
    if let Some((list, cursor)) = with_ref(|s| (s.ui.list.clone(), s.pane.cursor())) {
        list.scroll_to_row(cursor);
    }
}

/// A scroll cancels the inline rename.
pub fn cancel_inline_rename() {
    let inline = with_ref(|s| {
        s.prompt_place == PromptPlace::Inline && matches!(s.prompt, Some(PromptMode::Rename { .. }))
    });
    if inline == Some(true) {
        prompt_cancel();
    }
}

/// Redraws the list, the header, and the bars.
pub fn redraw_all() {
    if let Some(ui) = with_ref(|s| s.ui.clone()) {
        ui.list.setNeedsDisplay(true);
        ui.header.setNeedsDisplay(true);
        ui.status.setNeedsDisplay(true);
        ui.prompt.bar.setNeedsDisplay(true);
    }
}

fn open_file(path: &Path) {
    let opened =
        NSURL::from_file_path(path).is_some_and(|url| NSWorkspace::sharedWorkspace().openURL(&url));
    if !opened {
        show_error(format!("Cannot open \"{}\"", file_name(path)));
    }
}

/// Opens `path` in `editor` when it is a text file, else in its default app.
/// The text check reads the start of the file. That read can wait for the
/// network or for an iCloud download, so it runs on a background thread. The
/// result goes back to the main queue.
fn open_text_or_default(path: PathBuf, editor: String, terminal: String) {
    std::thread::spawn(move || {
        let is_text = bf_core::fs::is_text_file(&path);
        after_now(move || {
            if is_text {
                open_in_editor(&path, &editor, &terminal);
            } else {
                open_file(&path);
            }
        });
    });
}

/// Opens `path` in `editor`, in a new window of the `terminal` app.
fn open_in_editor(path: &Path, editor: &str, terminal: &str) {
    let Some(args) = editor::osascript_command(path, editor, terminal) else {
        show_error("The editor setting needs terminal = Ghostty or Terminal".to_string());
        return;
    };
    let failure = format!("Cannot open \"{}\" in {}", file_name(path), editor.trim());
    if let Err(err) = spawn_checked("/usr/bin/osascript", &args, failure) {
        show_error(format!("Cannot run osascript: {err}"));
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
        self.push_message(text, error, false);
    }

    fn push_message(&mut self, text: String, error: bool, sticky: bool) {
        trace!("message: {text}");
        self.message_id += 1;
        let id = self.message_id;
        self.message = Some(Message {
            text,
            error,
            born: Instant::now(),
            id,
            sticky,
        });
        if !error && !sticky {
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

    /// A progress message: it stays until the next message.
    fn set_progress(&mut self, text: impl Into<String>) {
        self.push_message(text.into(), false, true);
    }

    /// A page action with the vim rules (`bf_core::viewport`). The pane
    /// cursor changes at once. `sync_ui` then moves the view and the
    /// highlight to the target, with an animation when it is on.
    fn page_motion(&mut self, action: &Action) {
        let Some(motion) = Motion::from_action(action) else {
            return;
        };
        let Some(view) = self.ui.list.view_rows() else {
            return;
        };
        let h = view.row_height;
        // During an animation, the next move starts from its target.
        let top = smooth::target_y(Target::List).map_or(view.top, |y| (y + view.inset) / h);
        let viewport = Viewport {
            len: self.pane.visible_len(),
            height: view.height,
        };
        let from = self.pane.cursor();
        let to = viewport.apply(Position { cursor: from, top }, motion);
        self.pane.set_cursor(to.cursor);
        self.reveal_cursor = false;
        self.scroll_plan = Some(ListScroll {
            to_y: to.top * h - view.inset,
            from_row: from,
            to_row: to.cursor,
            row_height: h,
            inset: view.inset,
            label: action.to_string(),
        });
    }

    /// True when the filter prompt has a path (the location field).
    fn location_active(&self) -> bool {
        self.path_mode && self.prompt == Some(PromptMode::Filter)
    }

    /// True when a prompt takes a path: the "go to" panel or the location field.
    fn goto_active(&self) -> bool {
        self.prompt == Some(PromptMode::Goto) || self.location_active()
    }

    fn start_load(&mut self, request: LoadRequest) {
        let generation = request.generation;
        self.load_generation = generation;
        self.load_kind = request.kind;
        self.load_auto = false;
        self.load_started = Some(Instant::now());
        loader::spawn(request, self.pane.sort());
        after(Duration::from_millis(100), move || {
            loading_check(generation)
        });
    }

    /// The matches of the "go to" panel and the selected one.
    pub fn goto_matches(&self) -> (&[String], Option<usize>) {
        (&self.goto.matches, self.goto.selected)
    }

    /// True when the current directory is on a local volume.
    pub fn cwd_is_local(&self) -> bool {
        self.cwd_is_local
    }

    /// True while the pane loads another directory. A refresh does not count.
    fn navigating(&self) -> bool {
        self.pane.is_loading() && self.load_kind != LoadKind::Refresh
    }

    /// Refuses a file operation while the pane loads another directory: the
    /// entry under the cursor can belong to the directory that goes away.
    fn refuse_while_navigating(&mut self) -> bool {
        if self.navigating() {
            self.set_error("Wait until the folder loads");
        }
        self.navigating()
    }

    /// Reads the directory again after a change. When a load is in
    /// progress, it queues one refresh for after that load.
    fn refresh_after_change(&mut self) {
        if self.pane.is_loading() {
            self.refresh_queued = true;
        } else {
            let request = self.pane.refresh();
            self.start_load(request);
        }
    }

    fn open_prompt(
        &mut self,
        mode: PromptMode,
        text: String,
        select: (usize, usize),
        ql_visible: bool,
        fx: &mut Vec<Effect>,
    ) {
        let label = mode.label();
        let place = match mode {
            PromptMode::Filter if self.config.toolbar => PromptPlace::Search,
            PromptMode::Rename { .. } => PromptPlace::Inline,
            PromptMode::Goto => PromptPlace::Panel,
            _ => PromptPlace::Bar,
        };
        // A prompt in another place closes first.
        if self.prompt.is_some() && self.prompt_place != place {
            fx.push(Effect::ClosePrompt(self.prompt_place));
        }
        self.prompt = Some(mode);
        self.prompt_place = place;
        self.path_mode = false;
        // The Quick Look panel sends keys to the keymap. Close it, so that
        // the typed text goes only to the prompt.
        if ql_visible {
            fx.push(Effect::QuickLookClose);
        }
        if self.help.take().is_some() {
            fx.push(Effect::Help);
        }
        fx.push(Effect::OpenPrompt {
            place,
            label,
            text,
            select,
        });
    }

    fn close_prompt(&mut self, fx: &mut Vec<Effect>) {
        self.prompt = None;
        self.path_mode = false;
        fx.push(Effect::ClosePrompt(self.prompt_place));
    }

    /// Makes a new folder at once (`untitled folder`, then `untitled
    /// folder 2`, ...), selects it, and opens the inline rename on it after
    /// the load, like Finder.
    fn new_folder(&mut self) {
        if self.refuse_while_navigating() {
            return;
        }
        let dir = self.pane.cwd().to_path_buf();
        let mut created = None;
        for n in 1..1000 {
            let name = if n == 1 {
                "untitled folder".to_string()
            } else {
                format!("untitled folder {n}")
            };
            // `create_dir` fails when the name exists: it never replaces.
            match std::fs::create_dir(dir.join(&name)) {
                Ok(()) => {
                    created = Some(name);
                    break;
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => {
                    self.set_error(format!("Cannot create a folder: {err}"));
                    return;
                }
            }
        }
        let Some(name) = created else {
            self.set_error("Cannot find a free name for the new folder");
            return;
        };
        self.pending_rename = Some(dir.join(&name));
        self.pending_select = Some((dir, name));
        self.refresh_after_change();
    }

    /// The effect that opens a file. With `editor` set, a text file opens in the editor.
    /// Every other file opens in the default app. This function does no disk IO:
    /// the text check runs later, on a background thread.
    fn open_file_effect(&self, path: PathBuf) -> Effect {
        let editor = &self.config.editor;
        if !editor.trim().is_empty() {
            Effect::EditFile {
                path,
                editor: editor.clone(),
                terminal: self.config.terminal.clone(),
            }
        } else {
            Effect::OpenFile(path)
        }
    }

    /// The target of an action: the target of `dispatch_on`, else the entry
    /// under the cursor, else the current folder.
    fn current_or_cwd(&self) -> PathBuf {
        if let Some(target) = &self.target {
            return target.clone();
        }
        self.pane
            .current()
            .map(|e| e.path.clone())
            .unwrap_or_else(|| self.pane.cwd().to_path_buf())
    }

    /// `copy`: puts the entry under the cursor on the general pasteboard as
    /// a file URL. A paste in Finder, Mail, or a chat app gets the file.
    fn copy_file(&mut self) {
        let Some(entry) = self.pane.current() else {
            return;
        };
        let (name, path) = (entry.name.clone(), entry.path.clone());
        if copy_files_to_pasteboard(&[path]) {
            self.set_info(format!("Copied \"{name}\""));
        } else {
            self.set_error("Cannot copy to the pasteboard");
        }
    }

    /// `paste`: copies the files on the general pasteboard into the current
    /// folder, on a background thread. It never replaces an item.
    fn paste(&mut self) {
        if self.refuse_while_navigating() {
            return;
        }
        let files = pasteboard_files();
        if files.is_empty() {
            self.set_info("Nothing to paste");
            return;
        }
        file_ops::transfer(files, self.pane.cwd().to_path_buf(), TransferMode::Copy);
    }

    /// `duplicate` (`Copy`) and `make_alias` (`Alias`): a copy or an alias
    /// of the entry under the cursor, in its own folder.
    fn transfer_current(&mut self, mode: TransferMode) {
        if self.refuse_while_navigating() {
            return;
        }
        let Some(path) = self.pane.current().map(|e| e.path.clone()) else {
            return;
        };
        if let Some(dir) = path.parent().map(Path::to_path_buf) {
            file_ops::transfer(vec![path], dir, mode);
        }
    }

    /// `compress`: a zip archive of the entry under the cursor, next to it.
    /// `ditto` runs on a background thread.
    fn compress(&mut self) {
        if self.refuse_while_navigating() {
            return;
        }
        let Some(entry) = self.pane.current() else {
            return;
        };
        let (name, path) = (entry.name.clone(), entry.path.clone());
        let Some(dir) = path.parent().map(Path::to_path_buf) else {
            return;
        };
        let progress = format!("Compressing \"{name}\"\u{2026}");
        let spawned = in_background(
            "bf-compress",
            move || file_ops::compress(&path),
            move |result| {
                let message = match &result {
                    Ok(new) => (format!("Compressed \"{name}\" to \"{new}\""), false),
                    Err(err) => (format!("Cannot compress \"{name}\": {err}"), true),
                };
                let new = result.ok();
                new_items_finished(vec![dir.clone()], &dir, new, Some(message));
            },
        );
        match spawned {
            Ok(()) => self.set_progress(progress),
            Err(err) => self.set_error(format!("Cannot start a thread: {err}")),
        }
    }

    /// `get_info`: opens the Finder Info window of the target through
    /// `osascript`, on a background thread. The first time, macOS asks the
    /// user to allow Better Finder to control Finder.
    fn get_info(&mut self) {
        let path = self.current_or_cwd();
        let name = file_name(&path);
        let args = file_ops::info_window_args(&path);
        let spawned = in_background(
            "bf-info",
            move || {
                Command::new("/usr/bin/osascript")
                    .args(&args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .output()
            },
            move |output| {
                let detail = match output {
                    Ok(output) if output.status.success() => return,
                    Ok(output) => String::from_utf8_lossy(&output.stderr).trim().to_string(),
                    Err(err) => err.to_string(),
                };
                show_error(info_error(&name, &detail));
            },
        );
        if let Err(err) = spawned {
            self.set_error(format!("Cannot start a thread: {err}"));
        }
    }

    fn perform(&mut self, action: Action, ql_visible: bool) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.reveal_cursor = true;
        if self.message.as_ref().is_some_and(|m| m.error) {
            self.message = None;
        }
        match action {
            Action::CursorUp => self.pane.move_cursor(-1),
            Action::CursorDown => self.pane.move_cursor(1),
            Action::CursorTop
            | Action::CursorBottom
            | Action::PageUp
            | Action::PageDown
            | Action::HalfPageUp
            | Action::HalfPageDown => self.page_motion(&action),
            Action::Open => match self.pane.open_current() {
                OpenOutcome::Navigate(request) => self.start_load(request),
                OpenOutcome::OpenFile(path) => fx.push(self.open_file_effect(path)),
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
                self.open_prompt(PromptMode::Goto, String::new(), (0, 0), ql_visible, &mut fx)
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
                self.open_prompt(PromptMode::Filter, text, select, ql_visible, &mut fx);
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
                if self.refuse_while_navigating() {
                    return fx;
                }
                if let Some(entry) = self.pane.current() {
                    let name = entry.name.clone();
                    let select = (0, stem_utf16_len(entry));
                    // Keep the full path: the rename must not depend on
                    // the directory at the time of Enter.
                    let mode = PromptMode::Rename {
                        source: entry.path.clone(),
                    };
                    self.open_prompt(mode, name, select, ql_visible, &mut fx);
                }
            }
            Action::NewFolder => self.new_folder(),
            Action::ToggleSidebar => fx.push(Effect::ToggleSidebar),
            Action::FocusSidebar => fx.push(Effect::FocusSidebar),
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
            Action::Copy => self.copy_file(),
            Action::Paste => self.paste(),
            Action::Duplicate => self.transfer_current(TransferMode::Copy),
            Action::MakeAlias => self.transfer_current(TransferMode::Alias),
            Action::Compress => self.compress(),
            Action::GetInfo => self.get_info(),
            Action::Share => {
                if let Some(entry) = self.pane.current() {
                    fx.push(Effect::Share {
                        path: entry.path.clone(),
                        row: self.pane.cursor(),
                    });
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
            Action::Eject => self.eject(None, ql_visible, &mut fx),
            Action::OpenConfig => self.open_config(&mut fx),
            Action::ReloadConfig => fx.push(Effect::ReloadConfig),
            Action::Quit => fx.push(Effect::Quit),
            Action::Unbind => {}
        }
        fx
    }

    /// True when the current folder is on the volume at `mount`.
    fn cwd_on(&self, mount: &Path) -> bool {
        mount != Path::new("/")
            && (self.pane.cwd().starts_with(mount)
                || self.volume.as_ref().is_some_and(|v| v.mount == mount))
    }

    /// The volume with the mount point `path`, when it can eject.
    fn ejectable_volume(&self, path: &Path) -> Option<Volume> {
        self.volumes
            .iter()
            .find(|v| v.path == path && v.ejectable())
            .cloned()
    }

    /// The volume for `eject`, in the order of docs/UI.md:
    /// 1. With the focus in the sidebar (`sidebar` is `Some`): the selected row.
    /// 2. The entry under the cursor, when it is the mount point of a volume.
    /// 3. The volume of the current folder.
    ///
    /// The boot volume never ejects.
    fn eject_target(&self, sidebar: Option<Option<PathBuf>>) -> Option<Volume> {
        if let Some(row) = sidebar {
            return row.and_then(|path| self.ejectable_volume(&path));
        }
        let under_cursor = self
            .pane
            .current()
            .and_then(|e| self.ejectable_volume(&e.path));
        if under_cursor.is_some() {
            return under_cursor;
        }
        // The mount point from `statfs` (the loader thread), else the
        // longest mount point that contains the current folder.
        let cwd = self.pane.cwd();
        let mount = self.volume.as_ref().map(|v| v.mount.clone()).or_else(|| {
            self.volumes
                .iter()
                .filter(|v| cwd.starts_with(&v.path))
                .max_by_key(|v| v.path.components().count())
                .map(|v| v.path.clone())
        })?;
        self.ejectable_volume(&mount)
    }

    /// The `eject` action (`button` is `None`), or a click on the eject
    /// button of the sidebar row `button`. See docs/UI.md, "The eject action".
    fn eject(&mut self, button: Option<PathBuf>, ql_visible: bool, fx: &mut Vec<Effect>) {
        if let Some((_, name)) = &self.eject.running {
            let text = ejecting_text(name);
            self.set_progress(text);
            return;
        }
        let sidebar = match button {
            Some(path) => Some(Some(path)),
            None => sidebar_has_focus(&self.ui).then(sidebar::selected_path),
        };
        // The entry under the cursor can belong to the folder that goes away.
        if sidebar.is_none() && self.refuse_while_navigating() {
            return;
        }
        let target = self.eject_target(sidebar);
        let busy = self
            .eject
            .busy
            .take()
            .filter(|(_, at)| at.elapsed() < FORCE_WINDOW)
            .map(|(mount, _)| mount);
        // In the busy time, `eject` forces the busy volume. A target that
        // is another volume ejects normally.
        let (volume, force) = match (target, busy) {
            (Some(volume), Some(mount)) => {
                let force = volume.path == mount;
                (volume, force)
            }
            (None, Some(mount)) => match self.ejectable_volume(&mount) {
                Some(volume) => (volume, true),
                None => {
                    self.set_info("Nothing to eject here");
                    return;
                }
            },
            (Some(volume), None) => (volume, false),
            (None, None) => {
                self.set_info("Nothing to eject here");
                return;
            }
        };
        trace!("eject: {} (force {force})", volume.path.display());
        // Better Finder must not keep the volume busy.
        self.leave_volume(&volume.path, ql_visible, fx);
        self.set_progress(ejecting_text(&volume.name));
        self.eject.running = Some((volume.path.clone(), volume.name.clone()));
        fx.push(Effect::Eject {
            mount: volume.path,
            name: volume.name,
            force,
        });
    }

    /// Leaves the volume at `mount`, before an eject or after an unmount:
    /// - Quick Look closes when it shows a file on the volume.
    /// - A rename or new-folder prompt on the volume cancels.
    /// - When the current folder is on the volume, the pane goes to the
    ///   home folder. The watcher then watches the home folder.
    ///
    /// Returns true when the pane goes to the home folder.
    fn leave_volume(&mut self, mount: &Path, ql_visible: bool, fx: &mut Vec<Effect>) -> bool {
        let on_volume = self.cwd_on(mount);
        let shows_volume = on_volume
            || self
                .pane
                .current()
                .is_some_and(|e| e.path.starts_with(mount));
        if ql_visible && shows_volume {
            fx.push(Effect::QuickLookClose);
        }
        if matches!(&self.prompt, Some(PromptMode::Rename { source }) if source.starts_with(mount))
        {
            self.close_prompt(fx);
        }
        if self
            .pending_rename
            .as_ref()
            .is_some_and(|p| p.starts_with(mount))
        {
            self.pending_rename = None;
        }
        if self
            .pending_select
            .as_ref()
            .is_some_and(|(dir, _)| dir.starts_with(mount))
        {
            self.pending_select = None;
        }
        if on_volume {
            let request = self.pane.navigate_to(home_dir());
            self.start_load(request);
        }
        on_volume
    }

    fn trash(&mut self) {
        if self.refuse_while_navigating() {
            return;
        }
        // An automatic refresh can put another name under the cursor just
        // before the key press. Ask for a second press.
        if self
            .list_changed_at
            .take()
            .is_some_and(|t| t.elapsed() < Duration::from_millis(500))
        {
            self.set_error("The list changed. Press again to move the item to the Trash.");
            return;
        }
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
                self.refresh_after_change();
            }
            Err(err) => {
                let reason = err.localizedDescription();
                self.set_error(format!("Cannot move \"{name}\" to Trash: {reason}"));
            }
        }
    }

    fn open_config(&mut self, fx: &mut Vec<Effect>) {
        let path = Config::default_path();
        match file_ops::create_new_file(&path, default_config_template().as_bytes()) {
            Ok(true) => {
                self.set_info(format!("Created {}", display_path(&path)));
                fx.push(Effect::WatchConfig);
            }
            Ok(false) => {}
            Err(err) => {
                self.set_error(format!("Cannot create {}: {err}", display_path(&path)));
                return;
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
        match mode {
            PromptMode::Filter if self.path_mode => self.accept_path(text, &mut fx),
            PromptMode::Filter => self.close_prompt(&mut fx),
            PromptMode::Goto => self.accept_path(text, &mut fx),
            PromptMode::Rename { source } => {
                let old = source.file_name().unwrap_or(OsStr::new(""));
                let old_text = old.to_string_lossy().into_owned();
                if old == OsStr::new(&text) || old_text == text {
                    self.close_prompt(&mut fx);
                    return fx;
                }
                if let Err(problem) = check_name(&text) {
                    self.set_error(problem);
                    return fx;
                }
                if self.refuse_while_navigating() {
                    return fx;
                }
                let target = source.with_file_name(&text);
                match file_ops::rename_no_replace(&source, &target) {
                    Ok(()) => {
                        self.close_prompt(&mut fx);
                        if let Some(dir) = source.parent() {
                            self.pending_select = Some((dir.to_path_buf(), text));
                        }
                        self.refresh_after_change();
                    }
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                        self.set_error(format!("\"{text}\" already exists"));
                    }
                    Err(err) => self.set_error(format!("Cannot rename \"{old_text}\": {err}")),
                }
            }
        }
        fx
    }

    /// `Enter` in the "go to" panel or in the location field. The selected
    /// match wins over the text. The path check runs on a thread, because a
    /// `stat` can wait for a long time on a network volume; `goto_checked`
    /// continues.
    fn accept_path(&mut self, text: String, fx: &mut Vec<Effect>) {
        let selected = self
            .goto
            .selected
            .and_then(|i| self.goto.matches.get(i))
            .map(|name| format!("{}{name}", self.goto.dir_text));
        let text = selected.unwrap_or(text);
        let input = text.trim();
        if input.is_empty() {
            self.close_prompt(fx);
            return;
        }
        let path = expand_path(input, self.pane.cwd());
        self.goto_check += 1;
        let id = self.goto_check;
        let spawned = std::thread::Builder::new()
            .name("bf-goto".into())
            .spawn(move || {
                let is_dir = std::fs::metadata(&path).map(|m| m.is_dir());
                after_now(move || goto_checked(id, path, is_dir));
            });
        if let Err(err) = spawned {
            self.set_error(format!("Cannot start a thread: {err}"));
        }
    }

    /// Closes the "go to" panel or the location field after a successful
    /// check. The location field clears, and the list gets the focus.
    fn close_path_prompt(&mut self, fx: &mut Vec<Effect>) {
        if self.location_active() && self.prompt_place == PromptPlace::Search {
            fx.push(Effect::ClearSearch);
        }
        self.close_prompt(fx);
    }

    /// The result of the "go to" check: a directory opens, a file opens its
    /// directory with the cursor on the file. A missing path shows an error,
    /// and the prompt stays open.
    fn goto_result(&mut self, path: PathBuf, is_dir: io::Result<bool>) -> Vec<Effect> {
        let mut fx = Vec::new();
        match is_dir {
            Ok(true) => {
                self.close_path_prompt(&mut fx);
                let request = self.pane.navigate_to(path);
                self.start_load(request);
            }
            Ok(false) => {
                let parent = path.parent().map(Path::to_path_buf);
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                if let (Some(parent), Some(name)) = (parent, name) {
                    self.close_path_prompt(&mut fx);
                    self.pending_select = Some((parent.clone(), name));
                    let request = self.pane.navigate_to(parent);
                    self.start_load(request);
                }
            }
            Err(_) => self.set_error("No such folder"),
        }
        fx
    }
}

/// The background check of the "go to" prompt is complete.
fn goto_checked(id: u64, path: PathBuf, is_dir: io::Result<bool>) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let effects = with(|s| {
        if s.goto_check != id || !s.goto_active() {
            return Vec::new();
        }
        s.goto_result(path, is_dir)
    })
    .unwrap_or_default();
    run_effects(mtm, effects);
    sync_ui();
}

/// Checks a name for rename and new folder. The file operation itself
/// refuses a name that exists.
fn check_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("The name cannot be empty".to_string());
    }
    if name.contains('/') {
        return Err("The name cannot contain \"/\"".to_string());
    }
    if name == "." || name == ".." {
        return Err(format!("The name \"{name}\" is reserved"));
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

/// Puts `paths` on the general pasteboard as file URLs, like a copy in Finder.
fn copy_files_to_pasteboard(paths: &[PathBuf]) -> bool {
    let urls: Vec<Retained<NSURL>> = paths.iter().filter_map(NSURL::from_file_path).collect();
    if urls.is_empty() {
        return false;
    }
    let objects: Vec<&ProtocolObject<dyn NSPasteboardWriting>> = urls
        .iter()
        .map(|url| ProtocolObject::from_ref(&**url))
        .collect();
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    pasteboard.writeObjects(&NSArray::from_slice(&objects))
}

/// The file paths on the general pasteboard (only file URLs, for example
/// after a copy in Finder).
pub fn pasteboard_files() -> Vec<PathBuf> {
    let pasteboard = NSPasteboard::generalPasteboard();
    let classes = NSArray::from_slice(&[NSURL::class()]);
    // SAFETY: The option key is an immutable static from AppKit.
    let key = unsafe { NSPasteboardURLReadingFileURLsOnlyKey };
    let yes = NSNumber::new_bool(true);
    let value: &AnyObject = &yes;
    let options = NSDictionary::from_slices(&[key], &[value]);
    // SAFETY: The classes array holds `NSURL`, which conforms to
    // `NSPasteboardReading`. The option has the documented `NSNumber` type.
    let objects = unsafe { pasteboard.readObjectsForClasses_options(&classes, Some(&options)) };
    objects
        .map(|objects| {
            objects
                .iter()
                .filter_map(|object| object.downcast::<NSURL>().ok())
                .filter_map(|url| url.to_file_path())
                .collect()
        })
        .unwrap_or_default()
}

/// The status text for a failed `get_info`. When macOS refuses the Apple
/// Event, the text tells the user where to allow it.
fn info_error(name: &str, detail: &str) -> String {
    let refused = ["-1743", "-1744", "Not authorized", "not allowed"]
        .iter()
        .any(|code| detail.contains(code));
    if refused {
        "Allow Better Finder to control Finder: System Settings > Privacy & Security > Automation"
            .to_string()
    } else {
        let detail = detail.lines().next().unwrap_or("").trim();
        format!("Cannot show the info of \"{name}\": {detail}")
    }
}

/// Runs `/usr/bin/open` with `args` and does not wait. A thread waits for
/// the exit and shows `failure` when `open` fails.
fn spawn_open(args: &[OsString], failure: String) -> io::Result<()> {
    spawn_checked("/usr/bin/open", args, failure)
}

/// Runs `program` with `args` and does not wait. A thread waits for the exit.
/// When the program fails, the thread shows `failure` and the first line of stderr.
fn spawn_checked(program: &str, args: &[OsString], failure: String) -> io::Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    std::thread::spawn(move || {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            // A read error leaves the text empty. The exit status still decides.
            let _ = pipe.read_to_string(&mut stderr);
        }
        if child.wait().is_ok_and(|status| !status.success()) {
            let detail = stderr.lines().next().unwrap_or("").trim();
            let text = if detail.is_empty() {
                failure
            } else {
                format!("{failure}: {detail}")
            };
            after_now(move || show_error(text));
        }
    });
    Ok(())
}

pub fn show_error(text: String) {
    if with(|s| s.set_error(text)).is_some() {
        redraw_status();
    }
}

/// Shows a progress message. The result of the task replaces it.
pub fn show_progress(text: String) {
    if with(|s| s.set_progress(text)).is_some() {
        redraw_status();
    }
}

/// A `file_ops::transfer` is complete: refresh, select, and one message.
pub fn transfer_finished(report: TransferReport) {
    let first_new = report.results.iter().find_map(|r| match r {
        bf_core::transfer::ItemResult::Done { new, .. } => Some(new.clone()),
        _ => None,
    });
    let message = bf_core::transfer::summary(&report.results);
    new_items_finished(report.changed, &report.dest_dir, first_new, message);
}

/// A background file operation is complete. Shows `message`. When the
/// current folder is in `changed`, it reads the folder again, and the
/// cursor goes to `new_name` in `dir` when the list shows it.
fn new_items_finished(
    changed: Vec<PathBuf>,
    dir: &Path,
    new_name: Option<String>,
    message: Option<(String, bool)>,
) {
    let done = with(|s| {
        if let Some((text, error)) = message {
            s.set_message(text, error);
        }
        let cwd = s.pane.cwd().to_path_buf();
        if !changed.contains(&cwd) {
            return;
        }
        if let Some(name) = new_name.filter(|_| dir == cwd) {
            s.pending_select = Some((cwd, name));
        }
        s.refresh_after_change();
    });
    if done.is_some() {
        sync_ui();
    }
}

/// Puts the cursor on the entry at `path`. The context menu calls it before
/// an action on its row: a refresh can change the rows while the menu is
/// open. False when the list does not show `path`.
pub fn cursor_to(path: &Path) -> bool {
    let moved = with(|s| {
        if s.pane.current().is_some_and(|e| e.path == path) {
            return Some(false);
        }
        let row = (0..s.pane.visible_len())
            .find(|&i| s.pane.visible_entry(i).is_some_and(|e| e.path == path))?;
        s.pane.set_cursor(row);
        Some(true)
    })
    .flatten();
    if moved == Some(true) {
        sync_ui();
    }
    moved.is_some()
}

/// True when the list shows the folder `dir`.
pub fn shows_folder(dir: &Path) -> bool {
    with_ref(|s| s.pane.cwd() == dir).unwrap_or(false)
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
pub fn finish_load(delivery: Delivery) {
    let Delivery {
        request,
        result,
        read_time,
        is_local,
        volume,
    } = delivery;
    let started = Instant::now();
    let count = result.as_ref().map(Vec::len).unwrap_or(0);
    let outcome = with(|s| {
        let was_loaded = s.pane.is_loaded();
        let old_cwd = s.pane.cwd().to_path_buf();
        let old_name = s.pane.current().map(|e| e.name.clone());
        if !s.pane.finish_load(&request, result) {
            return None;
        }
        let was_auto = std::mem::take(&mut s.load_auto);
        s.loading_visible = false;
        let total = s.load_started.take().map(|t| t.elapsed());
        let failed = s.pane.error().map(str::to_string);
        if let Some(error) = failed.clone() {
            s.pane.clear_error();
            s.set_error(error);
        } else {
            s.cwd_is_local = is_local;
            s.volume = volume;
        }
        let cwd = s.pane.cwd().to_path_buf();
        let dir_changed = (!was_loaded && s.pane.is_loaded()) || cwd != old_cwd;
        if let Some((dir, name)) = s.pending_select.take() {
            let selected = dir == cwd && failed.is_none() && s.pane.select_name(&name);
            // A read that started before the change does not have the new
            // name yet. Keep the selection for the queued refresh.
            if !selected && s.refresh_queued && dir == cwd {
                s.pending_select = Some((dir, name));
            }
        }
        if was_auto && !dir_changed && s.pane.current().map(|e| &e.name) != old_name.as_ref() {
            s.list_changed_at = Some(Instant::now());
        }
        let mut fx = Vec::new();
        // A rename applies to the directory where the prompt opened. When
        // the directory changed, cancel the prompt.
        if dir_changed && matches!(s.prompt, Some(PromptMode::Rename { .. })) {
            s.close_prompt(&mut fx);
            s.set_info("The folder changed. The prompt closed.");
        }
        // A new folder: open the inline rename when its row is under the cursor.
        if let Some(path) = s.pending_rename.take() {
            let ready = failed.is_none()
                && s.pane.current().is_some_and(|e| e.path == path)
                && s.prompt.is_none();
            if ready {
                if let Some(entry) = s.pane.current() {
                    let name = entry.name.clone();
                    let select = (0, utf16_len(&name));
                    let mode = PromptMode::Rename { source: path };
                    s.open_prompt(mode, name, select, false, &mut fx);
                }
            } else if s.refresh_queued && path.parent() == Some(cwd.as_path()) {
                s.pending_rename = Some(path);
            }
        }
        if failed.is_none() {
            // The load left an unmounted volume: forget its folders now.
            for mount in std::mem::take(&mut s.forget_after_load) {
                s.pane.forget_history_under(&mount);
            }
        }
        if s.refresh_queued && !s.pane.is_loading() {
            s.refresh_queued = false;
            let auto = std::mem::take(&mut s.refresh_queued_auto);
            let request = s.pane.refresh();
            s.start_load(request);
            s.load_auto = auto;
        }
        s.content_dirty = true;
        Some((cwd, dir_changed, total, fx))
    });
    let Some(Some((cwd, dir_changed, total, effects))) = outcome else {
        return;
    };
    watcher::set_last_read(read_time);
    if dir_changed {
        icons::clear_paths();
        watcher::watch_directory(&cwd);
    }
    if let Some(mtm) = MainThreadMarker::new() {
        run_effects(mtm, effects);
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

/// The watcher calls this (debounced) when the current directory changes
/// on disk. A load in progress is not replaced: one refresh waits for it.
pub fn auto_refresh() {
    with(|s| {
        if !s.config.auto_refresh || !s.pane.is_loaded() {
            return;
        }
        if s.pane.is_loading() {
            s.refresh_queued = true;
            s.refresh_queued_auto = true;
            return;
        }
        let request = s.pane.refresh();
        s.start_load(request);
        s.load_auto = true;
    });
}

// ---- Volumes and eject ----

fn ejecting_text(name: &str) -> String {
    format!("Ejecting \"{name}\"\u{2026}")
}

/// The key text of `eject` in the keymap, for the busy message: the
/// shortest trigger, for example `cmd+e`.
fn eject_key(keymap: &Keymap) -> Option<String> {
    keymap
        .bindings()
        .into_iter()
        .filter(|(_, action)| *action == Action::Eject)
        .min_by_key(|(trigger, _)| trigger.0.len())
        .map(|(trigger, _)| trigger.to_string())
}

/// A click on the eject button of a sidebar row: eject the volume `path`.
pub fn eject_volume(path: PathBuf) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let ql_visible = quicklook::is_visible(mtm);
    trace!("action: eject (button) {}", path.display());
    let effects = with(|s| {
        if s.message.as_ref().is_some_and(|m| m.error) {
            s.message = None;
        }
        let mut fx = Vec::new();
        s.eject(Some(path), ql_visible, &mut fx);
        fx
    })
    .unwrap_or_default();
    run_effects(mtm, effects);
    sync_ui();
}

/// Starts the eject on a background thread. The watcher first stops its
/// stream when the stream is on the volume. The result goes to
/// `eject_finished` on the main queue.
fn start_eject(mount: PathBuf, name: String, force: bool) {
    let thread_mount = mount.clone();
    watcher::release_volume(mount.clone(), move || {
        volumes::spawn_eject(thread_mount, force, move |outcome| {
            after_now(move || eject_finished(mount, name, outcome));
        });
    });
}

/// The result of an eject: `Ejected "X"`, the busy message, or the reason
/// of the failure.
fn eject_finished(mount: PathBuf, name: String, outcome: EjectOutcome) {
    let ejected = outcome == EjectOutcome::Ejected;
    let busy_message = with(|s| {
        if s.eject.running.as_ref().is_some_and(|(m, _)| *m == mount) {
            s.eject.running = None;
        }
        match outcome {
            EjectOutcome::Ejected => {
                s.eject.ejected = Some(mount.clone());
                s.set_info(format!("Ejected \"{name}\""));
                None
            }
            EjectOutcome::Busy(apps) => {
                let key = eject_key(&s.config.keymap);
                s.set_error(volumes::busy_text(&name, &apps, key.as_deref()));
                s.eject.busy = Some((mount.clone(), Instant::now()));
                Some(s.message_id)
            }
            EjectOutcome::Failed(reason) => {
                s.set_error(format!("Cannot eject \"{name}\": {reason}"));
                None
            }
        }
    })
    .flatten();
    if let Some(id) = busy_message {
        // The force time ends: remove its message.
        after(FORCE_WINDOW, move || remove_message(id));
    }
    redraw_status();
    if ejected {
        volumes_changed();
    }
}

/// Removes the message `id` when it still shows.
fn remove_message(id: u64) {
    let removed = with(|s| {
        let shows = s.message.as_ref().is_some_and(|m| m.id == id);
        if shows {
            s.message = None;
        }
        shows
    });
    if removed == Some(true) {
        redraw_status();
    }
}

/// A new volume list from the background thread.
pub fn set_volumes(volumes: Vec<Volume>) {
    trace!(
        "volumes: {:?} after {} ms",
        volumes.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(),
        debug::since_start().as_millis()
    );
    with(|s| {
        let mounted_again = s
            .eject
            .ejected
            .as_ref()
            .is_some_and(|m| volumes.iter().any(|v| v.path == *m));
        if mounted_again {
            s.eject.ejected = None;
        }
        s.volumes = volumes;
    });
}

/// A volume mounted, unmounted, or changed its name: read the volume list
/// and build the sidebar again.
pub fn volumes_changed() {
    rebuild_sidebar();
    volumes::refresh();
}

/// A volume unmounted, for any reason: this app ejected it, another app
/// ejected it, or the cable came out. When the current folder is on it,
/// the pane goes to the home folder and shows `"X" was ejected`. Quick
/// Look and the prompts on the volume close. The history forgets the
/// folders on the volume.
pub fn volume_unmounted(mount: PathBuf, name: Option<String>) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let ql_visible = quicklook::is_visible(mtm);
    let effects = with(|s| {
        let mut fx = Vec::new();
        if mount == Path::new("/") {
            return fx;
        }
        let ours = s.eject.running.as_ref().is_some_and(|(m, _)| *m == mount)
            || s.eject.ejected.as_ref() == Some(&mount);
        if s.eject.busy.as_ref().is_some_and(|(m, _)| *m == mount) {
            s.eject.busy = None;
        }
        if s.leave_volume(&mount, ql_visible, &mut fx) {
            if !ours {
                let name = name.unwrap_or_else(|| file_name(&mount));
                s.set_info(format!("\"{name}\" was ejected"));
            }
            s.forget_after_load.push(mount.clone());
        }
        s.pane.forget_history_under(&mount);
        fx
    })
    .unwrap_or_default();
    run_effects(mtm, effects);
    sync_ui();
    volumes_changed();
}

/// Redraws the whole list (for example when new icons are available).
pub fn redraw_list() {
    if let Some(list) = with_ref(|s| s.ui.list.clone()) {
        list.setNeedsDisplay(true);
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

/// The text field of `place`.
fn place_field(ui: &Ui, place: PromptPlace) -> Option<Retained<objc2_app_kit::NSTextField>> {
    match place {
        PromptPlace::Bar => Some(ui.prompt.field.clone()),
        PromptPlace::Inline => Some(ui.prompt.inline.clone()),
        PromptPlace::Panel => ui.goto.field().cloned(),
        PromptPlace::Search => ui.toolbar.search_field().map(Retained::into_super),
    }
}

/// The text field of the open prompt. For `--debug-keys` and completion.
pub fn prompt_field() -> Option<Retained<objc2_app_kit::NSTextField>> {
    with_ref(|s| {
        s.prompt
            .is_some()
            .then(|| place_field(&s.ui, s.prompt_place))
            .flatten()
    })
    .flatten()
}

/// What a text change in a prompt does.
enum TextChange {
    Nothing,
    /// The filter changed: update the list.
    Filter,
    /// A path: list the matching folders. `sync`: the views change too
    /// (the location field shows or hides its match list).
    Query {
        sync: bool,
    },
}

/// The text of a prompt changed. The filter updates the list live. A
/// change in the toolbar filter field opens the filter mode (the user can
/// click into the field). In the filter field, a path (`is_path_query`)
/// selects the path mode: the list shows all entries, and the field shows
/// the matching folders, like the "go to" panel.
pub fn prompt_changed(text: String, is_search: bool) {
    let change = with(|s| {
        if is_search && s.prompt.is_none() {
            s.prompt = Some(PromptMode::Filter);
            s.prompt_place = PromptPlace::Search;
            s.path_mode = false;
        }
        if s.prompt == Some(PromptMode::Goto) && s.prompt_place == PromptPlace::Panel {
            return TextChange::Query { sync: false };
        }
        if s.prompt != Some(PromptMode::Filter) {
            return TextChange::Nothing;
        }
        if is_path_query(&text) {
            if !s.path_mode {
                s.path_mode = true;
                s.goto.clear();
            }
            if !s.pane.filter().is_empty() {
                s.pane.clear_filter();
                s.content_dirty = true;
                s.reveal_cursor = true;
            }
            return TextChange::Query { sync: true };
        }
        if s.path_mode {
            s.path_mode = false;
            s.goto.clear();
        }
        s.pane.set_filter(&text);
        s.content_dirty = true;
        s.reveal_cursor = true;
        TextChange::Filter
    });
    match change {
        Some(TextChange::Filter) => sync_ui(),
        Some(TextChange::Query { sync }) => {
            goto_query(text);
            if sync {
                sync_ui();
            }
        }
        _ => {}
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
                fx.push(Effect::ClearSearch);
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

/// The text field lost the focus (a click on another view). A rename
/// cancels; the filter stays.
pub fn prompt_end_editing() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let field = with_ref(|s| {
        s.prompt
            .is_some()
            .then(|| (place_field(&s.ui, s.prompt_place), s.ui.window.clone()))
    })
    .flatten();
    let Some((Some(field), window)) = field else {
        return;
    };
    // The field still has the focus when its field editor is the first
    // responder (a selector command ends and starts the edit again).
    let editing = field.currentEditor().is_some_and(|editor| {
        window.firstResponder().is_some_and(|r| {
            std::ptr::eq(
                Retained::as_ptr(&r).cast::<u8>(),
                Retained::as_ptr(&editor).cast::<u8>(),
            )
        })
    });
    if editing {
        return;
    }
    let effects = with(|s| {
        let mut fx = Vec::new();
        if s.prompt.is_some() {
            s.prompt = None;
            s.path_mode = false;
            if s.prompt_place == PromptPlace::Inline {
                fx.push(Effect::ClosePrompt(PromptPlace::Inline));
            } else if s.prompt_place == PromptPlace::Bar {
                fx.push(Effect::ClosePrompt(PromptPlace::Bar));
            }
        }
        fx
    })
    .unwrap_or_default();
    run_effects(mtm, effects);
    sync_ui();
}

/// What Up or Down did in a prompt.
enum PromptMove {
    /// A match of the "go to" panel.
    Panel,
    /// A match of the location field.
    Location,
    /// The list cursor (filter), or nothing to select.
    List,
}

/// Up and Down in the filter prompt move the list cursor. In the "go to"
/// panel and in the location field, they select a match.
pub fn prompt_move(delta: isize) -> bool {
    let moved = with(|s| {
        if s.goto_active() {
            let count = s.goto.matches.len().min(crate::goto_panel::MAX_MATCHES);
            if count == 0 {
                return Some(PromptMove::List);
            }
            s.goto.selected = Some(match (s.goto.selected, delta > 0) {
                (None, true) => 0,
                (None, false) => count - 1,
                (Some(i), true) => (i + 1).min(count - 1),
                (Some(i), false) => i.saturating_sub(1),
            });
            return Some(if s.prompt_place == PromptPlace::Panel {
                PromptMove::Panel
            } else {
                PromptMove::Location
            });
        }
        if s.prompt != Some(PromptMode::Filter) {
            return None;
        }
        s.pane.move_cursor(delta);
        Some(PromptMove::List)
    })
    .flatten();
    let Some(moved) = moved else {
        return false;
    };
    match moved {
        PromptMove::Panel => {
            if let Some(panel) = with_ref(|s| s.ui.goto.clone()) {
                panel.setNeedsDisplay(true);
            }
        }
        PromptMove::Location => {
            if let Some(list) = with_ref(|s| s.ui.location.clone()) {
                list.setNeedsDisplay(true);
            }
        }
        PromptMove::List => sync_ui(),
    }
    true
}

/// Splits the "go to" text into the folder part as typed, the name prefix,
/// and the folder path: `~/Doc` gives `~/`, `Doc`, and the home folder.
fn split_goto_text(text: &str, cwd: &Path) -> (String, String, PathBuf) {
    let (dir_text, prefix) = match text.rfind('/') {
        Some(i) => (text[..=i].to_string(), text[i + 1..].to_string()),
        None if text == "~" => ("~/".to_string(), String::new()),
        None => (String::new(), text.to_string()),
    };
    let dir = if dir_text.is_empty() {
        cwd.to_path_buf()
    } else {
        expand_path(&dir_text, cwd)
    };
    (dir_text, prefix, dir)
}

/// Lists the folders that match the "go to" text, on a thread. The panel
/// shows them when they arrive and the text is the same.
fn goto_query(text: String) {
    let info = with(|s| {
        s.goto.query += 1;
        (s.goto.query, s.pane.cwd().to_path_buf())
    });
    let Some((id, cwd)) = info else {
        return;
    };
    let (dir_text, prefix, dir) = split_goto_text(&text, &cwd);
    let spawned = std::thread::Builder::new()
        .name("bf-goto-list".into())
        .spawn(move || {
            let mut names = matching_directories(&dir, &prefix);
            names.sort_by(|a, b| bf_core::sort::natural_cmp(a, b));
            names.truncate(crate::goto_panel::MAX_MATCHES);
            after_now(move || goto_matches_arrived(id, dir_text, names));
        });
    if spawned.is_err() {
        trace!("goto: cannot start a thread");
    }
}

fn goto_matches_arrived(id: u64, dir_text: String, names: Vec<String>) {
    let ui = with(|s| {
        if s.goto.query != id || !s.goto_active() {
            return None;
        }
        s.goto.dir_text = dir_text;
        s.goto.matches = names;
        s.goto.selected = None;
        Some((
            s.ui.clone(),
            s.goto.matches.len(),
            s.prompt_place == PromptPlace::Panel,
        ))
    })
    .flatten();
    match ui {
        Some((ui, count, true)) => {
            ui.root.set_goto(Some(crate::goto_panel::height(count)));
            ui.goto.setNeedsDisplay(true);
        }
        // The location field: `sync_ui` shows the match list.
        Some((_, _, false)) => sync_ui(),
        None => {}
    }
}

/// Tab in the "go to" panel and in the location field completes the
/// directory name: the selected match, or the longest common prefix of the
/// matches. The directory read runs on a background thread.
pub fn prompt_complete(text: String) -> bool {
    let info = with_ref(|s| {
        s.goto_active().then(|| {
            let selected = s
                .goto
                .selected
                .and_then(|i| s.goto.matches.get(i))
                .map(|name| format!("{}{name}/", s.goto.dir_text));
            (s.pane.cwd().to_path_buf(), selected)
        })
    });
    let Some(Some((cwd, selected))) = info else {
        return false;
    };
    // A selected match of the panel completes to that folder.
    if let Some(new_text) = selected {
        if let Some(field) = prompt_field() {
            prompt::replace_field_text(&field, &new_text);
        }
        goto_query(new_text);
        return true;
    }
    let (dir_text, prefix, dir) = split_goto_text(&text, &cwd);
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
    if with_ref(AppState::goto_active) != Some(true) {
        return;
    }
    let Some(field) = prompt_field() else {
        return;
    };
    if prompt::field_text(&field) == original {
        prompt::replace_field_text(&field, &new_text);
        goto_query(new_text);
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
    let (style, warnings) = theme::style_for(&config, &window.effectiveAppearance());
    let new_config = config.clone();
    let ui = with(|s| {
        let sidebar_changed = s.config.sidebar_items != config.sidebar_items
            || s.config.sidebar_locations != config.sidebar_locations;
        let sidebar_toggled = s.config.sidebar != config.sidebar;
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
        (s.ui.clone(), sidebar_changed, sidebar_toggled)
    });
    let Some((ui, sidebar_changed, sidebar_toggled)) = ui else {
        return;
    };
    columns::reset();
    window::apply_look(&ui, &style, &new_config);
    if sidebar_toggled {
        ui.sidebar_item.setCollapsed(!new_config.sidebar);
    }
    if sidebar_changed || (sidebar_toggled && new_config.sidebar) {
        rebuild_sidebar();
    }
    if let Some(keymap) = with_ref(|s| s.config.keymap.clone()) {
        sync_menu_keys(&keymap);
    }
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
    if !matches!(mode, ThemeMode::Auto | ThemeMode::System) {
        return;
    }
    let appearance = window.effectiveAppearance();
    if theme::is_dark(&appearance) == was_dark {
        return;
    }
    icons::clear_rasters();
    let updated = with(|s| {
        let (style, _) = theme::style_for(&s.config, &appearance);
        s.style = style.clone();
        if s.help.is_some() {
            s.help = Some(HelpModel::build(&s.config.keymap, &s.style));
        }
        s.content_dirty = true;
        s.rendered.valid = false;
        (s.ui.clone(), style, s.config.clone())
    });
    let Some((ui, style, config)) = updated else {
        return;
    };
    window::apply_look(&ui, &style, &config);
    sync_ui();
}

// ---- View updates ----

enum Redraw {
    None,
    Rows(usize, usize),
    All,
}

/// What to do with the inline rename field after a list change.
enum InlinePlan {
    /// Put it over this row.
    Move(usize),
    /// The renamed item is not in the list now: close the field.
    Close,
}

/// The AppKit work to bring the views up to date with the state.
struct UiPlan {
    ui: Ui,
    window: Retained<NSWindow>,
    list: Retained<ListView>,
    status: Retained<StatusBar>,
    header: Retained<HeaderView>,
    redraw_header: bool,
    history: Option<(bool, bool)>,
    sidebar_path: Option<PathBuf>,
    /// The filter text for the toolbar field, when the field does not edit.
    search_text: Option<String>,
    inline: Option<InlinePlan>,
    inline_layout: Option<(theme::ColumnLayout, f64)>,
    metrics: Option<(usize, f64)>,
    redraw: Redraw,
    scroll_to: Option<usize>,
    /// A page action: scroll the list, with an animation when it is on.
    list_scroll: Option<ListScroll>,
    scroll_time: Option<Duration>,
    /// The cursor or the content changed without a page action: a smooth
    /// scroll ends at its target first.
    finish_scroll: bool,
    /// The match list of the location field changed: show it at this place
    /// with this number of rows, or hide it (`None`).
    location: Option<Option<(PromptPlace, usize)>>,
    /// Redraw the match list of the location field.
    location_redraw: bool,
    /// The title, the directory, and true when it is on a local volume.
    title: Option<(String, PathBuf, bool)>,
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
        } else if cursor_changed && self.scroll_plan.is_none() {
            Redraw::Rows(r.cursor, cursor)
        } else {
            Redraw::None
        };
        let list_scroll = self.scroll_plan.take();
        let finish_scroll =
            list_scroll.is_none() && (cursor_changed || self.content_dirty || layout_changed);
        let scroll_to = (list_scroll.is_none()
            && (self.reveal_cursor || dir_changed || cursor_changed))
            .then_some(cursor);
        let location = self.location_active().then(|| {
            (
                self.prompt_place,
                self.goto.matches.len().min(crate::goto_panel::MAX_MATCHES),
            )
        });
        let location_changed = !r.valid || r.location != location;
        let title = dir_changed.then(|| (display_path(&cwd), cwd.clone(), self.cwd_is_local));
        let quick_look_reload = dir_changed || r.current != current;
        let history = (self.pane.can_go_back(), self.pane.can_go_forward());
        let sort = self.pane.sort();
        let redraw_header = self.content_dirty || r.sort != Some(sort) || layout_changed;
        let history_changed = !r.valid || r.history != history;
        let content_changed = self.content_dirty || layout_changed;
        let editing_search =
            self.prompt == Some(PromptMode::Filter) && self.prompt_place == PromptPlace::Search;
        let search_text = (!editing_search).then(|| self.pane.filter().to_string());
        let mut inline = None;
        if content_changed && self.prompt_place == PromptPlace::Inline {
            if let Some(PromptMode::Rename { source }) = &self.prompt {
                let row = (0..len).find(|&i| {
                    self.pane
                        .visible_entry(i)
                        .is_some_and(|e| &e.path == source)
                });
                inline = Some(match row {
                    Some(row) => InlinePlan::Move(row),
                    None => {
                        self.prompt = None;
                        InlinePlan::Close
                    }
                });
            }
        }
        let inline_layout = inline.as_ref().map(|_| {
            (
                self.style.layout(self.ui.list.bounds().size.width),
                (self.style.line_height + 4.0).round(),
            )
        });
        self.content_dirty = false;
        self.reveal_cursor = false;
        self.rendered = Rendered {
            valid: true,
            cwd: cwd.clone(),
            cursor,
            len,
            current,
            row_height,
            history,
            sort: Some(sort),
            location,
        };
        UiPlan {
            ui: self.ui.clone(),
            window: self.ui.window.clone(),
            list: self.ui.list.clone(),
            status: self.ui.status.clone(),
            header: self.ui.header.clone(),
            redraw_header,
            history: history_changed.then_some(history),
            sidebar_path: dir_changed.then_some(cwd),
            search_text,
            inline,
            inline_layout,
            metrics: layout_changed.then_some((len, row_height)),
            redraw,
            scroll_to,
            list_scroll,
            scroll_time: self.config.smooth_scroll_time(),
            finish_scroll,
            location: location_changed.then_some(location),
            location_redraw: location.is_some(),
            title,
            quick_look_reload,
        }
    }
}

impl UiPlan {
    fn apply(self) {
        if self.finish_scroll {
            smooth::finish();
        }
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
        if let Some(plan) = self.list_scroll {
            smooth::scroll_list(&self.list, &self.ui.scroll, plan, self.scroll_time);
        }
        if let Some(row) = self.scroll_to {
            self.list.scroll_to_row(row);
        }
        if let Some(location) = self.location {
            match location {
                Some((place, count)) => {
                    let anchor = place_field(&self.ui, place)
                        .map(|f| Retained::into_super(Retained::into_super(f)));
                    self.ui
                        .root
                        .set_location(anchor, crate::goto_panel::list_height(count));
                }
                None => self.ui.root.set_location(None, 0.0),
            }
        }
        if self.location_redraw {
            self.ui.location.setNeedsDisplay(true);
        }
        self.status.setNeedsDisplay(true);
        if self.redraw_header {
            self.header.setNeedsDisplay(true);
        }
        if let Some((back, forward)) = self.history {
            self.ui.toolbar.set_navigation(back, forward);
        }
        if let Some(path) = &self.sidebar_path {
            sidebar::select_path(path);
        }
        if let Some(text) = &self.search_text {
            if let Some(field) = self.ui.toolbar.search_field() {
                if field.stringValue().to_string() != *text {
                    field.setStringValue(&NSString::from_str(text));
                }
            }
        }
        match (self.inline, self.inline_layout) {
            (Some(InlinePlan::Move(row)), Some((layout, field_h))) => {
                let frame = self.list.name_field_frame(row, &layout, field_h);
                self.ui.prompt.inline.setFrame(frame);
            }
            (Some(InlinePlan::Close), _) => {
                self.window
                    .makeFirstResponder(Some(list_responder(&self.list)));
                self.ui.prompt.inline.removeFromSuperview();
            }
            _ => {}
        }
        if let Some((title, path, is_local)) = self.title {
            self.window.setTitle(&NSString::from_str(&title));
            if is_local {
                // The proxy icon costs a few ms. Set it after the frame.
                after_now(move || set_represented_url(&path));
            } else {
                // The proxy icon reads the disk. Skip it on a network volume.
                self.window.setRepresentedURL(None);
            }
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
