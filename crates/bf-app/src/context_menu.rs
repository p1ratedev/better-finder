//! The context menu of the list (a right click or a control click), with
//! the Finder items and order of macOS 15. See docs/UI.md, "Context menu".
//!
//! - A click on a row moves the cursor to the row, then shows the menu of
//!   that entry.
//! - A click below the rows shows the menu of the current folder. The
//!   cursor does not move.
//!
//! The menu items do not call AppKit actions of their own. One target object
//! (`MenuTarget`) gets all clicks. The tag of an item is the index of its
//! command in the model of the open menu. The command runs on the main
//! queue after the menu closes, without a borrow of the app state.
//!
//! This module also has the AppKit parts of some actions: Open With (and
//! its app panel), Share, and the Services of the list.

use std::cell::{Cell, OnceCell, RefCell};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use bf_core::Action;
use bf_core::Entry;
use bf_core::fs::display_path;
use bf_core::names::truncate_middle;
use bf_core::sort::SortKey;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, ClassType, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSControlStateValueOn, NSMenu, NSMenuDelegate, NSMenuItem, NSModalResponse,
    NSModalResponseOK, NSOpenPanel, NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypeString,
    NSPasteboardWriting, NSRunningApplication, NSSharingServicePicker, NSWorkspace,
    NSWorkspaceOpenConfiguration,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSError, NSFileManager, NSInteger, NSObject, NSObjectProtocol,
    NSRectEdge, NSSize, NSString, NSURL,
};
use objc2_uniform_type_identifiers::UTTypeApplicationBundle;

use crate::app;
use crate::debug::trace;
use crate::list_view::ListView;

/// The longest file name in an item title, in characters.
const TITLE_NAME_MAX: usize = 40;

/// What a menu item does.
#[derive(Clone, Debug)]
enum Command {
    Action(Action),
    /// Open the entry with the app at this path.
    OpenWith(PathBuf),
    /// Choose an app in a panel, then open the entry with it.
    OpenWithOther,
}

/// What the open menu is for.
#[derive(Clone, Debug)]
enum Subject {
    /// The entry of the clicked row.
    Entry(PathBuf),
    /// The current folder (a click below the rows).
    Folder(PathBuf),
}

/// The commands of the open menu. An item tag is an index into `commands`.
struct Model {
    subject: Subject,
    commands: Vec<Command>,
    /// The file of the Open With submenu. The submenu fills when it opens
    /// (`menuNeedsUpdate:`): the app list and the icons take 10 to 30 ms.
    open_with: Option<PathBuf>,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclass requirements.
    // - MenuTarget does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BFMenuTarget"]
    struct MenuTarget;

    // SAFETY: `NSObjectProtocol` has no safety requirements. The signatures match.
    unsafe impl NSObjectProtocol for MenuTarget {}

    // SAFETY: `NSMenuDelegate` has no safety requirements. The signatures match.
    unsafe impl NSMenuDelegate for MenuTarget {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            // Only the Open With submenu is empty before it opens.
            if menu.numberOfItems() == 0 {
                fill_open_with_now(menu);
            }
        }

        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, menu: &NSMenu) {
            // SAFETY: AppKit gives a live menu. The call only reads its
            // supermenu, and the result is only checked for `None`.
            if unsafe { menu.supermenu() }.is_none() {
                FOLDER_MENU.set(false);
            }
        }
    }

    impl MenuTarget {
        #[unsafe(method(menuItemClicked:))]
        fn menu_item_clicked(&self, sender: Option<&NSMenuItem>) {
            if let Some(item) = sender {
                clicked(item.tag());
            }
        }
    }
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: `init` is the designated initializer of NSObject.
        unsafe { msg_send![super(this), init] }
    }
}

