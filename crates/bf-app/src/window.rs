//! The `NSWindow`, the split view (sidebar and content), and the layout of
//! the content: the column header, the list (in a scroll view), the prompt,
//! the path and status bar, and the help overlay.
//!
//! The window has a full-size content view. The list scrolls under the
//! toolbar; the column header stays pinned below it. `RootView` reads the
//! safe area for the height that the title bar and the toolbar cover.

use std::cell::{Cell, OnceCell};
use std::sync::OnceLock;

use bf_core::config::{Config, ThemeMode, TitlebarStyle};
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSApplication, NSBackingStoreType, NSBorderType, NSColor, NSResponder, NSScrollView,
    NSSplitViewController, NSSplitViewItem, NSTitlebarSeparatorStyle, NSView, NSViewController,
    NSWindow, NSWindowStyleMask, NSWindowTabbingMode, NSWindowTitleVisibility,
    NSWindowToolbarStyle,
};
use objc2_foundation::{
    MainThreadMarker, NSEdgeInsets, NSObject, NSPoint, NSRect, NSSize, NSString, NSUserDefaults,
    ns_string,
};

use crate::app;
use crate::goto_panel::{self, GotoPanel};
use crate::header::HeaderView;
use crate::help::{self, HelpUi};
use crate::list_view::ListView;
use crate::prompt::{self, PromptDelegate, PromptUi};
use crate::sidebar::{self, SidebarUi};
use crate::status_bar::StatusBar;
use crate::theme::Style;
use crate::toolbar::{self, ToolbarUi};

const SPLIT_AUTOSAVE: &str = "BetterFinderSplit";

#[derive(Default)]
pub struct RootIvars {
    scroll: OnceCell<Retained<NSScrollView>>,
    list: OnceCell<Retained<ListView>>,
    header: OnceCell<Retained<HeaderView>>,
    status: OnceCell<Retained<StatusBar>>,
    prompt: OnceCell<Retained<prompt::PromptBar>>,
    help: OnceCell<HelpUi>,
    goto: OnceCell<Retained<GotoPanel>>,
    goto_height: Cell<f64>,
    header_height: Cell<f64>,
    status_height: Cell<f64>,
    prompt_height: Cell<f64>,
    prompt_visible: Cell<bool>,
    help_visible: Cell<bool>,
    help_height: Cell<f64>,
    /// The top inset of the last layout, to scroll only when it changes.
    last_top: Cell<f64>,
}

define_class!(
    // SAFETY:
    // - NSView has no subclass requirements.
    // - RootView does not implement Drop.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BFRootView"]
    #[ivars = RootIvars]
    pub struct RootView;

    impl RootView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(layout))]
        fn layout(&self) {
            // SAFETY: NSView implements this method with this signature.
            let _: () = unsafe { msg_send![super(self), layout] };
            self.layout_children();
        }

        #[unsafe(method(resizeSubviewsWithOldSize:))]
        fn resize_subviews_with_old_size(&self, _old: NSSize) {
            self.layout_children();
        }

        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn view_did_change_effective_appearance(&self) {
            // AppKit can call this in the middle of our own changes. Do the
            // work later on the main queue.
            app::after_now(app::appearance_changed);
        }
    }
);

