//! The prompt: one `NSTextField` line above the status bar, with a label.
//! It serves the filter, "go to folder", rename, and new folder.

use std::cell::OnceCell;

use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::{ClassType, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSControl, NSControlTextEditingDelegate, NSFocusRingType, NSResponder,
    NSTextField, NSTextFieldDelegate, NSTextView, NSView, NSWindow,
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
    /// `original` is the name before the rename.
    Rename {
        original: String,
    },
    NewFolder,
}

impl PromptMode {
    pub fn label(&self) -> &'static str {
        match self {
            PromptMode::Filter => "/",
            PromptMode::Goto => "Go to:",
            PromptMode::Rename { .. } => "Rename:",
            PromptMode::NewFolder => "New folder:",
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
                s.style.status_bg.setFill();
                NSBezierPath::fillRect(bounds);
                s.style.border.setFill();
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
            let object = notification.object();
            let text = object
                .and_then(|o| o.downcast::<NSTextField>().ok())
                .map(|field| field.stringValue().to_string());
            if let Some(text) = text {
                app::prompt_changed(text);
            }
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
);

impl PromptDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: `init` is the designated initializer of NSObject.
        unsafe { msg_send![super(this), init] }
    }
}

/// The views and the delegate of the prompt.
#[derive(Clone)]
pub struct PromptUi {
    pub bar: Retained<PromptBar>,
    pub label: Retained<NSTextField>,
    pub field: Retained<NSTextField>,
    /// The field keeps only a weak reference to its delegate.
    pub _delegate: Retained<PromptDelegate>,
}

pub fn create(mtm: MainThreadMarker) -> PromptUi {
    let bar = PromptBar::new(mtm);
    let label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
    let field = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
    field.setBezeled(false);
    field.setBordered(false);
    field.setDrawsBackground(false);
    field.setFocusRingType(NSFocusRingType::None);
    let delegate = PromptDelegate::new(mtm);
    // SAFETY: `PromptUi` keeps the delegate alive as long as the field.
    unsafe { field.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*delegate))) };
    bar.addSubview(&label);
    bar.addSubview(&field);
    let _ = bar.ivars().label.set(label.clone());
    let _ = bar.ivars().field.set(field.clone());
    PromptUi {
        bar,
        label,
        field,
        _delegate: delegate,
    }
}

impl PromptUi {
    /// Applies the fonts and colors of `style`.
    pub fn apply_style(&self, style: &Style) {
        self.label.setFont(Some(&style.font));
        self.label.setTextColor(Some(&style.accent));
        self.field.setFont(Some(&style.font));
        self.field.setTextColor(Some(&style.foreground));
        self.bar.layout_fields();
        self.bar.setNeedsDisplay(true);
    }

    /// Fills the prompt and moves the keyboard focus to the field.
    /// `select` is a UTF-16 range (start, length) to select.
    pub fn open(
        &self,
        window: &NSWindow,
        label: &str,
        text: &str,
        select: (usize, usize),
        caret: &NSColor,
    ) {
        self.label.setStringValue(&NSString::from_str(label));
        self.field.setStringValue(&NSString::from_str(text));
        self.bar.layout_fields();
        window.makeFirstResponder(Some(self.field_responder()));
        if let Some(editor) = self.field.currentEditor() {
            editor.setSelectedRange(NSRange::new(select.0, select.1));
            if let Ok(view) = editor.downcast::<NSTextView>() {
                view.setInsertionPointColor(Some(caret));
            }
        }
    }

    /// Replaces the text while the field is in edit mode, and puts the
    /// insertion point at the end.
    pub fn replace_text(&self, text: &str) {
        let value = NSString::from_str(text);
        match self.field.currentEditor() {
            Some(editor) => {
                editor.setString(&value);
                editor.setSelectedRange(NSRange::new(value.length(), 0));
            }
            None => self.field.setStringValue(&value),
        }
    }

    /// The current text of the field.
    pub fn text(&self) -> String {
        match self.field.currentEditor() {
            Some(editor) => editor.string().to_string(),
            None => self.field.stringValue().to_string(),
        }
    }

    fn field_responder(&self) -> &NSResponder {
        self.field.as_super().as_super().as_super()
    }
}
