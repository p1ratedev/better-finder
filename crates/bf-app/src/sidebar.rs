//! The sidebar: an `NSOutlineView` with the source list style, like Finder.
//!
//! - Two groups: Favorites (from the `sidebar-item` keys) and Locations
//!   (iCloud Drive, the mounted volumes, the Trash).
//! - A background thread builds the model: existence checks, display names,
//!   and the volume list access the disk. The main thread only shows it.
//! - The row of the current folder is selected. A click opens the row.
//! - With the keyboard focus in the sidebar, `j`, `k`, and the arrows move
//!   the selection; `enter`, `l`, and `right` open the folder; `escape`,
//!   `tab`, and `h` give the focus back to the list. Other keys go to the
//!   normal keymap.

use std::cell::{OnceCell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackgroundStyle, NSColor, NSControl, NSControlTextEditingDelegate,
    NSEvent, NSEventModifierFlags, NSFont, NSFontWeightSemibold, NSImageScaling, NSImageView,
    NSLineBreakMode, NSOutlineView, NSOutlineViewDataSource, NSOutlineViewDelegate, NSResponder,
    NSScrollView, NSTableCellView, NSTableColumn, NSTableRowView, NSTableView,
    NSTableViewDataSource, NSTableViewRowSizeStyle, NSTableViewStyle, NSTextField, NSView,
    NSWorkspace, NSWorkspaceDidMountNotification, NSWorkspaceDidRenameVolumeNotification,
    NSWorkspaceDidUnmountNotification,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSFileManager, NSIndexSet, NSInteger, NSNotification, NSNumber,
    NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURLVolumeIsInternalKey,
    NSURLVolumeIsLocalKey, NSURLVolumeLocalizedNameKey, NSVolumeEnumerationOptions,
};

use bf_core::KeyEvent;
use bf_core::keymap::{Key, NamedKey};

use crate::app;
use crate::debug::trace;
use crate::list_view;

/// One folder row.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    /// The SF Symbol name.
    pub symbol: &'static str,
}

/// The rows of the two groups.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    pub favorites: Vec<Row>,
    pub locations: Vec<Row>,
}

/// The SF Symbol for a favorite folder, like Finder.
fn symbol_for(path: &Path, home: &Path) -> &'static str {
    if path == home {
        return "house";
    }
    if path == Path::new("/Applications") {
        return "app.dashed";
    }
    if path.parent() == Some(home) {
        match path.file_name().and_then(|n| n.to_str()) {
            Some("Desktop") => return "menubar.dock.rectangle",
            Some("Documents") => return "doc",
            Some("Downloads") => return "arrow.down.circle",
            Some("Movies") => return "film",
            Some("Music") => return "music.note",
            Some("Pictures") => return "photo",
            _ => {}
        }
    }
    "folder"
}

/// Builds the model. It accesses the disk: call it on a background thread.
pub fn build_model(items: &[PathBuf], locations: bool) -> Model {
    let manager = NSFileManager::defaultManager();
    let home = bf_core::fs::home_dir();
    let favorites = items
        .iter()
        .filter(|path| path.is_dir())
        .map(|path| Row {
            path: path.clone(),
            name: manager
                .displayNameAtPath(&NSString::from_str(&path.to_string_lossy()))
                .to_string(),
            symbol: symbol_for(path, &home),
        })
        .collect();
    let mut model = Model {
        favorites,
        locations: Vec::new(),
    };
    if !locations {
        return model;
    }
    let icloud = home.join("Library/Mobile Documents/com~apple~CloudDocs");
    if icloud.is_dir() {
        model.locations.push(Row {
            path: icloud,
            name: "iCloud Drive".to_string(),
            symbol: "icloud",
        });
    }
    model.locations.extend(volumes());
    let trash = home.join(".Trash");
    if trash.is_dir() {
        model.locations.push(Row {
            path: trash,
            name: "Trash".to_string(),
            symbol: "trash",
        });
    }
    model
}

