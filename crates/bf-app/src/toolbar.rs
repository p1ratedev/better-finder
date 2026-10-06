//! The toolbar: unified style, icons only, no user customization.
//!
//! Items: the sidebar tracking separator, back and forward (one group, in
//! the navigational area), a flexible space, and the filter field
//! (`NSSearchToolbarItem`). The window title shows `display_path(cwd)`
//! after the navigational items.
//!
//! The filter field costs about 25 ms to make and to lay out. The toolbar
//! has its full height without it, so `add_search` inserts it after the
//! first frame: the start is faster and nothing moves.
//!
//! The filter field uses the same delegate as the bottom prompt, so `Enter`,
//! `Escape`, `Up`, and `Down` work the same way. It is also the location
//! field: a path in it goes to a folder (see `app::prompt_changed`).

use std::cell::OnceCell;

use bf_core::Action;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSImage, NSSearchField, NSSearchToolbarItem, NSToolbar, NSToolbarDelegate,
    NSToolbarDisplayMode, NSToolbarFlexibleSpaceItemIdentifier, NSToolbarItem, NSToolbarItemGroup,
    NSToolbarItemGroupSelectionMode, NSToolbarItemIdentifier,
    NSToolbarSidebarTrackingSeparatorItemIdentifier,
};
use objc2_foundation::{MainThreadMarker, NSArray, NSObject, NSObjectProtocol, NSString};

use crate::app;
use crate::prompt::PromptDelegate;

const NAV_ID: &str = "bf.navigation";
const SEARCH_ID: &str = "bf.filter";

pub struct ToolbarIvars {
    nav: OnceCell<Retained<NSToolbarItemGroup>>,
    search: OnceCell<Retained<NSSearchToolbarItem>>,
    delegate: OnceCell<Retained<PromptDelegate>>,
    /// The filter field is in the toolbar.
    search_added: std::cell::Cell<bool>,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclass requirements.
    // - ToolbarController does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BFToolbarController"]
    #[ivars = ToolbarIvars]
    pub struct ToolbarController;

    // SAFETY: `NSObjectProtocol` has no safety requirements. The signatures match.
    unsafe impl NSObjectProtocol for ToolbarController {}

    // SAFETY: `NSToolbarDelegate` has no safety requirements. The signatures match.
    unsafe impl NSToolbarDelegate for ToolbarController {
        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item_for_identifier(
            &self,
            _toolbar: &NSToolbar,
            identifier: &NSToolbarItemIdentifier,
            _inserted: bool,
        ) -> Option<Retained<NSToolbarItem>> {
            self.item(&identifier.to_string())
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn default_identifiers(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            identifiers(self.ivars().search_added.get())
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed_identifiers(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            identifiers(true)
        }
    }

    impl ToolbarController {
        #[unsafe(method(navigate:))]
        fn navigate(&self, sender: Option<&AnyObject>) {
            let index = sender
                .and_then(|s| s.downcast_ref::<NSToolbarItemGroup>())
                .map(|g| g.selectedIndex())
                .unwrap_or(-1);
            match index {
                0 => app::dispatch(Action::Back),
                1 => app::dispatch(Action::Forward),
                _ => {}
            }
        }
    }
);

impl ToolbarController {
    fn item(&self, id: &str) -> Option<Retained<NSToolbarItem>> {
        match id {
            NAV_ID => self
                .ivars()
                .nav
                .get()
                .map(|g| Retained::into_super(g.clone())),
            SEARCH_ID => Some(Retained::into_super(self.search_item())),
            _ => None,
        }
    }

