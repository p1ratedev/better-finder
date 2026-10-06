//! The text inputs of the app, with one shared delegate:
//! - The bottom prompt: one `NSTextField` line above the status bar, with a
//!   label. It is the location and filter field when the toolbar is off.
//! - The inline rename field over the name of the cursor row.
//! - The toolbar filter field (it lives in `toolbar`, with this delegate).

use std::cell::OnceCell;
use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::{ProtocolObject, Sel};
use objc2::{ClassType, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSControl, NSControlTextEditingDelegate, NSFocusRingType, NSResponder,
    NSSearchField, NSSearchFieldDelegate, NSTextField, NSTextFieldDelegate, NSTextView, NSView,
    NSWindow,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRange, NSRect, NSSize,
    NSString,
};

use crate::app;
use crate::theme::Style;

/// What the prompt is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptMode {
    Filter,
    Goto,
    /// `source` is the full path of the item, from the time the prompt
    /// opened. The rename does not depend on the current directory.
    Rename {
        source: PathBuf,
    },
}

impl PromptMode {
    pub fn label(&self) -> &'static str {
        match self {
            PromptMode::Filter => "/",
            PromptMode::Goto => "Go to:",
            PromptMode::Rename { .. } => "Rename:",
        }
    }
}

#[derive(Default)]
pub struct BarIvars {
    label: OnceCell<Retained<NSTextField>>,
    field: OnceCell<Retained<NSTextField>>,
}

define_class!(
    // SAFETY:
    // - NSView has no subclass requirements.
    // - PromptBar does not implement Drop.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BFPromptBar"]
    #[ivars = BarIvars]
    pub struct PromptBar;

    impl PromptBar {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let bounds = self.bounds();
            app::with_ref(|s| {
                s.style.colors.bar_bg.setFill();
                NSBezierPath::fillRect(bounds);
                s.style.colors.separator.setFill();
                let line = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(bounds.size.width, 1.0));
                NSBezierPath::fillRect(line);
            });
        }

        #[unsafe(method(resizeSubviewsWithOldSize:))]
        fn resize_subviews_with_old_size(&self, _old: NSSize) {
            self.layout_fields();
        }
    }
);

impl PromptBar {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(BarIvars::default());
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 30.0));
        // SAFETY: `initWithFrame:` is the designated initializer of NSView.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// Puts the label on the left and the field in the rest of the width,
    /// both centered vertically.
    pub fn layout_fields(&self) {
        let (Some(label), Some(field)) = (self.ivars().label.get(), self.ivars().field.get())
        else {
            return;
        };
        let bounds = self.bounds();
        // `sizeToFit` includes the insets of the cell.
        label.sizeToFit();
        let label_size = label.frame().size;
        let label_w = label_size.width.ceil().max(0.0);
        let label_h = label_size.height.ceil().max(0.0);
        label.setFrame(NSRect::new(
            NSPoint::new(12.0, ((bounds.size.height - label_h) / 2.0).round()),
            NSSize::new(label_w, label_h),
        ));
        let field_h = field.intrinsicContentSize().height.ceil().max(0.0);
        let x = 12.0 + label_w + 6.0;
        field.setFrame(NSRect::new(
            NSPoint::new(x, ((bounds.size.height - field_h) / 2.0).round()),
            NSSize::new((bounds.size.width - x - 12.0).max(0.0), field_h),
        ));
    }
}

define_class!(
    // SAFETY:
    // - NSObject has no subclass requirements.
    // - PromptDelegate does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BFPromptDelegate"]
    pub struct PromptDelegate;

    // SAFETY: `NSObjectProtocol` has no safety requirements. The signatures match.
    unsafe impl NSObjectProtocol for PromptDelegate {}

    // SAFETY: `NSControlTextEditingDelegate` has no safety requirements. The signatures match.
    unsafe impl NSControlTextEditingDelegate for PromptDelegate {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, notification: &NSNotification) {
            let Some(object) = notification.object() else {
                return;
            };
            let is_search = object.downcast_ref::<NSSearchField>().is_some();
            if let Ok(field) = object.downcast::<NSTextField>() {
                app::prompt_changed(field.stringValue().to_string(), is_search);
            }
        }

        #[unsafe(method(controlTextDidEndEditing:))]
        fn control_text_did_end_editing(&self, _notification: &NSNotification) {
            // AppKit is in the middle of a first responder change.
            app::after_now(app::prompt_end_editing);
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn control_text_view_do_command_by_selector(
            &self,
            _control: &NSControl,
            text_view: &NSTextView,
            command: Sel,
        ) -> bool {
            let text = text_view.string().to_string();
            if command == sel!(insertNewline:) {
                // Accept and cancel change the first responder. Do that after
                // the field editor completes this command.
                app::after_now(move || app::prompt_accept(text));
                true
            } else if command == sel!(cancelOperation:) {
                app::after_now(app::prompt_cancel);
                true
            } else if command == sel!(moveUp:) {
                app::prompt_move(-1)
            } else if command == sel!(moveDown:) {
                app::prompt_move(1)
            } else if command == sel!(insertTab:) {
                app::prompt_complete(text)
            } else {
                false
            }
        }
    }

    // SAFETY: `NSTextFieldDelegate` has no safety requirements. The signatures match.
    unsafe impl NSTextFieldDelegate for PromptDelegate {}

    // SAFETY: `NSSearchFieldDelegate` has no safety requirements. The signatures match.
    unsafe impl NSSearchFieldDelegate for PromptDelegate {}
);

