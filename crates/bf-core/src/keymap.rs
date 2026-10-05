//! Key bindings, Ghostty style.
//!
//! # Trigger syntax
//!
//! - One key press: modifiers and a key, joined with `+`: `cmd+shift+g`, `ctrl+d`, `j`.
//! - Modifier names: `cmd` | `command` | `super`, `ctrl` | `control`,
//!   `alt` | `opt` | `option`, `shift`.
//! - A key is one character (`j`, `G`, `/`, `?`, `.`) or a name from the
//!   table in `KeyPress::from_str` (`enter`, `space`, `comma`, `f5`, ...).
//!   Names ignore case; one-character keys keep case.
//! - A sequence joins key presses with `>`: `g>g`, `g>d`, `ctrl+x>ctrl+s`.
//!
//! # Normalization of letters and shift
//!
//! A letter with shift is the uppercase letter without shift. So `shift+g`,
//! `G`, and `shift+G` are the same `KeyPress`: `Char('G')` with no shift.
//! For all other keys, `shift` stays a modifier.
//!
//! # Matching a key event
//!
//! The UI reports a `KeyEvent` with the base key (no shift applied) and the
//! shifted character. The keymap tries two candidates, in this order:
//! 1. The base key with all modifiers, normalized (`/` + shift).
//! 2. The shifted character with all modifiers except shift (`?`).
//!
//! So both `shift+slash` and `?` match the key press "shift and /".

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use crate::action::Action;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mods {
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedKey {
    Enter,
    Escape,
    Space,
    Tab,
    /// The key with the label "delete" on a Mac keyboard (backward delete).
    Backspace,
    /// Forward delete.
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// F1 to F20.
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Named(NamedKey),
}

/// One normalized key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyPress {
    pub key: Key,
    pub mods: Mods,
}

impl KeyPress {
    /// Applies the letter and shift rule in the module docs.
    pub fn normalized(self) -> Self {
        todo!()
    }
}

/// Parses one key press (no `>`).
///
/// Key names (ignore case):
/// - `enter` | `return`, `escape` | `esc`, `space`, `tab`, `backspace`,
///   `delete`, `up`, `down`, `left`, `right`, `home`, `end`,
///   `page_up` | `pageup`, `page_down` | `pagedown`, `f1` .. `f20`.
/// - Character names: `comma` `,`, `period` `.`, `slash` `/`, `backslash` `\`,
///   `semicolon` `;`, `colon` `:`, `apostrophe` | `quote` `'`, `grave` | `backquote` `` ` ``,
///   `bracket_left` `[`, `bracket_right` `]`, `minus` `-`, `equal` `=`,
///   `plus` `+`, `greater` `>`, `less` `<`, `question` `?`.
///
/// `=`, `+`, and `>` are separators in the syntax, so they need their names.
impl FromStr for KeyPress {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let _ = s;
        todo!()
    }
}

/// Canonical form: modifiers in the order `cmd`, `ctrl`, `alt`, `shift`,
/// then the key. Characters that have a name in the table use the name
/// (`cmd+comma`). `FromStr` must parse the output back to the same value.
impl fmt::Display for KeyPress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = f;
        todo!()
    }
}

/// A sequence of one or more key presses.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Trigger(pub Vec<KeyPress>);

impl FromStr for Trigger {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let _ = s;
        todo!()
    }
}

/// Key presses joined with `>`.
impl fmt::Display for Trigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = f;
        todo!()
    }
}

/// One key down, as the UI reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    /// The key without shift applied: `g` for "shift+g", `/` for "shift+/".
    /// Named keys (arrows, space, enter, ...) use `Key::Named`.
    pub base: Key,
    /// The character with shift applied, when shift is down and the key makes
    /// a character: `G`, `?`. `None` otherwise.
    pub shifted: Option<char>,
    pub mods: Mods,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    /// A binding matched.
    Action(Action),
    /// The key press continues a sequence. Wait for the next key.
    Pending,
    /// No binding. When a sequence was pending, the sequence is cancelled and
    /// this key press is consumed (it does not start a new match).
    Unbound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Leaf(Action),
    Branch(HashMap<KeyPress, Node>),
}