thread_local! {
    /// `NSMenuItem.target` is a weak reference: this keeps the target alive.
    static TARGET: OnceCell<Retained<MenuTarget>> = const { OnceCell::new() };
    /// The model of the most recent menu.
    static MODEL: RefCell<Option<Model>> = const { RefCell::new(None) };
    /// The share picker that shows now.
    static PICKER: RefCell<Option<Retained<NSSharingServicePicker>>> =
        const { RefCell::new(None) };
    /// The menu of the folder is open. The Services of the list act on the
    /// entry under the cursor, not on the folder: they are off in this menu.
    static FOLDER_MENU: Cell<bool> = const { Cell::new(false) };
}

fn target(mtm: MainThreadMarker) -> Retained<MenuTarget> {
    TARGET.with(|t| t.get_or_init(|| MenuTarget::new(mtm)).clone())
}

/// A menu item click: run its command after the menu closes.
fn clicked(tag: NSInteger) {
    let picked = MODEL.with(|m| {
        let model = m.borrow();
        let model = model.as_ref()?;
        let command = model.commands.get(usize::try_from(tag).ok()?)?.clone();
        Some((model.subject.clone(), command))
    });
    if let Some((subject, command)) = picked {
        app::after_now(move || run(subject, command));
    }
}

/// Runs a command. An automatic refresh can change the list while the menu
/// is open: the command acts only when its entry or folder still shows.
fn run(subject: Subject, command: Command) {
    trace!("context menu: {command:?} on {subject:?}");
    let path = match &subject {
        Subject::Entry(path) => {
            if !app::cursor_to(path) {
                app::show_error(format!("\"{}\" is not in the list now", file_name(path)));
                return;
            }
            path.clone()
        }
        Subject::Folder(dir) => {
            if !app::shows_folder(dir) {
                app::show_error("The folder changed. Open the menu again.".to_string());
                return;
            }
            dir.clone()
        }
    };
    match (command, subject) {
        (Command::Action(action), Subject::Entry(_)) => app::dispatch(action),
        (Command::Action(action), Subject::Folder(dir)) => app::dispatch_on(action, dir),
        (Command::OpenWith(app_path), _) => open_with(&[path], &app_path),
        (Command::OpenWithOther, _) => choose_app(path),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// `“name”`, truncated in the middle, for item titles.
fn quoted(name: &str) -> String {
    format!("\u{201C}{}\u{201D}", truncate_middle(name, TITLE_NAME_MAX))
}

/// Builds one menu and its commands.
struct Builder {
    mtm: MainThreadMarker,
    target: Retained<MenuTarget>,
    commands: Vec<Command>,
    /// See `Model::open_with`.
    open_with: Option<PathBuf>,
}

impl Builder {
    fn new(mtm: MainThreadMarker) -> Self {
        Self {
            mtm,
            target: target(mtm),
            commands: Vec::new(),
            open_with: None,
        }
    }

    fn menu(&self, title: &str) -> Retained<NSMenu> {
        let menu = NSMenu::initWithTitle(NSMenu::alloc(self.mtm), &NSString::from_str(title));
        // The items are enabled or disabled here, not by the responder chain.
        menu.setAutoenablesItems(false);
        menu
    }

    fn item(&mut self, menu: &NSMenu, title: &str, command: Command) -> Retained<NSMenuItem> {
        // SAFETY: `menuItemClicked:` is a method of the target.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm),
                &NSString::from_str(title),
                Some(sel!(menuItemClicked:)),
                &NSString::from_str(""),
            )
        };
        // SAFETY: The target lives in a thread_local for the whole app run.
        unsafe { item.setTarget(Some(&self.target)) };
        item.setTag(self.commands.len() as NSInteger);
        self.commands.push(command);
        menu.addItem(&item);
        item
    }

    fn action(&mut self, menu: &NSMenu, title: &str, action: Action) -> Retained<NSMenuItem> {
        self.item(menu, title, Command::Action(action))
    }

    /// An item with a checkmark when `on` is true.
    fn check(&mut self, menu: &NSMenu, title: &str, action: Action, on: bool) {
        let item = self.action(menu, title, action);
        if on {
            item.setState(NSControlStateValueOn);
        }
    }

    fn separator(&self, menu: &NSMenu) {
        menu.addItem(&NSMenuItem::separatorItem(self.mtm));
    }

    fn submenu(&self, menu: &NSMenu, title: &str) -> Retained<NSMenu> {
        let sub = self.menu(title);
        let item = NSMenuItem::new(self.mtm);
        item.setTitle(&NSString::from_str(title));
        item.setSubmenu(Some(&sub));
        menu.addItem(&item);
        sub
    }

    /// Stores the commands for the clicks, and gives the menu back.
    fn finish(self, menu: Retained<NSMenu>, subject: Subject) -> Retained<NSMenu> {
        FOLDER_MENU.set(matches!(subject, Subject::Folder(_)));
        // `menuDidClose:` clears `FOLDER_MENU`.
        menu.setDelegate(Some(ProtocolObject::from_ref(&*self.target)));
        MODEL.with(|m| {
            *m.borrow_mut() = Some(Model {
                subject,
                commands: self.commands,
                open_with: self.open_with,
            })
        });
        menu
    }
}