/// The mounted volumes that are not hidden, the boot volume first.
fn volumes() -> Vec<Row> {
    let manager = NSFileManager::defaultManager();
    // SAFETY: The resource keys are immutable statics from Foundation.
    let (name_key, internal_key, local_key) = unsafe {
        (
            NSURLVolumeLocalizedNameKey,
            NSURLVolumeIsInternalKey,
            NSURLVolumeIsLocalKey,
        )
    };
    let keys = NSArray::from_slice(&[name_key, internal_key, local_key]);
    let Some(urls) = manager.mountedVolumeURLsIncludingResourceValuesForKeys_options(
        Some(&keys),
        NSVolumeEnumerationOptions::SkipHiddenVolumes,
    ) else {
        return Vec::new();
    };
    let mut rows: Vec<Row> = Vec::new();
    for url in urls.iter() {
        let Some(path) = url.to_file_path() else {
            continue;
        };
        let Ok(values) = url.resourceValuesForKeys_error(&keys) else {
            continue;
        };
        let flag = |key: &objc2_foundation::NSURLResourceKey| {
            values
                .objectForKey(key)
                .and_then(|v| v.downcast::<NSNumber>().ok())
                .map(|n| n.boolValue())
        };
        let name = values
            .objectForKey(name_key)
            .and_then(|v| v.downcast::<NSString>().ok())
            .map(|s| s.to_string())
            .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let symbol = if flag(local_key) == Some(false) {
            "network"
        } else if flag(internal_key) == Some(true) {
            "internaldrive"
        } else {
            "externaldrive"
        };
        rows.push(Row { path, name, symbol });
    }
    // The boot volume first.
    rows.sort_by_key(|r| r.path != Path::new("/"));
    rows
}

/// What an outline item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Group(usize),
    Row(usize, usize),
}

/// The cell of one folder row. It changes its colors with the selection.
pub struct CellIvars {
    image: OnceCell<Retained<NSImageView>>,
    text: OnceCell<Retained<NSTextField>>,
}

define_class!(
    // SAFETY:
    // - NSTableCellView has no subclass requirements.
    // - SidebarCell does not implement Drop.
    #[unsafe(super(NSTableCellView, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BFSidebarCell"]
    #[ivars = CellIvars]
    pub struct SidebarCell;

    impl SidebarCell {
        #[unsafe(method(setBackgroundStyle:))]
        fn set_background_style(&self, style: NSBackgroundStyle) {
            // SAFETY: NSTableCellView implements this method with this signature.
            let _: () = unsafe { msg_send![super(self), setBackgroundStyle: style] };
            self.recolor(style);
        }
    }
);

impl SidebarCell {
    fn new(mtm: MainThreadMarker, row: &Row) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(CellIvars {
            image: OnceCell::new(),
            text: OnceCell::new(),
        });
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(160.0, 24.0));
        // SAFETY: `initWithFrame:` is the designated initializer of NSView.
        let cell: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        let image_view = NSImageView::new(mtm);
        if let Some(image) =
            objc2_app_kit::NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(row.symbol),
                None,
            )
        {
            image.setTemplate(true);
            image_view.setImage(Some(&image));
        }
        image_view.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        image_view.setFrame(NSRect::new(NSPoint::new(2.0, 3.0), NSSize::new(18.0, 18.0)));
        image_view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinYMargin | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        let text = NSTextField::labelWithString(&NSString::from_str(&row.name), mtm);
        text.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        text.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        text.setFrame(NSRect::new(
            NSPoint::new(26.0, 4.0),
            NSSize::new(130.0, 16.0),
        ));
        text.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewMinYMargin
                | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        cell.addSubview(&image_view);
        cell.addSubview(&text);
        // SAFETY: The cell keeps both subviews alive.
        unsafe {
            cell.setImageView(Some(&image_view));
            cell.setTextField(Some(&text));
        }
        let _ = cell.ivars().image.set(image_view);
        let _ = cell.ivars().text.set(text);
        cell.recolor(NSBackgroundStyle::Normal);
        cell
    }

    /// The colors, like Finder: gray icons; the selected row uses the
    /// accent color; with the focus (emphasized), white on the accent.
    fn recolor(&self, style: NSBackgroundStyle) {
        // SAFETY: The superview of a cell view is its row view, or nothing.
        let selected = unsafe { self.superview() }
            .and_then(|v| v.downcast::<NSTableRowView>().ok())
            .is_some_and(|row| row.isSelected());
        let (text, icon) = if style == NSBackgroundStyle::Emphasized {
            let white = NSColor::alternateSelectedControlTextColor();
            (white.clone(), white)
        } else if selected {
            (NSColor::controlAccentColor(), NSColor::controlAccentColor())
        } else {
            (NSColor::labelColor(), NSColor::secondaryLabelColor())
        };
        if let Some(t) = self.ivars().text.get() {
            t.setTextColor(Some(&text));
        }
        if let Some(i) = self.ivars().image.get() {
            i.setContentTintColor(Some(&icon));
        }
    }
}

