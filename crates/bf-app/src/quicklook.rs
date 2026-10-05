//! The data source and the delegate of the macOS Quick Look panel.
//!
//! `ListView` accepts control of the panel (it is in the responder chain).
//! When the panel starts to use it, `begin_control` connects the panel to
//! the one `QuickLookController` of the app.
//!
//! Re-entrancy: the panel calls the data source synchronously from
//! `reloadData`, `makeKeyAndOrderFront:`, and `updateController`. The app
//! calls these methods only when it holds no borrow of the app state.

use std::cell::OnceCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSEvent, NSEventType, NSWindowDelegate};
use objc2_foundation::{MainThreadMarker, NSInteger, NSObject, NSObjectProtocol, NSRect, NSURL};
use objc2_quick_look_ui::{
    QLPreviewItem, QLPreviewPanel, QLPreviewPanelDataSource, QLPreviewPanelDelegate,
};

use crate::app;
use crate::debug::trace;
use crate::icons;
use crate::list_view;

define_class!(
    // SAFETY:
    // - NSObject has no subclass requirements.
    // - QuickLookController does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BFQuickLookController"]
    pub struct QuickLookController;

    // SAFETY: `NSObjectProtocol` has no safety requirements. The signatures match.
    unsafe impl NSObjectProtocol for QuickLookController {}

    // SAFETY: `QLPreviewPanelDataSource` has no safety requirements. The signatures match.
    unsafe impl QLPreviewPanelDataSource for QuickLookController {
        #[unsafe(method(numberOfPreviewItemsInPreviewPanel:))]
        fn number_of_preview_items(&self, _panel: Option<&QLPreviewPanel>) -> NSInteger {
            NSInteger::from(current_url().is_some())
        }

        #[unsafe(method_id(previewPanel:previewItemAtIndex:))]
        fn preview_item_at_index(
            &self,
            _panel: Option<&QLPreviewPanel>,
            _index: NSInteger,
        ) -> Option<Retained<ProtocolObject<dyn QLPreviewItem>>> {
            let url = current_url();
            trace!("quicklook: item {:?}", url.as_ref().and_then(|u| u.path()));
            url.map(ProtocolObject::from_retained)
        }
    }

    // SAFETY: `NSWindowDelegate` has no safety requirements. The signatures match.
    unsafe impl NSWindowDelegate for QuickLookController {}

    // SAFETY: `QLPreviewPanelDelegate` has no safety requirements. The signatures match.
    unsafe impl QLPreviewPanelDelegate for QuickLookController {
        #[unsafe(method(previewPanel:handleEvent:))]
        fn preview_panel_handle_event(
            &self,
            _panel: Option<&QLPreviewPanel>,
            event: Option<&NSEvent>,
        ) -> bool {
            // Key presses in the panel go through the keymap, like in the
            // list. So `j` and `k` change the preview, `space` closes it.
            match event {
                Some(event) if event.r#type() == NSEventType::KeyDown => {
                    list_view::handle_ns_key(event)
                }
                _ => false,
            }
        }

        #[unsafe(method(previewPanel:sourceFrameOnScreenForPreviewItem:))]
        fn preview_panel_source_frame(
            &self,
            _panel: Option<&QLPreviewPanel>,
            _item: Option<&ProtocolObject<dyn QLPreviewItem>>,
        ) -> NSRect {
            cursor_screen_rect().unwrap_or(NSRect::ZERO)
        }

        #[unsafe(method_id(previewPanel:transitionImageForPreviewItem:contentRect:))]
        fn preview_panel_transition_image(
            &self,
            _panel: Option<&QLPreviewPanel>,
            _item: Option<&ProtocolObject<dyn QLPreviewItem>>,
            _content_rect: *mut NSRect,
        ) -> Option<Retained<AnyObject>> {
            transition_image()
        }
    }
);

impl QuickLookController {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: `init` is the designated initializer of NSObject.
        unsafe { msg_send![super(this), init] }
    }
}

