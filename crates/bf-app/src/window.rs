//! The `NSWindow` and the layout of its views: the list (in a scroll view),
//! the prompt, the status bar, and the help overlay.

use std::cell::{Cell, OnceCell};

use bf_core::config::{Config, ThemeMode, TitlebarStyle};
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSApplication, NSBackingStoreType, NSBorderType, NSResponder, NSScrollView,
    NSTitlebarSeparatorStyle, NSView, NSWindow, NSWindowStyleMask, NSWindowTabbingMode,
    NSWindowTitleVisibility,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, ns_string};

use crate::app;
use crate::help::{self, HelpUi};
use crate::list_view::ListView;
use crate::prompt::{self, PromptUi};
use crate::status_bar::StatusBar;
use crate::theme::Style;

#[derive(Default)]
pub struct RootIvars {
    scroll: OnceCell<Retained<NSScrollView>>,
    list: OnceCell<Retained<ListView>>,
    status: OnceCell<Retained<StatusBar>>,
    prompt: OnceCell<Retained<prompt::PromptBar>>,
    help: OnceCell<HelpUi>,
    status_height: Cell<f64>,
    prompt_height: Cell<f64>,
    prompt_visible: Cell<bool>,
    help_visible: Cell<bool>,
    help_height: Cell<f64>,
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

    pub fn set_heights(&self, status: f64, prompt: f64) {
        self.ivars().status_height.set(status);
        self.ivars().prompt_height.set(prompt);
        self.layout_children();
    }

    pub fn set_prompt_visible(&self, visible: bool) {
        if self.ivars().prompt_visible.replace(visible) != visible {
            self.layout_children();
        }
    }

    /// Shows or hides the help overlay. `height` is the content height.
    pub fn set_help(&self, visible: bool, height: f64) {
        self.ivars().help_visible.set(visible);
        self.ivars().help_height.set(height);
        self.layout_children();
    }

    /// Puts the status bar at the bottom, the prompt above it (when visible),
    /// and the list in the rest. The help overlay covers the list area.
    pub fn layout_children(&self) {
        let iv = self.ivars();
        let size = self.bounds().size;
        let (w, h) = (size.width, size.height);
        let status_h = iv.status_height.get().min(h);
        let prompt_h = if iv.prompt_visible.get() {
            iv.prompt_height.get()
        } else {
            0.0
        };
        let prompt_h = prompt_h.min((h - status_h).max(0.0));
        if let Some(status) = iv.status.get() {
            status.setFrame(rect(0.0, 0.0, w, status_h));
        }
        if let Some(prompt) = iv.prompt.get() {
            prompt.setHidden(prompt_h <= 0.0);
            prompt.setFrame(rect(0.0, status_h, w, iv.prompt_height.get()));
        }
        let list_area = rect(
            0.0,
            status_h + prompt_h,
            w,
            (h - status_h - prompt_h).max(0.0),
        );
        if let Some(scroll) = iv.scroll.get() {
            scroll.setFrame(list_area);
        }
        if let Some(list) = iv.list.get() {
            list.update_frame();
        }
        if let Some(help) = iv.help.get() {
            let visible = iv.help_visible.get();
            help.scroll.setHidden(!visible);
            if visible {
                help.scroll.setFrame(list_area);
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
    pub root: Retained<RootView>,
    pub scroll: Retained<NSScrollView>,
    pub list: Retained<ListView>,
    pub status: Retained<StatusBar>,
    pub prompt: PromptUi,
    pub help: HelpUi,
}

/// Makes the window with the size from the config, centered. With
/// `autosave`, the window restores and saves its frame in the user defaults.
pub fn create_window(mtm: MainThreadMarker, config: &Config, autosave: bool) -> Retained<NSWindow> {
    let frame = rect(
        0.0,
        0.0,
        config.window_width.max(360.0),
        config.window_height.max(220.0),
    );
    let mask = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
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
    window.setContentMinSize(NSSize::new(360.0, 220.0));
    window.setTabbingMode(NSWindowTabbingMode::Disallowed);
    window.center();
    if autosave {
        let name = ns_string!("BetterFinderMainWindow");
        window.setFrameUsingName(name);
        window.setFrameAutosaveName(name);
    }
    window
}

/// Makes all views and puts them in `window`.
pub fn create_views(mtm: MainThreadMarker, window: &Retained<NSWindow>) -> Ui {
    let frame = window
        .contentView()
        .map(|v| v.bounds())
        .unwrap_or_else(|| rect(0.0, 0.0, 800.0, 600.0));
    let root = RootView::new(mtm, frame);

    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    scroll.setDrawsBackground(false);
    scroll.setBorderType(NSBorderType::NoBorder);
    let list = ListView::new(mtm, frame);
    scroll.setDocumentView(Some(&list));

    let status = StatusBar::new(mtm);
    let prompt = prompt::create(mtm);
    let help = help::create(mtm);

    root.addSubview(&scroll);
    root.addSubview(&prompt.bar);
    root.addSubview(&status);
    root.addSubview(&help.scroll);
    let iv = root.ivars();
    let _ = iv.scroll.set(scroll.clone());
    let _ = iv.list.set(list.clone());
    let _ = iv.status.set(status.clone());
    let _ = iv.prompt.set(prompt.bar.clone());
    let _ = iv.help.set(help.clone());

    window.setContentView(Some(&root));
    window.setInitialFirstResponder(Some(&list));
    Ui {
        window: window.clone(),
        root,
        scroll,
        list,
        status,
        prompt,
        help,
    }
}

/// Sets the appearance of the app and the window for the theme mode, so
/// that the title bar and the Quick Look panel match. `auto` follows the
/// system. Call it before `Style::new`, so that the effective appearance is
/// right. AppKit calls `viewDidChangeEffectiveAppearance` on the views; the
/// root view defers its work, so a borrow of the app state is no problem.
pub fn apply_appearance(window: &NSWindow, mode: ThemeMode) {
    // SAFETY: The appearance names are immutable statics from AppKit.
    let name = unsafe {
        match mode {
            ThemeMode::Auto => None,
            ThemeMode::Light => Some(NSAppearanceNameAqua),
            ThemeMode::Dark => Some(NSAppearanceNameDarkAqua),
        }
    };
    let appearance = name.and_then(NSAppearance::appearanceNamed);
    NSApplication::sharedApplication(window.mtm()).setAppearance(appearance.as_deref());
    window.setAppearance(appearance.as_deref());
}

/// Applies the style and the title bar style to the window and its views.
/// Call it only when no borrow of the app state is active: it changes
/// frames, and AppKit can call back into the views.
pub fn apply_look(ui: &Ui, style: &Style, titlebar: TitlebarStyle) {
    let window = &ui.window;
    window.setOpaque(style.opacity >= 1.0);
    window.setBackgroundColor(Some(&style.window_bg));
    let (transparent, visibility) = match titlebar {
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
    window.invalidateShadow();

    ui.prompt.apply_style(style);
    ui.root
        .set_heights(style.status_height, style.prompt_height);
    ui.status.setNeedsDisplay(true);
    ui.list.setNeedsDisplay(true);
    ui.help.view.setNeedsDisplay(true);
}