define_class!(
    // SAFETY:
    // - NSOutlineView has no subclass requirements.
    // - SidebarOutline does not implement Drop.
    #[unsafe(super(NSOutlineView, NSTableView, NSControl, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BFSidebarOutline"]
    pub struct SidebarOutline;

    impl SidebarOutline {
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            sidebar_key(event);
        }

        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            sidebar_key_equivalent(self, event)
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            // The selection goes back to the current folder.
            app::after_now(app::sync_sidebar_selection);
            // SAFETY: NSResponder implements this method with this signature.
            let resigned: bool = unsafe { msg_send![super(self), resignFirstResponder] };
            resigned
        }
    }
);

/// A key with `cmd`, with the focus in the sidebar: the keymap gets it.
fn sidebar_key_equivalent(outline: &SidebarOutline, event: &NSEvent) -> bool {
    let is_first = outline
        .window()
        .and_then(|w| w.firstResponder())
        .is_some_and(|r| {
            std::ptr::eq(
                Retained::as_ptr(&r).cast::<u8>(),
                std::ptr::from_ref(outline).cast::<u8>(),
            )
        });
    is_first
        && event
            .modifierFlags()
            .contains(NSEventModifierFlags::Command)
        && list_view::handle_ns_key(event)
}

/// A key with the focus in the sidebar. True when the sidebar used it;
/// else the key goes to the normal keymap.
pub fn handle_key(event: &KeyEvent) -> bool {
    let Some(outline) = with_controller(|c| c.ivars().outline.get().cloned()).flatten() else {
        return false;
    };
    let m = event.mods;
    if m.cmd || m.ctrl || m.alt {
        return false;
    }
    let plain_char = |c: char| !m.shift && event.base == Key::Char(c);
    let named = |n: NamedKey| event.base == Key::Named(n);
    if named(NamedKey::Down) || plain_char('j') {
        move_selection(&outline, 1);
        return true;
    }
    if named(NamedKey::Up) || plain_char('k') {
        move_selection(&outline, -1);
        return true;
    }
    if named(NamedKey::Enter) || named(NamedKey::Right) || plain_char('l') {
        let row = outline.selectedRow();
        if let Some(path) = with_controller(|c| c.path_at_row(&outline, row)).flatten() {
            app::sidebar_open(path);
        }
        return true;
    }
    if named(NamedKey::Escape) || named(NamedKey::Tab) || plain_char('h') {
        app::focus_list();
        return true;
    }
    false
}

/// The keys with the focus in the sidebar (from `keyDown:`).
fn sidebar_key(event: &NSEvent) {
    let Some(key) = list_view::key_event_from(event) else {
        return;
    };
    if !handle_key(&key) {
        app::handle_key(key, event.isARepeat());
    }
}

/// Moves the selection by `step` rows. Group rows do not take the selection.
fn move_selection(outline: &SidebarOutline, step: isize) {
    let count = outline.numberOfRows();
    let mut row = outline.selectedRow();
    if row < 0 {
        row = if step > 0 { -1 } else { count };
    }
    loop {
        row += step as NSInteger;
        if row < 0 || row >= count {
            return;
        }
        let selectable =
            with_controller(|c| c.path_at_row(outline, row).is_some()).unwrap_or(false);
        if selectable {
            let set = NSIndexSet::indexSetWithIndex(row as usize);
            outline.selectRowIndexes_byExtendingSelection(&set, false);
            outline.scrollRowToVisible(row);
            return;
        }
    }
}

pub struct ControllerIvars {
    model: RefCell<Model>,
    /// The outline items: the objects that the outline view holds.
    items: RefCell<Vec<(Retained<NSString>, Item)>>,
    outline: OnceCell<Retained<SidebarOutline>>,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclass requirements.
    // - SidebarController does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BFSidebarController"]
    #[ivars = ControllerIvars]
    pub struct SidebarController;