/// Fills the Open With submenu when it opens. The commands of its items
/// go after the commands of the main menu.
fn fill_open_with_now(menu: &NSMenu) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let taken = MODEL.with(|m| {
        let mut model = m.borrow_mut();
        let model = model.as_mut()?;
        let path = model.open_with.take()?;
        Some((path, std::mem::take(&mut model.commands)))
    });
    let Some((path, commands)) = taken else {
        return;
    };
    let started = Instant::now();
    let mut b = Builder {
        mtm,
        target: target(mtm),
        commands,
        open_with: None,
    };
    fill_open_with(&mut b, menu, &path);
    MODEL.with(|m| {
        if let Some(model) = m.borrow_mut().as_mut() {
            model.commands = b.commands;
        }
    });
    trace!(
        "context menu: Open With in {} µs",
        started.elapsed().as_micros()
    );
}

/// The menu for a right click or a control click on the list. `row` is the
/// row under the mouse; it can be below the last row. The list becomes the
/// first responder. A click on a row moves the cursor to it first.
pub fn menu_for_click(list: &ListView, row: Option<usize>) -> Option<Retained<NSMenu>> {
    if let Some(window) = list.window() {
        window.makeFirstResponder(Some(list.as_super().as_super()));
    }
    let len = app::with_ref(|s| s.pane.is_loaded().then(|| s.pane.visible_len())).flatten()?;
    match row.filter(|&row| row < len) {
        Some(row) => {
            app::click_row(row, false);
            entry_menu(row)
        }
        None => folder_menu(),
    }
}

/// The menu of the entry in the list row `row`.
fn entry_menu(row: usize) -> Option<Retained<NSMenu>> {
    let mtm = MainThreadMarker::new()?;
    let entry = app::with_ref(|s| s.pane.visible_entry(row).cloned()).flatten()?;
    let started = Instant::now();
    let mut b = Builder::new(mtm);
    let menu = b.menu("");
    fill_entry_menu(&mut b, &menu, &entry);
    trace!(
        "context menu: \"{}\" in {} µs",
        entry.name,
        started.elapsed().as_micros()
    );
    Some(b.finish(menu, Subject::Entry(entry.path)))
}