/// A trie of triggers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    root: HashMap<KeyPress, Node>,
}

/// The default bindings, in `keybind` value form (`trigger=action`).
/// `Keymap::default()` applies them in order. The config template and the
/// README show this list.
pub const DEFAULT_BINDINGS: &[&str] = &[
    // Cursor
    "j=cursor_down",
    "down=cursor_down",
    "k=cursor_up",
    "up=cursor_up",
    "g>g=cursor_top",
    "home=cursor_top",
    "G=cursor_bottom",
    "end=cursor_bottom",
    "ctrl+d=half_page_down",
    "ctrl+u=half_page_up",
    "page_down=page_down",
    "page_up=page_up",
    // Navigation
    "l=open",
    "right=open",
    "enter=open",
    "cmd+down=open",
    "cmd+o=open",
    "h=parent",
    "left=parent",
    "backspace=parent",
    "cmd+up=parent",
    "H=back",
    "cmd+bracket_left=back",
    "L=forward",
    "cmd+bracket_right=forward",
    "cmd+shift+g=goto_prompt",
    "g>h=goto:~",
    "g>d=goto:~/Downloads",
    "g>e=goto:~/Desktop",
    "g>o=goto:~/Documents",
    "g>a=goto:/Applications",
    "g>c=goto:~/.config",
    "g>r=goto:/",
    // View
    "space=quick_look",
    "slash=filter",
    "escape=cancel",
    "period=toggle_hidden",
    "cmd+shift+period=toggle_hidden",
    "s>n=sort:name",
    "s>s=sort:size",
    "s>m=sort:modified",
    "s>e=sort:extension",
    "s>r=toggle_sort_reverse",
    "cmd+r=refresh",
    "?=help",
    // File operations
    "r=rename",
    "cmd+shift+n=new_folder",
    "cmd+backspace=trash",
    "c>c=copy_path",
    "c>n=copy_name",
    "cmd+alt+c=copy_path",
    "T=open_terminal",
    // App
    "cmd+comma=open_config",
    "cmd+shift+comma=reload_config",
    "cmd+q=quit",
    "cmd+w=quit",
];

impl Default for Keymap {
    /// The keymap with `DEFAULT_BINDINGS`.
    fn default() -> Self {
        todo!()
    }
}

impl Keymap {
    pub fn empty() -> Self {
        Self { root: HashMap::new() }
    }

    /// Adds a binding. A later binding replaces an earlier one:
    /// - A sequence through an existing leaf replaces the leaf (`g` then `g>g`).
    /// - A leaf on an existing branch replaces the branch (`g>g` then `g`).
    ///
    /// `Action::Unbind` removes the binding for `trigger` and prunes empty branches.
    pub fn bind(&mut self, trigger: Trigger, action: Action) {
        let _ = (trigger, action);
        todo!()
    }

    /// Removes all bindings.
    pub fn clear(&mut self) {
        self.root.clear();
    }

    /// Parses a `keybind` value: `trigger=action`. The split is at the first
    /// `=`. The value `clear` is not a binding; the config parser handles it.
    pub fn parse_binding(s: &str) -> Result<(Trigger, Action), String> {
        let _ = s;
        todo!()
    }

    /// All bindings, sorted by the text form of the action and then of the
    /// trigger. For `+list-keybinds` and the help overlay.
    pub fn bindings(&self) -> Vec<(Trigger, Action)> {
        todo!()
    }
}

/// The state of a key sequence in progress. One for each window.
#[derive(Debug, Clone, Default)]
pub struct KeymapState {
    pending: Vec<KeyPress>,
}

impl KeymapState {
    /// Feeds one key event. See the module docs for the candidate order.
    /// `Escape` while a sequence is pending cancels it and returns `Unbound`.
    pub fn feed(&mut self, keymap: &Keymap, event: &KeyEvent) -> KeyOutcome {
        let _ = (keymap, event);
        todo!()
    }

    /// The key presses of the pending sequence, for the status bar (`g-`).
    pub fn pending(&self) -> &[KeyPress] {
        &self.pending
    }

    pub fn reset(&mut self) {
        self.pending.clear();
    }
}