    // SAFETY: `NSObjectProtocol` has no safety requirements. The signatures match.
    unsafe impl NSObjectProtocol for SidebarController {}

    // SAFETY: `NSTableViewDataSource` has no safety requirements. The signatures match.
    unsafe impl NSTableViewDataSource for SidebarController {}

    // SAFETY: `NSOutlineViewDataSource` has no safety requirements. The signatures match.
    unsafe impl NSOutlineViewDataSource for SidebarController {
        #[unsafe(method(outlineView:numberOfChildrenOfItem:))]
        fn number_of_children(&self, _view: &NSOutlineView, item: Option<&AnyObject>) -> NSInteger {
            let model = self.ivars().model.borrow();
            match item.and_then(|i| self.item_of(i)) {
                None => self.groups(&model).len() as NSInteger,
                Some(Item::Group(g)) => group_rows(&model, g).len() as NSInteger,
                Some(Item::Row(..)) => 0,
            }
        }

        #[unsafe(method_id(outlineView:child:ofItem:))]
        fn child(
            &self,
            _view: &NSOutlineView,
            index: NSInteger,
            item: Option<&AnyObject>,
        ) -> Retained<AnyObject> {
            self.child_object(index, item)
        }

        #[unsafe(method(outlineView:isItemExpandable:))]
        fn is_expandable(&self, _view: &NSOutlineView, item: &AnyObject) -> bool {
            matches!(self.item_of(item), Some(Item::Group(_)))
        }
    }

    // SAFETY: `NSControlTextEditingDelegate` has no safety requirements. The signatures match.
    unsafe impl NSControlTextEditingDelegate for SidebarController {}

    // SAFETY: `NSOutlineViewDelegate` has no safety requirements. The signatures match.
    unsafe impl NSOutlineViewDelegate for SidebarController {
        #[unsafe(method_id(outlineView:viewForTableColumn:item:))]
        fn view_for_item(
            &self,
            _view: &NSOutlineView,
            _column: Option<&NSTableColumn>,
            item: &AnyObject,
        ) -> Option<Retained<NSView>> {
            self.make_view(item)
        }

        #[unsafe(method(outlineView:isGroupItem:))]
        fn is_group(&self, _view: &NSOutlineView, item: &AnyObject) -> bool {
            matches!(self.item_of(item), Some(Item::Group(_)))
        }

        #[unsafe(method(outlineView:shouldSelectItem:))]
        fn should_select(&self, _view: &NSOutlineView, item: &AnyObject) -> bool {
            matches!(self.item_of(item), Some(Item::Row(..)))
        }

        #[unsafe(method(outlineView:shouldShowOutlineCellForItem:))]
        fn should_show_outline_cell(&self, _view: &NSOutlineView, _item: &AnyObject) -> bool {
            false
        }
    }

    impl SidebarController {
        #[unsafe(method(rowClicked:))]
        fn row_clicked(&self, _sender: Option<&AnyObject>) {
            let Some(outline) = self.ivars().outline.get() else {
                return;
            };
            let row = outline.clickedRow();
            if let Some(path) = self.path_at_row(outline, row) {
                app::sidebar_open(path);
            }
        }

        #[unsafe(method(volumesChanged:))]
        fn volumes_changed(&self, _notification: &NSNotification) {
            app::rebuild_sidebar();
        }
    }
);

fn group_rows(model: &Model, group: usize) -> &[Row] {
    if group == 0 {
        &model.favorites
    } else {
        &model.locations
    }
}