impl RootView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RootIvars::default());
        // SAFETY: `initWithFrame:` is the designated initializer of NSView.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    pub fn set_heights(&self, header: f64, status: f64, prompt: f64) {
        let iv = self.ivars();
        iv.header_height.set(header);
        iv.status_height.set(status);
        iv.prompt_height.set(prompt);
        self.layout_children();
    }

    pub fn set_prompt_visible(&self, visible: bool) {
        if self.ivars().prompt_visible.replace(visible) != visible {
            self.layout_children();
        }
    }

    /// Shows the "go to" panel with `height`, or hides it (`None`).
    pub fn set_goto(&self, height: Option<f64>) {
        self.ivars().goto_height.set(height.unwrap_or(0.0));
        self.layout_children();
    }

    /// Shows or hides the help overlay. `height` is the content height.
    pub fn set_help(&self, visible: bool, height: f64) {
        self.ivars().help_visible.set(visible);
        self.ivars().help_height.set(height);
        self.layout_children();
    }

    /// From the bottom: the path and status bar, the prompt (when visible),
    /// and the list area. The scroll view fills the list area up to the top
    /// of the view; its content inset keeps the rows below the toolbar and
    /// the column header. The help overlay covers the visible list area.
    pub fn layout_children(&self) {
        let iv = self.ivars();
        let size = self.bounds().size;
        let (w, h) = (size.width, size.height);
        let top = self.safeAreaInsets().top.max(0.0);
        let status_h = iv.status_height.get().min(h);
        let prompt_h = if iv.prompt_visible.get() {
            iv.prompt_height.get()
        } else {
            0.0
        };
        let prompt_h = prompt_h.min((h - status_h).max(0.0));
        let list_bottom = (h - status_h - prompt_h).max(0.0);
        let header_h = iv.header_height.get();
        if let Some(status) = iv.status.get() {
            status.setFrame(rect(0.0, h - status_h, w, status_h));
        }
        if let Some(prompt) = iv.prompt.get() {
            prompt.setHidden(prompt_h <= 0.0);
            prompt.setFrame(rect(0.0, list_bottom, w, iv.prompt_height.get()));
        }
        if let Some(header) = iv.header.get() {
            header.setHidden(header_h <= 0.0);
            header.setFrame(rect(0.0, top, w, header_h));
        }
        let inset = top + header_h;
        if let Some(scroll) = iv.scroll.get() {
            scroll.setFrame(rect(0.0, 0.0, w, list_bottom));
            let insets = NSEdgeInsets {
                top: inset,
                left: 0.0,
                bottom: 0.0,
                right: 0.0,
            };
            scroll.setContentInsets(insets);
        }
        if let Some(list) = iv.list.get() {
            list.set_inset_top(inset);
            list.update_frame();
            if iv.last_top.replace(inset) != inset {
                // The covered height changed: keep the cursor in view.
                app::after_now(app::reveal_cursor);
            }
        }
        if let Some(panel) = iv.goto.get() {
            let height = iv.goto_height.get();
            panel.setHidden(height <= 0.0);
            if height > 0.0 {
                let width = (w - 40.0).clamp(0.0, 560.0);
                panel.setFrame(rect(
                    ((w - width) / 2.0).round(),
                    inset + 10.0,
                    width,
                    height,
                ));
            }
        }
        if let Some(help) = iv.help.get() {
            let visible = iv.help_visible.get();
            help.scroll.setHidden(!visible);
            if visible {
                help.scroll
                    .setFrame(rect(0.0, inset, w, (list_bottom - inset).max(0.0)));
                help.view.set_content_height(iv.help_height.get());
            }
        }
    }
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

/// All the views of the window. Cheap to clone (reference counts).
#[derive(Clone)]
pub struct Ui {
    pub window: Retained<NSWindow>,
    pub _split: Retained<NSSplitViewController>,
    pub sidebar_item: Retained<NSSplitViewItem>,
    pub root: Retained<RootView>,
    pub scroll: Retained<NSScrollView>,
    pub list: Retained<ListView>,
    pub header: Retained<HeaderView>,
    pub status: Retained<StatusBar>,
    pub prompt: PromptUi,
    pub help: HelpUi,
    pub goto: Retained<GotoPanel>,
    pub sidebar: SidebarUi,
    pub toolbar: ToolbarUi,
}

/// Makes the window. `create_views` fills it and sets its size.
pub fn create_window(mtm: MainThreadMarker, config: &Config) -> Retained<NSWindow> {
    let frame = rect(
        0.0,
        0.0,
        config.window_width.max(360.0),
        config.window_height.max(220.0),
    );
    let mask = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::FullSizeContentView;
    // SAFETY: The arguments are valid. A window without a window controller
    // needs `setReleasedWhenClosed(false)`, which follows below.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            mask,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: The app keeps a strong reference to the window in its state.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTabbingMode(NSWindowTabbingMode::Disallowed);
    window
}

fn view_controller(mtm: MainThreadMarker, view: &NSView) -> Retained<NSViewController> {
    let vc = NSViewController::initWithNibName_bundle(NSViewController::alloc(mtm), None, None);
    vc.setView(view);
    vc
}