fn fill_entry_menu(b: &mut Builder, menu: &NSMenu, entry: &Entry) {
    let name = quoted(&entry.name);
    let plain_folder = entry.is_dir && !entry.is_package;
    b.action(menu, "Open", Action::Open);
    if !plain_folder {
        // The submenu fills when it opens: `fill_open_with_now`.
        let sub = b.submenu(menu, "Open With");
        sub.setDelegate(Some(ProtocolObject::from_ref(&*b.target)));
        b.open_with = Some(entry.path.clone());
    }
    if entry.is_package {
        let path = entry.path.to_string_lossy().into_owned();
        b.action(menu, "Show Package Contents", Action::Goto(path));
    }
    b.separator(menu);
    b.action(menu, "Move to Trash", Action::Trash);
    b.separator(menu);
    b.action(menu, "Get Info", Action::GetInfo);
    b.action(menu, "Rename", Action::Rename);
    b.action(menu, &format!("Compress {name}"), Action::Compress);
    b.action(menu, "Duplicate", Action::Duplicate);
    b.action(menu, "Make Alias", Action::MakeAlias);
    b.action(menu, &format!("Quick Look {name}"), Action::QuickLook);
    b.separator(menu);
    b.action(menu, &format!("Copy {name}"), Action::Copy);
    b.action(menu, "Copy as Pathname", Action::CopyPath);
    b.action(menu, "Share\u{2026}", Action::Share);
    b.separator(menu);
    b.action(menu, "Show in Finder", Action::RevealInFinder);
}

/// The name of an app for a menu: the display name without `.app`.
fn app_name(app: &Path) -> String {
    let path = NSString::from_str(&app.to_string_lossy());
    let name = NSFileManager::defaultManager()
        .displayNameAtPath(&path)
        .to_string();
    match name.strip_suffix(".app") {
        Some(stem) if !stem.is_empty() => stem.to_string(),
        _ => name,
    }
}

/// The Open With submenu: the default app, the other apps that can open
/// the file (sorted by name), and `Other…`. Each app has its icon.
fn fill_open_with(b: &mut Builder, menu: &NSMenu, path: &Path) {
    let workspace = NSWorkspace::sharedWorkspace();
    let Some(url) = NSURL::from_file_path(path) else {
        b.item(menu, "Other\u{2026}", Command::OpenWithOther);
        return;
    };
    let default = workspace
        .URLForApplicationToOpenURL(&url)
        .and_then(|u| u.to_file_path());
    let mut others: Vec<(String, PathBuf)> = workspace
        .URLsForApplicationsToOpenURL(&url)
        .iter()
        .filter_map(|u| u.to_file_path())
        .filter(|app| Some(app) != default.as_ref())
        .map(|app| (app_name(&app), app))
        .collect();
    others.sort_by(|a, b| bf_core::sort::natural_cmp(&a.0, &b.0).then(a.1.cmp(&b.1)));
    others.dedup_by(|a, b| a.1 == b.1);
    let mut groups = 0;
    if let Some(app) = default {
        let title = format!("{} (default)", app_name(&app));
        app_item(b, menu, &title, app);
        groups += 1;
    }
    if !others.is_empty() {
        if groups > 0 {
            b.separator(menu);
        }
        for (name, app) in others {
            app_item(b, menu, &name, app);
        }
        groups += 1;
    }
    if groups > 0 {
        b.separator(menu);
    }
    b.item(menu, "Other\u{2026}", Command::OpenWithOther);
}

/// An Open With item with the 16 x 16 icon of the app.
fn app_item(b: &mut Builder, menu: &NSMenu, title: &str, app: PathBuf) {
    let icon =
        NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(&app.to_string_lossy()));
    icon.setSize(NSSize::new(16.0, 16.0));
    let item = b.item(menu, title, Command::OpenWith(app));
    item.setImage(Some(&icon));
}