thread_local! {
    static CONTROLLER: OnceCell<Retained<QuickLookController>> = const { OnceCell::new() };
}

fn controller(mtm: MainThreadMarker) -> Retained<QuickLookController> {
    CONTROLLER.with(|c| c.get_or_init(|| QuickLookController::new(mtm)).clone())
}

/// The file URL of the entry under the cursor.
fn current_url() -> Option<Retained<NSURL>> {
    let path = app::with_ref(|s| s.pane.current().map(|e| e.path.clone())).flatten()?;
    NSURL::from_file_path(path)
}

/// The icon of the cursor row on the screen, for the zoom animation.
fn cursor_screen_rect() -> Option<NSRect> {
    let (list, row, icon) = app::with_ref(|s| {
        let icon = s.style.show_icons.then_some(s.style.icon_size);
        (s.ui.list.clone(), s.pane.cursor(), icon)
    })?;
    list.screen_rect_of_row(row, icon)
}

/// The icon of the entry under the cursor, for the zoom animation.
fn transition_image() -> Option<Retained<AnyObject>> {
    let (entry, local) = app::with_ref(|s| (s.pane.current().cloned(), s.cwd_is_local()))?;
    let icon = icons::icon_for(&entry?, local)?;
    Some(Retained::into_super(Retained::into_super(icon)))
}

/// `ListView::beginPreviewPanelControl:` calls this.
pub fn begin_control(panel: Option<&QLPreviewPanel>) {
    let (Some(panel), Some(mtm)) = (panel, MainThreadMarker::new()) else {
        return;
    };
    trace!("quicklook: begin control");
    let controller = controller(mtm);
    let delegate: &AnyObject = &controller;
    // SAFETY: The controller lives in a thread-local for the whole life of
    // the app, so the unretained references stay valid.
    unsafe {
        panel.setDataSource(Some(ProtocolObject::from_ref(&*controller)));
        panel.setDelegate(Some(delegate));
    }
}

/// `ListView::endPreviewPanelControl:` calls this.
pub fn end_control(panel: Option<&QLPreviewPanel>) {
    let Some(panel) = panel else { return };
    trace!("quicklook: end control");
    // SAFETY: `None` removes the data source and the delegate.
    unsafe {
        panel.setDataSource(None);
        panel.setDelegate(None);
    }
}

/// The shared panel, only when it exists already.
fn existing_panel(mtm: MainThreadMarker) -> Option<Retained<QLPreviewPanel>> {
    // SAFETY: These class methods have no preconditions on the main thread.
    unsafe {
        if !QLPreviewPanel::sharedPreviewPanelExists(mtm) {
            return None;
        }
        QLPreviewPanel::sharedPreviewPanel(mtm)
    }
}

pub fn is_visible(mtm: MainThreadMarker) -> bool {
    existing_panel(mtm).is_some_and(|p| p.isVisible())
}

/// Shows the panel, or hides it when it is visible.
/// Call it only when no borrow of the app state is active.
pub fn toggle(mtm: MainThreadMarker) {
    // SAFETY: `sharedPreviewPanel` has no preconditions on the main thread.
    let Some(panel) = (unsafe { QLPreviewPanel::sharedPreviewPanel(mtm) }) else {
        return;
    };
    if panel.isVisible() {
        panel.orderOut(None);
    } else {
        panel.makeKeyAndOrderFront(None);
    }
}

/// Hides the panel when it is visible.
pub fn close(mtm: MainThreadMarker) {
    if let Some(panel) = existing_panel(mtm) {
        if panel.isVisible() {
            panel.orderOut(None);
        }
    }
}

/// Asks the visible panel to show the entry under the cursor again.
/// Call it only when no borrow of the app state is active.
pub fn reload_if_visible(mtm: MainThreadMarker) {
    if let Some(panel) = existing_panel(mtm) {
        if panel.isVisible() {
            // SAFETY: The data source is the controller, which is alive.
            unsafe { panel.reloadData() };
        }
    }
}