/// Makes all views and puts them in `window`. With `autosave`, the window
/// frame and the sidebar width persist in the user defaults.
pub fn create_views(
    mtm: MainThreadMarker,
    window: &Retained<NSWindow>,
    config: &Config,
    autosave: bool,
) -> Ui {
    let size = NSSize::new(
        config.window_width.max(360.0),
        config.window_height.max(220.0),
    );
    let frame = rect(0.0, 0.0, size.width, size.height);
    let root = RootView::new(mtm, frame);

    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    scroll.setDrawsBackground(false);
    scroll.setBorderType(NSBorderType::NoBorder);
    scroll.setAutomaticallyAdjustsContentInsets(false);
    let list = ListView::new(mtm, frame);
    scroll.setDocumentView(Some(&list));

    let header = HeaderView::new(mtm);
    let status = StatusBar::new(mtm);
    let delegate = PromptDelegate::new(mtm);
    let prompt = prompt::create(mtm, &delegate);
    let help = help::create(mtm);
    let goto = goto_panel::create(mtm, &delegate);
    goto_panel::set_shadow(&goto);

    root.addSubview(&scroll);
    root.addSubview(&header);
    root.addSubview(&prompt.bar);
    root.addSubview(&status);
    root.addSubview(&help.scroll);
    root.addSubview(&goto);
    let iv = root.ivars();
    let _ = iv.scroll.set(scroll.clone());
    let _ = iv.list.set(list.clone());
    let _ = iv.header.set(header.clone());
    let _ = iv.status.set(status.clone());
    let _ = iv.prompt.set(prompt.bar.clone());
    let _ = iv.help.set(help.clone());
    let _ = iv.goto.set(goto.clone());

    let sidebar = sidebar::create(mtm);
    let split = NSSplitViewController::new(mtm);
    let sidebar_item =
        NSSplitViewItem::sidebarWithViewController(&view_controller(mtm, &sidebar.scroll));
    sidebar_item.setMinimumThickness(120.0);
    sidebar_item.setMaximumThickness(400.0);
    sidebar_item.setCanCollapse(true);
    let content_item =
        NSSplitViewItem::splitViewItemWithViewController(&view_controller(mtm, &root));
    split.addSplitViewItem(&sidebar_item);
    split.addSplitViewItem(&content_item);
    let saved_split = autosave && has_saved_split();
    if autosave {
        split
            .splitView()
            .setAutosaveName(Some(&NSString::from_str(SPLIT_AUTOSAVE)));
    }
    sidebar_item.setCollapsed(!config.sidebar);

    // Each change of the window makes AppKit lay out all views again. So
    // the toolbar comes first, and the split view gets its final size
    // before it goes into the window.
    let toolbar = toolbar::create(mtm, &delegate);
    if config.toolbar {
        window.setToolbar(Some(&toolbar.toolbar));
        window.setToolbarStyle(NSWindowToolbarStyle::Unified);
    }
    // The split view takes the frames of its panes: give them their final
    // widths now, so that no resize pass follows.
    let sidebar_width = if config.sidebar {
        config.sidebar_width
    } else {
        0.0
    };
    sidebar
        .scroll
        .setFrameSize(NSSize::new(sidebar_width, size.height));
    root.setFrameSize(NSSize::new(
        (size.width - sidebar_width).max(0.0),
        size.height,
    ));
    split.view().setFrameSize(size);
    window.setContentViewController(Some(&split));
    if window.contentRectForFrameRect(window.frame()).size != size {
        window.setContentSize(size);
    }
    if !saved_split && config.sidebar {
        split
            .splitView()
            .setPosition_ofDividerAtIndex(config.sidebar_width, 0);
    }
    window.setContentMinSize(NSSize::new(360.0, 220.0));
    window.center();
    if autosave {
        let name = ns_string!("BetterFinderMainWindow");
        window.setFrameUsingName(name);
        window.setFrameAutosaveName(name);
    }
    window.setInitialFirstResponder(Some(&list));
    Ui {
        window: window.clone(),
        _split: split,
        sidebar_item,
        root,
        scroll,
        list,
        header,
        status,
        prompt,
        help,
        goto,
        sidebar,
        toolbar,
    }
}