/// The menu of the current folder (a click below the rows).
fn folder_menu() -> Option<Retained<NSMenu>> {
    let mtm = MainThreadMarker::new()?;
    let (cwd, sort, hidden) = app::with_ref(|s| {
        (
            s.pane.cwd().to_path_buf(),
            s.pane.sort(),
            s.pane.show_hidden(),
        )
    })?;
    let paste_count = app::pasteboard_files().len();
    let mut b = Builder::new(mtm);
    let menu = b.menu("");
    b.action(&menu, "New Folder", Action::NewFolder);
    b.separator(&menu);
    b.action(&menu, "Get Info", Action::GetInfo);
    b.separator(&menu);
    let title = match paste_count {
        0 | 1 => "Paste Item".to_string(),
        n => format!("Paste {n} Items"),
    };
    b.action(&menu, &title, Action::Paste)
        .setEnabled(paste_count > 0);
    b.separator(&menu);
    let sub = b.submenu(&menu, "Sort By");
    for (title, key) in [
        ("Name", SortKey::Name),
        ("Size", SortKey::Size),
        ("Date Modified", SortKey::Modified),
        ("Date Created", SortKey::Created),
        ("Extension", SortKey::Extension),
    ] {
        b.check(&sub, title, Action::Sort(key), sort.key == key);
    }
    b.separator(&sub);
    b.check(
        &sub,
        "Reverse Order",
        Action::ToggleSortReverse,
        sort.reverse,
    );
    b.check(&menu, "Show Hidden Files", Action::ToggleHidden, hidden);
    b.separator(&menu);
    b.action(&menu, "Copy as Pathname", Action::CopyPath);
    b.action(&menu, "Open in Terminal", Action::OpenTerminal);
    b.action(&menu, "Show in Finder", Action::RevealInFinder);
    Some(b.finish(menu, Subject::Folder(cwd)))
}

/// Opens `files` with the app at `app` (Open With, or a drop on an app
/// row). An error shows in the status bar.
pub fn open_with(files: &[PathBuf], app: &Path) {
    let file_urls: Vec<Retained<NSURL>> = files.iter().filter_map(NSURL::from_file_path).collect();
    let (Some(first), Some(app_url)) = (files.first(), NSURL::from_directory_path(app)) else {
        return;
    };
    let what = match files {
        [_] => format!("\"{}\"", file_name(first)),
        _ => format!("{} items", files.len()),
    };
    let failure = format!("Cannot open {what} with {}", app_name(app));
    // AppKit calls the handler on a background queue. It only makes text
    // and sends it to the main queue.
    let handler = RcBlock::new(
        move |_app: *mut NSRunningApplication, error: *mut NSError| {
            // SAFETY: AppKit gives a valid error or null.
            if let Some(error) = unsafe { error.as_ref() } {
                let text = format!("{failure}: {}", error.localizedDescription());
                app::after_now(move || app::show_error(text));
            }
        },
    );
    let config = NSWorkspaceOpenConfiguration::configuration();
    NSWorkspace::sharedWorkspace().openURLs_withApplicationAtURL_configuration_completionHandler(
        &NSArray::from_retained_slice(&file_urls),
        &app_url,
        &config,
        Some(&handler),
    );
}

/// Open With > Other…: a panel at /Applications that accepts only apps.
/// The chosen app opens `file`.
fn choose_app(file: PathBuf) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(window) = app::with_ref(|s| s.ui.window.clone()) else {
        return;
    };
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(false);
    panel.setDirectoryURL(NSURL::from_directory_path("/Applications").as_deref());
    // SAFETY: `UTTypeApplicationBundle` is an immutable static.
    let apps = unsafe { UTTypeApplicationBundle };
    panel.setAllowedContentTypes(&NSArray::from_slice(&[apps]));
    panel.setPrompt(Some(&NSString::from_str("Open")));
    panel.setMessage(Some(&NSString::from_str(&format!(
        "Choose an app to open {}.",
        quoted(&file_name(&file))
    ))));
    let chosen = panel.clone();
    let handler = RcBlock::new(move |response: NSModalResponse| {
        if response != NSModalResponseOK {
            return;
        }
        if let Some(app) = chosen.URL().and_then(|u| u.to_file_path()) {
            open_with(std::slice::from_ref(&file), &app);
        }
    });
    panel.beginSheetModalForWindow_completionHandler(&window, &handler);
}

/// Shows the macOS share menu for `path`, next to the list row `row`.
pub fn share(path: &Path, row: usize) {
    let Some(list) = app::with_ref(|s| s.ui.list.clone()) else {
        return;
    };
    let Some(url) = NSURL::from_file_path(path) else {
        return;
    };
    let item: Retained<AnyObject> = url.into();
    let items = NSArray::from_retained_slice(&[item]);
    // SAFETY: The items conform to `NSPasteboardWriting` (a file URL).
    let picker =
        unsafe { NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), &items) };
    list.scroll_to_row(row);
    picker.showRelativeToRect_ofView_preferredEdge(list.row_rect(row), &list, NSRectEdge::MaxY);
    PICKER.with(|p| *p.borrow_mut() = Some(picker));
}