    /// The filter item. It is made at the first use.
    fn search_item(&self) -> Retained<NSSearchToolbarItem> {
        self.ivars()
            .search
            .get_or_init(|| {
                let search = NSSearchToolbarItem::initWithItemIdentifier(
                    NSSearchToolbarItem::alloc(self.mtm()),
                    &NSString::from_str(SEARCH_ID),
                );
                let field = search.searchField();
                field.setPlaceholderString(Some(&NSString::from_str("Filter or go to a path")));
                if let Some(delegate) = self.ivars().delegate.get() {
                    // SAFETY: The controller keeps the delegate alive as long
                    // as the field.
                    unsafe { field.setDelegate(Some(ProtocolObject::from_ref(&**delegate))) };
                }
                search
            })
            .clone()
    }
}

fn identifiers(with_search: bool) -> Retained<NSArray<NSToolbarItemIdentifier>> {
    // SAFETY: The system identifiers are immutable statics from AppKit.
    let (separator, flexible) = unsafe {
        (
            NSToolbarSidebarTrackingSeparatorItemIdentifier,
            NSToolbarFlexibleSpaceItemIdentifier,
        )
    };
    let nav = NSString::from_str(NAV_ID);
    let search = NSString::from_str(SEARCH_ID);
    if with_search {
        NSArray::from_slice(&[separator, &*nav, flexible, &*search])
    } else {
        NSArray::from_slice(&[separator, &*nav, flexible])
    }
}

/// The toolbar and its items.
#[derive(Clone)]
pub struct ToolbarUi {
    pub toolbar: Retained<NSToolbar>,
    pub controller: Retained<ToolbarController>,
}

impl ToolbarUi {
    /// The filter field, when it is in the toolbar.
    pub fn search_field(&self) -> Option<Retained<NSSearchField>> {
        self.controller
            .ivars()
            .search_added
            .get()
            .then(|| self.controller.search_item().searchField())
    }

    /// Inserts the filter field at the end of the toolbar (once).
    pub fn add_search(&self) {
        let ivars = self.controller.ivars();
        if ivars.search_added.replace(true) {
            return;
        }
        let count = self.toolbar.items().count();
        self.toolbar
            .insertItemWithItemIdentifier_atIndex(&NSString::from_str(SEARCH_ID), count as isize);
    }

    /// Opens the filter field and puts the keyboard focus in it.
    pub fn begin_search(&self) {
        self.add_search();
        self.controller.search_item().beginSearchInteraction();
    }

    /// Enables back and forward from the history.
    pub fn set_navigation(&self, can_back: bool, can_forward: bool) {
        let Some(nav) = self.controller.ivars().nav.get() else {
            return;
        };
        let items = nav.subitems();
        for (i, enabled) in [can_back, can_forward].into_iter().enumerate() {
            if i < items.count() {
                let item = items.objectAtIndex(i);
                if item.isEnabled() != enabled {
                    item.setEnabled(enabled);
                }
            }
        }
    }
}

/// Makes the toolbar. `delegate` handles the filter field text.
pub fn create(mtm: MainThreadMarker, delegate: &Retained<PromptDelegate>) -> ToolbarUi {
    let controller: Retained<ToolbarController> = {
        let this = ToolbarController::alloc(mtm).set_ivars(ToolbarIvars {
            nav: OnceCell::new(),
            search: OnceCell::new(),
            delegate: OnceCell::new(),
            search_added: std::cell::Cell::new(false),
        });
        // SAFETY: `init` is the designated initializer of NSObject.
        unsafe { msg_send![super(this), init] }
    };
    let images: Vec<Retained<NSImage>> = ["chevron.left", "chevron.right"]
        .iter()
        .filter_map(|name| {
            NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(name),
                Some(&NSString::from_str(if *name == "chevron.left" {
                    "Back"
                } else {
                    "Forward"
                })),
            )
        })
        .collect();
    let images = NSArray::from_retained_slice(&images);
    let labels =
        NSArray::from_retained_slice(&[NSString::from_str("Back"), NSString::from_str("Forward")]);
    // SAFETY: The target lives for the whole life of the app and has the
    // `navigate:` method.
    let nav = unsafe {
        NSToolbarItemGroup::groupWithItemIdentifier_images_selectionMode_labels_target_action(
            &NSString::from_str(NAV_ID),
            &images,
            NSToolbarItemGroupSelectionMode::Momentary,
            Some(&labels),
            Some(&controller),
            Some(sel!(navigate:)),
            mtm,
        )
    };
    nav.setNavigational(true);
    let _ = controller.ivars().nav.set(nav);
    let _ = controller.ivars().delegate.set(delegate.clone());

    let toolbar = NSToolbar::initWithIdentifier(
        NSToolbar::alloc(mtm),
        &NSString::from_str("BetterFinderToolbar"),
    );
    toolbar.setDelegate(Some(ProtocolObject::from_ref(&*controller)));
    toolbar.setDisplayMode(NSToolbarDisplayMode::IconOnly);
    toolbar.setAllowsUserCustomization(false);
    ToolbarUi {
        toolbar,
        controller,
    }
}