/// True when the user defaults have a saved sidebar width.
fn has_saved_split() -> bool {
    let key = NSString::from_str(&format!("NSSplitView Subview Frames {SPLIT_AUTOSAVE}"));
    NSUserDefaults::standardUserDefaults()
        .objectForKey(&key)
        .is_some()
}

static FORCED_APPEARANCE: OnceLock<Option<bool>> = OnceLock::new();

/// Forces the light (`false`) or dark (`true`) appearance, for screenshots.
/// Call it before the first `apply_appearance`.
pub fn force_appearance(dark: Option<bool>) {
    let _ = FORCED_APPEARANCE.set(dark);
}

/// Sets the appearance of the app and the window for the theme mode, so
/// that the title bar and the Quick Look panel match. `system` and `auto`
/// follow the system. Call it before `Style::new`, so that the effective
/// appearance is right. AppKit calls `viewDidChangeEffectiveAppearance` on
/// the views; the root view defers its work, so a borrow of the app state
/// is no problem.
pub fn apply_appearance(window: &NSWindow, mode: ThemeMode) {
    let dark = match mode {
        ThemeMode::System | ThemeMode::Auto => FORCED_APPEARANCE.get().copied().flatten(),
        ThemeMode::Light => Some(false),
        ThemeMode::Dark => Some(true),
    };
    // SAFETY: The appearance names are immutable statics from AppKit.
    let name = dark.map(|dark| unsafe {
        if dark {
            NSAppearanceNameDarkAqua
        } else {
            NSAppearanceNameAqua
        }
    });
    let appearance = name.and_then(NSAppearance::appearanceNamed);
    NSApplication::sharedApplication(window.mtm()).setAppearance(appearance.as_deref());
    window.setAppearance(appearance.as_deref());
}

/// Applies the style, the bars, and the title bar style to the window and
/// its views. Call it only when no borrow of the app state is active: it
/// changes frames, and AppKit can call back into the views.
pub fn apply_look(ui: &Ui, style: &Style, config: &Config) {
    let window = &ui.window;
    window.setOpaque(style.opacity >= 1.0);
    if style.system {
        // The scroll view draws the list background; the sidebar glass
        // shows the window background.
        window.setBackgroundColor(Some(
            &NSColor::windowBackgroundColor().colorWithAlphaComponent(style.opacity),
        ));
        ui.scroll.setDrawsBackground(true);
        ui.scroll.setBackgroundColor(&style.colors.list_bg);
    } else {
        // The custom palette: the window background is the list background.
        window.setBackgroundColor(Some(&style.colors.list_bg));
        ui.scroll.setDrawsBackground(false);
    }
    if config.toolbar {
        window.setToolbar(Some(&ui.toolbar.toolbar));
        window.setToolbarStyle(NSWindowToolbarStyle::Unified);
        window.setTitlebarAppearsTransparent(false);
        window.setTitleVisibility(if config.titlebar_style == TitlebarStyle::Hidden {
            NSWindowTitleVisibility::Hidden
        } else {
            NSWindowTitleVisibility::Visible
        });
        window.setTitlebarSeparatorStyle(NSTitlebarSeparatorStyle::Automatic);
    } else {
        window.setToolbar(None);
        let (transparent, visibility) = match config.titlebar_style {
            TitlebarStyle::Native => (false, NSWindowTitleVisibility::Visible),
            TitlebarStyle::Transparent => (true, NSWindowTitleVisibility::Visible),
            TitlebarStyle::Hidden => (true, NSWindowTitleVisibility::Hidden),
        };
        window.setTitlebarAppearsTransparent(transparent);
        window.setTitleVisibility(visibility);
        window.setTitlebarSeparatorStyle(if transparent {
            NSTitlebarSeparatorStyle::None
        } else {
            NSTitlebarSeparatorStyle::Automatic
        });
    }
    window.invalidateShadow();

    ui.prompt.apply_style(style);
    let header_h = if config.column_header {
        style.header_height
    } else {
        0.0
    };
    ui.root
        .set_heights(header_h, style.bar_height, style.prompt_height);
    ui.status.setNeedsDisplay(true);
    ui.header.setNeedsDisplay(true);
    ui.list.setNeedsDisplay(true);
    ui.help.view.setNeedsDisplay(true);
}