impl SidebarController {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ControllerIvars {
            model: RefCell::new(Model::default()),
            items: RefCell::new(Vec::new()),
            outline: OnceCell::new(),
        });
        // SAFETY: `init` is the designated initializer of NSObject.
        unsafe { msg_send![super(this), init] }
    }

    fn child_object(&self, index: NSInteger, item: Option<&AnyObject>) -> Retained<AnyObject> {
        let model = self.ivars().model.borrow();
        let wanted = match item.and_then(|i| self.item_of(i)) {
            None => self
                .groups(&model)
                .get(index as usize)
                .map(|g| Item::Group(*g)),
            Some(Item::Group(g)) => Some(Item::Row(g, index as usize)),
            Some(Item::Row(..)) => None,
        };
        drop(model);
        self.object_for(wanted.unwrap_or(Item::Group(0)))
    }

    fn make_view(&self, item: &AnyObject) -> Option<Retained<NSView>> {
        let mtm = self.mtm();
        match self.item_of(item)? {
            Item::Group(g) => {
                let title = if g == 0 { "Favorites" } else { "Locations" };
                let text = NSTextField::labelWithString(&NSString::from_str(title), mtm);
                // SAFETY: The weight is an immutable static from AppKit.
                let weight = unsafe { NSFontWeightSemibold };
                text.setFont(Some(&NSFont::systemFontOfSize_weight(11.0, weight)));
                text.setTextColor(Some(&NSColor::tertiaryLabelColor()));
                let cell = NSTableCellView::new(mtm);
                text.setFrame(NSRect::new(
                    NSPoint::new(2.0, 2.0),
                    NSSize::new(150.0, 15.0),
                ));
                text.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
                cell.addSubview(&text);
                // SAFETY: The cell keeps the text field alive.
                unsafe { cell.setTextField(Some(&text)) };
                Some(Retained::into_super(cell))
            }
            Item::Row(g, i) => {
                let model = self.ivars().model.borrow();
                let row = group_rows(&model, g).get(i)?.clone();
                drop(model);
                let cell = SidebarCell::new(mtm, &row);
                Some(Retained::into_super(Retained::into_super(cell)))
            }
        }
    }

    /// The groups that have rows.
    fn groups(&self, model: &Model) -> Vec<usize> {
        [0, 1]
            .into_iter()
            .filter(|g| !group_rows(model, *g).is_empty())
            .collect()
    }

    fn item_of(&self, object: &AnyObject) -> Option<Item> {
        let items = self.ivars().items.borrow();
        items
            .iter()
            .find(|(o, _)| {
                std::ptr::eq(
                    Retained::as_ptr(o).cast::<u8>(),
                    (object as *const AnyObject).cast::<u8>(),
                )
            })
            .map(|(_, item)| *item)
    }

    /// The object for `item`. The outline view compares items by address,
    /// so the same item always gives the same object.
    fn object_for(&self, item: Item) -> Retained<AnyObject> {
        let mut items = self.ivars().items.borrow_mut();
        if let Some((o, _)) = items.iter().find(|(_, i)| *i == item) {
            return Retained::into_super(Retained::into_super(o.clone()));
        }
        let object = NSString::from_str(&format!("{item:?}"));
        items.push((object.clone(), item));
        Retained::into_super(Retained::into_super(object))
    }

    fn path_at_row(&self, outline: &NSOutlineView, row: NSInteger) -> Option<PathBuf> {
        if row < 0 {
            return None;
        }
        let object = outline.itemAtRow(row)?;
        let Item::Row(g, i) = self.item_of(&object)? else {
            return None;
        };
        let model = self.ivars().model.borrow();
        group_rows(&model, g).get(i).map(|r| r.path.clone())
    }

    /// Shows a new model. Call it only when no borrow of the app state is
    /// active: `reloadData` calls the data source at once.
    fn set_model(&self, model: Model) {
        if *self.ivars().model.borrow() == model {
            return;
        }
        *self.ivars().model.borrow_mut() = model;
        self.ivars().items.borrow_mut().clear();
        if let Some(outline) = self.ivars().outline.get() {
            outline.reloadData();
            // SAFETY: `None` means all items.
            unsafe { outline.expandItem_expandChildren(None, true) };
        }
    }

    /// Selects the row of `path`, or nothing when no row matches.
    fn select_path(&self, path: &Path) {
        let Some(outline) = self.ivars().outline.get() else {
            return;
        };
        let found = {
            let model = self.ivars().model.borrow();
            [0, 1].into_iter().find_map(|g| {
                group_rows(&model, g)
                    .iter()
                    .position(|r| r.path == path)
                    .map(|i| Item::Row(g, i))
            })
        };
        let row = found.map(|item| {
            let object = self.object_for(item);
            // SAFETY: `object` is an item of this outline view.
            unsafe { outline.rowForItem(Some(&object)) }
        });
        match row {
            Some(row) if row >= 0 => {
                let set = NSIndexSet::indexSetWithIndex(row as usize);
                outline.selectRowIndexes_byExtendingSelection(&set, false);
            }
            // SAFETY: `None` is a valid sender.
            _ => unsafe { outline.deselectAll(None) },
        }
    }
}