// ---- Services ----

/// Registers the types that the list can send to services: a file URL and
/// a string (the path).
pub fn register_services(app: &NSApplication) {
    // SAFETY: The pasteboard types are immutable statics from AppKit.
    let send = unsafe { [NSPasteboardTypeFileURL, NSPasteboardTypeString] };
    app.registerServicesMenuSendTypes_returnTypes(&NSArray::from_slice(&send), &NSArray::new());
}

/// True when the list can give the entry under the cursor to a service that
/// takes `send` and returns nothing.
pub fn serves(send: Option<&NSString>, give_back: Option<&NSString>) -> bool {
    let Some(send) = send else {
        return false;
    };
    if give_back.is_some() || FOLDER_MENU.get() {
        return false;
    }
    // SAFETY: The pasteboard types are immutable statics from AppKit.
    let known = unsafe { [NSPasteboardTypeFileURL, NSPasteboardTypeString] };
    known.contains(&send) && app::with_ref(|s| s.pane.current().is_some()) == Some(true)
}

/// Writes the entry under the cursor for a service: its file URL and its
/// path as a string.
pub fn write_selection(pasteboard: &NSPasteboard) -> bool {
    let Some(Some(path)) = app::with_ref(|s| s.pane.current().map(|e| e.path.clone())) else {
        return false;
    };
    let Some(url) = NSURL::from_file_path(&path) else {
        return false;
    };
    pasteboard.clearContents();
    let object: &ProtocolObject<dyn NSPasteboardWriting> = ProtocolObject::from_ref(&*url);
    let wrote_url = pasteboard.writeObjects(&NSArray::from_slice(&[object]));
    // SAFETY: `NSPasteboardTypeString` is an immutable static from AppKit.
    let kind = unsafe { NSPasteboardTypeString };
    let wrote_text =
        pasteboard.setString_forType(&NSString::from_str(&path.to_string_lossy()), kind);
    wrote_url || wrote_text
}

// ---- Debug ----

/// `--debug-keys` `@menu:<row>` and `@menu:bg`: builds the context menu like
/// a right click (a row moves the cursor first) and prints its items to
/// stdout. Submenus are indented. `[x]` is a checkmark, `---` a separator.
pub fn debug_print(row: Option<usize>) {
    let menu = match row {
        Some(row) => {
            app::click_row(row, false);
            entry_menu(row)
        }
        None => folder_menu(),
    };
    let subject = app::with_ref(|s| match row {
        Some(row) => s.pane.visible_entry(row).map(|e| e.name.clone()),
        None => Some(display_path(s.pane.cwd())),
    })
    .flatten()
    .unwrap_or_default();
    let mut text = match row {
        Some(row) => format!("menu: row {row} \"{subject}\"\n"),
        None => format!("menu: folder {subject}\n"),
    };
    match menu {
        Some(menu) => print_items(&menu, 1, &mut text),
        None => text.push_str("  (no menu)\n"),
    }
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

fn print_items(menu: &NSMenu, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    for item in menu.itemArray().iter() {
        if item.isSeparatorItem() {
            out.push_str(&format!("{indent}---\n"));
            continue;
        }
        let check = if item.state() == NSControlStateValueOn {
            "[x] "
        } else {
            ""
        };
        let disabled = if item.isEnabled() { "" } else { " (disabled)" };
        out.push_str(&format!("{indent}{check}{}{disabled}\n", item.title()));
        if let Some(sub) = item.submenu() {
            // Open With fills when it opens: fill it now.
            fill_open_with_now(&sub);
            print_items(&sub, depth + 1, out);
        }
    }
}