impl PromptDelegate {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: `init` is the designated initializer of NSObject.
        unsafe { msg_send![super(this), init] }
    }
}

/// The views and the delegate of the prompt.
#[derive(Clone)]
pub struct PromptUi {
    /// The bottom prompt: the filter without a toolbar, and "go to".
    pub bar: Retained<PromptBar>,
    pub label: Retained<NSTextField>,
    pub field: Retained<NSTextField>,
    /// The rename field over the name of the cursor row.
    pub inline: Retained<NSTextField>,
    /// The fields keep only a weak reference to their delegate.
    pub _delegate: Retained<PromptDelegate>,
}

pub fn create(mtm: MainThreadMarker, delegate: &Retained<PromptDelegate>) -> PromptUi {
    let bar = PromptBar::new(mtm);
    let label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
    let field = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
    field.setBezeled(false);
    field.setBordered(false);
    field.setDrawsBackground(false);
    field.setFocusRingType(NSFocusRingType::None);
    field.setPlaceholderString(Some(&NSString::from_str("Filter or go to a path")));
    let inline = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
    inline.setBezeled(false);
    inline.setBordered(true);
    inline.setDrawsBackground(true);
    inline.setBackgroundColor(Some(&NSColor::textBackgroundColor()));
    inline.setTextColor(Some(&NSColor::textColor()));
    inline.setFocusRingType(NSFocusRingType::Exterior);
    if let Some(cell) = inline.cell() {
        cell.setScrollable(true);
        cell.setWraps(false);
    }
    // SAFETY: `PromptUi` keeps the delegate alive as long as the fields.
    unsafe {
        field.setDelegate(Some(ProtocolObject::from_ref(&**delegate)));
        inline.setDelegate(Some(ProtocolObject::from_ref(&**delegate)));
    }
    bar.addSubview(&label);
    bar.addSubview(&field);
    let _ = bar.ivars().label.set(label.clone());
    let _ = bar.ivars().field.set(field.clone());
    PromptUi {
        bar,
        label,
        field,
        inline,
        _delegate: delegate.clone(),
    }
}

/// Puts the keyboard focus in `field` with `text`, and selects a UTF-16
/// range (start, length). The window becomes the key window first: the
/// Quick Look panel can be the key window, and typed text must go here.
pub fn focus_field(
    window: &NSWindow,
    field: &NSTextField,
    text: &str,
    select: (usize, usize),
    caret: &NSColor,
) {
    field.setStringValue(&NSString::from_str(text));
    window.makeKeyWindow();
    let responder: &NSResponder = field.as_super().as_super().as_super();
    window.makeFirstResponder(Some(responder));
    if let Some(editor) = field.currentEditor() {
        editor.setSelectedRange(NSRange::new(select.0, select.1));
        if let Ok(view) = editor.downcast::<NSTextView>() {
            view.setInsertionPointColor(Some(caret));
        }
    }
}

/// The current text of `field`, also while the field editor edits it.
pub fn field_text(field: &NSTextField) -> String {
    match field.currentEditor() {
        Some(editor) => editor.string().to_string(),
        None => field.stringValue().to_string(),
    }
}

/// Replaces the text of `field` while the field editor edits it, and puts
/// the insertion point at the end.
pub fn replace_field_text(field: &NSTextField, text: &str) {
    let value = NSString::from_str(text);
    match field.currentEditor() {
        Some(editor) => {
            editor.setString(&value);
            editor.setSelectedRange(NSRange::new(value.length(), 0));
        }
        None => field.setStringValue(&value),
    }
}

impl PromptUi {
    /// Applies the fonts and colors of `style`.
    pub fn apply_style(&self, style: &Style) {
        self.label.setFont(Some(&style.font));
        self.label.setTextColor(Some(&style.colors.accent));
        self.field.setFont(Some(&style.font));
        self.field.setTextColor(Some(&style.colors.name));
        self.inline.setFont(Some(&style.font));
        self.bar.layout_fields();
        self.bar.setNeedsDisplay(true);
    }

    /// Sets the label of the bottom prompt.
    pub fn set_label(&self, label: &str) {
        self.label.setStringValue(&NSString::from_str(label));
        self.bar.layout_fields();
    }
}