thread_local! {
    static CONTROLLER: OnceCell<Retained<SidebarController>> = const { OnceCell::new() };
}

fn with_controller<R>(f: impl FnOnce(&SidebarController) -> R) -> Option<R> {
    CONTROLLER.with(|c| c.get().map(|c| f(c)))
}

/// The sidebar views.
#[derive(Clone)]
pub struct SidebarUi {
    pub scroll: Retained<NSScrollView>,
    pub outline: Retained<SidebarOutline>,
}

/// Makes the sidebar views and the controller.
pub fn create(mtm: MainThreadMarker) -> SidebarUi {
    let controller = SidebarController::new(mtm);
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(180.0, 400.0));
    let outline: Retained<SidebarOutline> = {
        let this = SidebarOutline::alloc(mtm).set_ivars(());
        // SAFETY: `initWithFrame:` is the designated initializer of NSView.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    };
    let column =
        NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), &NSString::from_str("name"));
    column.setWidth(170.0);
    outline.addTableColumn(&column);
    // SAFETY: The column belongs to the outline view.
    unsafe { outline.setOutlineTableColumn(Some(&column)) };
    outline.setHeaderView(None);
    outline.setStyle(NSTableViewStyle::SourceList);
    outline.setRowSizeStyle(NSTableViewRowSizeStyle::Default);
    outline.setFloatsGroupRows(false);
    outline.setIndentationPerLevel(0.0);
    outline.setAllowsEmptySelection(true);
    // SAFETY: The controller lives in a thread-local for the whole life of
    // the app, so the weak references stay valid.
    unsafe {
        outline.setDataSource(Some(ProtocolObject::from_ref(&*controller)));
        outline.setDelegate(Some(ProtocolObject::from_ref(&*controller)));
        outline.setTarget(Some(&controller));
        outline.setAction(Some(sel!(rowClicked:)));
    }
    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
    scroll.setDocumentView(Some(&outline));
    scroll.setDrawsBackground(false);
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    let _ = controller.ivars().outline.set(outline.clone());

    // Volume changes rebuild the Locations group.
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: The notification names are immutable statics. The observer
    // lives for the whole life of the app.
    unsafe {
        for name in [
            NSWorkspaceDidMountNotification,
            NSWorkspaceDidUnmountNotification,
            NSWorkspaceDidRenameVolumeNotification,
        ] {
            center.addObserver_selector_name_object(
                &controller,
                sel!(volumesChanged:),
                Some(name),
                None,
            );
        }
    }
    CONTROLLER.with(|c| {
        let _ = c.set(controller);
    });
    SidebarUi { scroll, outline }
}

/// A model that a thread builds. The main thread can take it without a wait.
pub struct PendingModel(Arc<Mutex<Option<Model>>>);

/// Builds the model on a thread. The result goes to the main queue
/// (`apply_model`). `PendingModel::try_take` can take it earlier.
pub fn spawn_build(items: Vec<PathBuf>, locations: bool) -> PendingModel {
    let slot: Arc<Mutex<Option<Model>>> = Arc::new(Mutex::new(None));
    let thread_slot = slot.clone();
    let spawned = std::thread::Builder::new()
        .name("bf-sidebar".into())
        .spawn(move || {
            let started = std::time::Instant::now();
            let model = build_model(&items, locations);
            trace!(
                "sidebar: {} rows in {} µs",
                model.favorites.len() + model.locations.len(),
                started.elapsed().as_micros()
            );
            *thread_slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(model);
            app::after_now(move || {
                let model = thread_slot
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                if let Some(model) = model {
                    apply_model(model);
                }
            });
        });
    if spawned.is_err() {
        trace!("sidebar: cannot start a thread");
    }
    PendingModel(slot)
}

impl PendingModel {
    /// The model, when the thread is done. Never waits.
    pub fn try_take(&self) -> Option<Model> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// Shows `model` and selects the current folder.
pub fn apply_model(model: Model) {
    with_controller(|c| c.set_model(model));
    app::sync_sidebar_selection();
}

/// Selects the row of `path`. Call it without a borrow of the app state.
pub fn select_path(path: &Path) {
    with_controller(|c| c.select_path(path));
}
