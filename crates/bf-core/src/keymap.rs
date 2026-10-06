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
        let Self { key, mut mods } = self;
        let key = match key {
            Key::Char(' ') => Key::Named(NamedKey::Space),
            Key::Char('\r' | '\n') => Key::Named(NamedKey::Enter),
            Key::Char('\t') => Key::Named(NamedKey::Tab),
            Key::Char('\u{1b}') => Key::Named(NamedKey::Escape),
            Key::Char('\u{7f}') => Key::Named(NamedKey::Backspace),
            Key::Char(c) if c.is_uppercase() => {
                mods.shift = false;
                Key::Char(c)
            }
            Key::Char(c) if c.is_lowercase() && mods.shift => {
                let mut upper = c.to_uppercase();
                match (upper.next(), upper.next()) {
                    (Some(u), None) => {
                        mods.shift = false;
                        Key::Char(u)
                    }
                    _ => Key::Char(c),
                }
            }
            other => other,
        };
        Self { key, mods }
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
        let s = s.trim();
        if s.is_empty() {
            return Err("empty key".to_string());
        }
        let mut parts: Vec<&str> = s.split('+').collect();
        let key_text = parts.pop().unwrap_or_default();
        if key_text.is_empty() {
            return Err(format!(
                "missing key in \"{s}\" (write `plus` for the + key)"
            ));
        }
        let mut mods = Mods::default();
        for part in parts {
            match part.trim().to_ascii_lowercase().as_str() {
                "cmd" | "command" | "super" => mods.cmd = true,
                "ctrl" | "control" => mods.ctrl = true,
                "alt" | "opt" | "option" => mods.alt = true,
                "shift" => mods.shift = true,
                "" => return Err(format!("empty modifier in \"{s}\"")),
                other => return Err(format!("unknown modifier: {other}")),
            }
        }
        Ok(Self {
            key: parse_key(key_text.trim())?,
            mods,
        }
        .normalized())
    }
}

/// Characters that have a name in the key table. The first name of a
/// character is the canonical name.
const CHAR_NAMES: &[(&str, char)] = &[
    ("comma", ','),
    ("period", '.'),
    ("slash", '/'),
    ("backslash", '\\'),
    ("semicolon", ';'),
    ("colon", ':'),
    ("apostrophe", '\''),
    ("quote", '\''),
    ("grave", '`'),
    ("backquote", '`'),
    ("bracket_left", '['),
    ("bracket_right", ']'),
    ("minus", '-'),
    ("equal", '='),
    ("plus", '+'),
    ("greater", '>'),
    ("less", '<'),
    ("question", '?'),
];

const NAMED_KEYS: &[(&str, NamedKey)] = &[
    ("enter", NamedKey::Enter),
    ("return", NamedKey::Enter),
    ("escape", NamedKey::Escape),
    ("esc", NamedKey::Escape),
    ("space", NamedKey::Space),
    ("tab", NamedKey::Tab),
    ("backspace", NamedKey::Backspace),
    ("delete", NamedKey::Delete),
    ("up", NamedKey::Up),
    ("down", NamedKey::Down),
    ("left", NamedKey::Left),
    ("right", NamedKey::Right),
    ("home", NamedKey::Home),
    ("end", NamedKey::End),
    ("page_up", NamedKey::PageUp),
    ("pageup", NamedKey::PageUp),
    ("page_down", NamedKey::PageDown),
    ("pagedown", NamedKey::PageDown),
];

fn parse_key(text: &str) -> Result<Key, String> {
    let mut chars = text.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Ok(Key::Char(c));
    }
    let name = text.to_ascii_lowercase();
    if let Some((_, key)) = NAMED_KEYS.iter().find(|(n, _)| *n == name) {
        return Ok(Key::Named(*key));
    }
    if let Some((_, c)) = CHAR_NAMES.iter().find(|(n, _)| *n == name) {
        return Ok(Key::Char(*c));
    }
    if let Some(number) = name.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        if (1..=20).contains(&number) {
            return Ok(Key::Named(NamedKey::F(number)));
        }
    }
    Err(format!("unknown key: {text}"))
}

fn named_key_text(key: NamedKey) -> String {
    match key {
        NamedKey::F(n) => format!("f{n}"),
        other => NAMED_KEYS
            .iter()
            .find(|(_, k)| *k == other)
            .map_or_else(String::new, |(n, _)| (*n).to_string()),
    }
}

/// Canonical form: modifiers in the order `cmd`, `ctrl`, `alt`, `shift`,
/// then the key. Characters that have a name in the table use the name
/// (`cmd+comma`). `FromStr` must parse the output back to the same value.
impl fmt::Display for KeyPress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Mods {
            cmd,
            ctrl,
            alt,
            shift,
        } = self.mods;
        // An uppercase letter with another modifier shows as `shift` and the
        // lowercase letter: `cmd+shift+g`, not `cmd+G`. A bare uppercase
        // letter stays `G`.
        let shifted_letter = match self.key {
            Key::Char(c) if (cmd || ctrl || alt) && c.is_uppercase() => lowercase_of(c),
            _ => None,
        };
        let shift = shift || shifted_letter.is_some();
        for (on, name) in [(cmd, "cmd"), (ctrl, "ctrl"), (alt, "alt"), (shift, "shift")] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        if let Some(lower) = shifted_letter {
            return write!(f, "{lower}");
        }
        match self.key {
            Key::Named(key) => f.write_str(&named_key_text(key)),
            Key::Char(' ') => f.write_str("space"),
            Key::Char(c) => match CHAR_NAMES.iter().find(|(_, named)| *named == c) {
                Some((name, _)) => f.write_str(name),
                None => write!(f, "{c}"),
            },
        }
    }
}

/// The lowercase form of the uppercase letter `c`, when it is one character
/// and its uppercase form is `c` again (so that the text round-trips).
fn lowercase_of(c: char) -> Option<char> {
    let mut lower = c.to_lowercase();
    let (Some(l), None) = (lower.next(), lower.next()) else {
        return None;
    };
    let mut upper = l.to_uppercase();
    (upper.next() == Some(c) && upper.next().is_none()).then_some(l)
}

/// A sequence of one or more key presses.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Trigger(pub Vec<KeyPress>);

impl FromStr for Trigger {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let presses = s
            .split('>')
            .map(|part| part.parse::<KeyPress>())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self(presses))
    }
}

/// Key presses joined with `>`.
impl fmt::Display for Trigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, press) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(">")?;
            }
            write!(f, "{press}")?;
        }
        Ok(())
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
    "cmd+shift+h=goto:~",
    "cmd+shift+d=goto:~/Desktop",
    "cmd+shift+o=goto:~/Documents",
    "cmd+alt+l=goto:~/Downloads",
    "cmd+shift+a=goto:/Applications",
    "cmd+shift+u=goto:/Applications/Utilities",
    "cmd+shift+i=goto:~/Library/Mobile Documents/com~apple~CloudDocs",
    "g>d=goto:~/Downloads",
    "g>e=goto:~/Desktop",
    "g>o=goto:~/Documents",
    "g>a=goto:/Applications",
    "g>c=goto:~/.config",
    "g>r=goto:/",
    // View
    "space=quick_look",
    "slash=filter",
    "cmd+f=filter",
    "cmd+ctrl+s=toggle_sidebar",
    "tab=focus_sidebar",
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
        let mut keymap = Self::empty();
        for text in DEFAULT_BINDINGS {
            let (trigger, action) = Self::parse_binding(text)
                .unwrap_or_else(|err| panic!("bad default binding \"{text}\": {err}"));
            keymap.bind(trigger, action);
        }
        keymap
    }
}

impl Keymap {
    pub fn empty() -> Self {
        Self {
            root: HashMap::new(),
        }
    }

    /// Adds a binding. A later binding replaces an earlier one:
    /// - A sequence through an existing leaf replaces the leaf (`g` then `g>g`).
    /// - A leaf on an existing branch replaces the branch (`g>g` then `g`).
    ///
    /// `Action::Unbind` removes the binding for `trigger` and prunes empty branches.
    pub fn bind(&mut self, trigger: Trigger, action: Action) {
        if trigger.0.is_empty() {
            return;
        }
        if action == Action::Unbind {
            remove_leaf(&mut self.root, &trigger.0);
        } else {
            insert_leaf(&mut self.root, &trigger.0, action);
        }
    }

    /// Removes all bindings.
    pub fn clear(&mut self) {
        self.root.clear();
    }

    /// Parses a `keybind` value: `trigger=action`. The split is at the first
    /// `=`. The value `clear` is not a binding; the config parser handles it.
    pub fn parse_binding(s: &str) -> Result<(Trigger, Action), String> {
        let (trigger, action) = s
            .split_once('=')
            .ok_or_else(|| format!("expected trigger=action, got \"{}\"", s.trim()))?;
        let (trigger, action) = (trigger.trim(), action.trim());
        if trigger.is_empty() {
            return Err("empty trigger".to_string());
        }
        if action.is_empty() {
            return Err("empty action".to_string());
        }
        let trigger = trigger.parse::<Trigger>()?;
        let action = action.parse::<Action>().map_err(|e| e.to_string())?;
        Ok((trigger, action))
    }

    /// All bindings, sorted by the text form of the action and then of the
    /// trigger. For `+list-keybinds` and the help overlay.
    pub fn bindings(&self) -> Vec<(Trigger, Action)> {
        let mut out = Vec::new();
        collect(&self.root, &mut Vec::new(), &mut out);
        out.sort_by_cached_key(|(trigger, action)| (action.to_string(), trigger.to_string()));
        out
    }

    /// The node map after the key presses in `path`. Empty `path` is the root.
    fn children_at(&self, path: &[KeyPress]) -> Option<&HashMap<KeyPress, Node>> {
        let mut map = &self.root;
        for press in path {
            match map.get(press)? {
                Node::Branch(next) => map = next,
                Node::Leaf(_) => return None,
            }
        }
        Some(map)
    }
}

fn insert_leaf(map: &mut HashMap<KeyPress, Node>, path: &[KeyPress], action: Action) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    if rest.is_empty() {
        map.insert(*first, Node::Leaf(action));
        return;
    }
    let node = map
        .entry(*first)
        .or_insert_with(|| Node::Branch(HashMap::new()));
    if matches!(node, Node::Leaf(_)) {
        *node = Node::Branch(HashMap::new());
    }
    if let Node::Branch(children) = node {
        insert_leaf(children, rest, action);
    }
}

/// Removes the leaf at `path` and prunes branches that become empty.
fn remove_leaf(map: &mut HashMap<KeyPress, Node>, path: &[KeyPress]) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    if rest.is_empty() {
        if matches!(map.get(first), Some(Node::Leaf(_))) {
            map.remove(first);
        }
        return;
    }
    if let Some(Node::Branch(children)) = map.get_mut(first) {
        remove_leaf(children, rest);
        if children.is_empty() {
            map.remove(first);
        }
    }
}

fn collect(
    map: &HashMap<KeyPress, Node>,
    prefix: &mut Vec<KeyPress>,
    out: &mut Vec<(Trigger, Action)>,
) {
    for (press, node) in map {
        prefix.push(*press);
        match node {
            Node::Leaf(action) => out.push((Trigger(prefix.clone()), action.clone())),
            Node::Branch(children) => collect(children, prefix, out),
        }
        prefix.pop();
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
        if !self.pending.is_empty() && event.base == Key::Named(NamedKey::Escape) {
            self.pending.clear();
            return KeyOutcome::Unbound;
        }
        let first = KeyPress {
            key: event.base,
            mods: event.mods,
        }
        .normalized();
        let second = match event.shifted {
            Some(c) if event.mods.shift => {
                let mods = Mods {
                    shift: false,
                    ..event.mods
                };
                Some(
                    KeyPress {
                        key: Key::Char(c),
                        mods,
                    }
                    .normalized(),
                )
            }
            _ => None,
        };
        let candidates = std::iter::once(first).chain(second.filter(|s| *s != first));
        let Some(children) = keymap.children_at(&self.pending) else {
            // The keymap changed under a pending sequence.
            self.pending.clear();
            return KeyOutcome::Unbound;
        };
        for press in candidates {
            match children.get(&press) {
                Some(Node::Leaf(action)) => {
                    let action = action.clone();
                    self.pending.clear();
                    return KeyOutcome::Action(action);
                }
                Some(Node::Branch(_)) => {
                    self.pending.push(press);
                    return KeyOutcome::Pending;
                }
                None => {}
            }
        }
        self.pending.clear();
        KeyOutcome::Unbound
    }

    /// The key presses of the pending sequence, for the status bar (`g-`).
    pub fn pending(&self) -> &[KeyPress] {
        &self.pending
    }

    pub fn reset(&mut self) {
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sort::SortKey;

    fn press(s: &str) -> KeyPress {
        s.parse().unwrap()
    }

    fn trigger(s: &str) -> Trigger {
        s.parse().unwrap()
    }

    fn ev(c: char) -> KeyEvent {
        KeyEvent {
            base: Key::Char(c),
            shifted: None,
            mods: Mods::default(),
        }
    }

    fn named(key: NamedKey) -> KeyEvent {
        KeyEvent {
            base: Key::Named(key),
            shifted: None,
            mods: Mods::default(),
        }
    }

    fn feed(state: &mut KeymapState, map: &Keymap, event: KeyEvent) -> KeyOutcome {
        state.feed(map, &event)
    }

    #[test]
    fn default_bindings_parse_and_round_trip() {
        for text in DEFAULT_BINDINGS {
            let (trigger, action) =
                Keymap::parse_binding(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            let again: Trigger = trigger.to_string().parse().unwrap();
            assert_eq!(again, trigger, "{text}");
            assert_eq!(
                action.to_string().parse::<Action>().unwrap(),
                action,
                "{text}"
            );
        }
    }

    #[test]
    fn default_keymap_has_all_bindings() {
        let map = Keymap::default();
        assert_eq!(map.bindings().len(), DEFAULT_BINDINGS.len());
    }

    #[test]
    fn shift_letter_normalization() {
        assert_eq!(press("shift+g"), press("G"));
        assert_eq!(press("shift+G"), press("G"));
        assert_eq!(press("G").mods, Mods::default());
        assert_eq!(press("G").key, Key::Char('G'));
        assert_eq!(press("cmd+shift+g"), press("cmd+G"));
        assert_eq!(press("G").to_string(), "G");
        assert_eq!(press("shift+g").to_string(), "G");
        assert_eq!(press("cmd+shift+g").to_string(), "cmd+shift+g");
        assert_eq!(press("cmd+G").to_string(), "cmd+shift+g");
        assert_eq!(press("ctrl+alt+X").to_string(), "ctrl+alt+shift+x");
        assert_eq!(press("cmd+g").to_string(), "cmd+g");
        for text in ["cmd+shift+g", "ctrl+alt+shift+x", "G", "cmd+g", "alt+É"] {
            let p = press(text);
            assert_eq!(p.to_string().parse::<KeyPress>().unwrap(), p, "{text}");
        }
        // Other keys keep shift.
        assert!(press("shift+slash").mods.shift);
        assert!(press("shift+up").mods.shift);
    }

    #[test]
    fn key_names_and_aliases() {
        assert_eq!(press("Return"), press("enter"));
        assert_eq!(press("ESC"), press("escape"));
        assert_eq!(press("command+opt+comma"), press("cmd+alt+,"));
        assert_eq!(
            press("super+control+f20").mods,
            Mods {
                cmd: true,
                ctrl: true,
                alt: false,
                shift: false
            }
        );
        assert_eq!(press("f5").key, Key::Named(NamedKey::F(5)));
        assert_eq!(press("quote").key, Key::Char('\''));
        assert_eq!(press("plus").to_string(), "plus");
        assert_eq!(press("cmd+comma").to_string(), "cmd+comma");
        assert_eq!(press("?").to_string(), "question");
        assert_eq!(press("page_up").to_string(), "page_up");
        assert_eq!(press("shift+up").to_string(), "shift+up");
    }

    #[test]
    fn normalizes_control_characters() {
        let n = |c: char| {
            KeyPress {
                key: Key::Char(c),
                mods: Mods::default(),
            }
            .normalized()
            .key
        };
        assert_eq!(n(' '), Key::Named(NamedKey::Space));
        assert_eq!(n('\r'), Key::Named(NamedKey::Enter));
        assert_eq!(n('\n'), Key::Named(NamedKey::Enter));
        assert_eq!(n('\t'), Key::Named(NamedKey::Tab));
        assert_eq!(n('\u{1b}'), Key::Named(NamedKey::Escape));
        assert_eq!(n('\u{7f}'), Key::Named(NamedKey::Backspace));
    }

    #[test]
    fn bad_keys() {
        for bad in ["", "cmd+", "cmd++g", "hyper+g", "f21", "f0", "nokey", "+"] {
            assert!(bad.parse::<KeyPress>().is_err(), "{bad}");
        }
        assert!("g>".parse::<Trigger>().is_err());
        assert!(">g".parse::<Trigger>().is_err());
        assert!("".parse::<Trigger>().is_err());
    }

    #[test]
    fn trigger_round_trip() {
        let t = trigger("ctrl+x>ctrl+s");
        assert_eq!(t.0.len(), 2);
        assert_eq!(t.to_string(), "ctrl+x>ctrl+s");
        assert_eq!(trigger("g > g").0.len(), 2);
    }

    #[test]
    fn parse_binding_rules() {
        let (t, a) = Keymap::parse_binding("  g>d = goto:~/Downloads ").unwrap();
        assert_eq!(t, trigger("g>d"));
        assert_eq!(a, Action::Goto("~/Downloads".into()));
        let (_, a) = Keymap::parse_binding("x=goto:/a=b").unwrap();
        assert_eq!(a, Action::Goto("/a=b".into()));
        for bad in ["", "j", "=quit", "j=", " = ", "j=bogus", "bogus=quit"] {
            assert!(Keymap::parse_binding(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn single_key_action() {
        let map = Keymap::default();
        let mut st = KeymapState::default();
        assert_eq!(
            feed(&mut st, &map, ev('j')),
            KeyOutcome::Action(Action::CursorDown)
        );
        assert_eq!(
            feed(&mut st, &map, named(NamedKey::Enter)),
            KeyOutcome::Action(Action::Open)
        );
        assert_eq!(feed(&mut st, &map, ev('~')), KeyOutcome::Unbound);
    }

    #[test]
    fn shifted_letter_event() {
        let map = Keymap::default();
        let mut st = KeymapState::default();
        let shift = Mods {
            shift: true,
            ..Mods::default()
        };
        let event = KeyEvent {
            base: Key::Char('g'),
            shifted: Some('G'),
            mods: shift,
        };
        assert_eq!(
            st.feed(&map, &event),
            KeyOutcome::Action(Action::CursorBottom)
        );
        let cmd_shift = Mods {
            cmd: true,
            shift: true,
            ..Mods::default()
        };
        let event = KeyEvent {
            base: Key::Char('g'),
            shifted: Some('G'),
            mods: cmd_shift,
        };
        assert_eq!(
            st.feed(&map, &event),
            KeyOutcome::Action(Action::GotoPrompt)
        );
    }

    #[test]
    fn question_and_shift_slash_both_match() {
        let shift = Mods {
            shift: true,
            ..Mods::default()
        };
        let event = KeyEvent {
            base: Key::Char('/'),
            shifted: Some('?'),
            mods: shift,
        };
        for text in ["?=help", "shift+slash=help"] {
            let mut map = Keymap::empty();
            let (t, a) = Keymap::parse_binding(text).unwrap();
            map.bind(t, a);
            let mut st = KeymapState::default();
            assert_eq!(
                st.feed(&map, &event),
                KeyOutcome::Action(Action::Help),
                "{text}"
            );
        }
    }

    #[test]
    fn shifted_candidate_second() {
        // Candidate 1 (shift+slash) wins over candidate 2 (?).
        let mut map = Keymap::empty();
        for text in ["?=help", "shift+slash=refresh"] {
            let (t, a) = Keymap::parse_binding(text).unwrap();
            map.bind(t, a);
        }
        let shift = Mods {
            shift: true,
            ..Mods::default()
        };
        let event = KeyEvent {
            base: Key::Char('/'),
            shifted: Some('?'),
            mods: shift,
        };
        let mut st = KeymapState::default();
        assert_eq!(st.feed(&map, &event), KeyOutcome::Action(Action::Refresh));
    }

    #[test]
    fn sequences() {
        let map = Keymap::default();
        let mut st = KeymapState::default();
        assert_eq!(feed(&mut st, &map, ev('g')), KeyOutcome::Pending);
        assert_eq!(st.pending(), &[press("g")]);
        assert_eq!(
            feed(&mut st, &map, ev('g')),
            KeyOutcome::Action(Action::CursorTop)
        );
        assert!(st.pending().is_empty());

        assert_eq!(feed(&mut st, &map, ev('g')), KeyOutcome::Pending);
        assert_eq!(
            feed(&mut st, &map, ev('d')),
            KeyOutcome::Action(Action::Goto("~/Downloads".into()))
        );

        assert_eq!(feed(&mut st, &map, ev('s')), KeyOutcome::Pending);
        assert_eq!(
            feed(&mut st, &map, ev('s')),
            KeyOutcome::Action(Action::Sort(SortKey::Size))
        );
    }

    #[test]
    fn wrong_second_key_is_consumed() {
        let map = Keymap::default();
        let mut st = KeymapState::default();
        assert_eq!(feed(&mut st, &map, ev('g')), KeyOutcome::Pending);
        // `j` is bound alone, but the pending sequence consumes it.
        assert_eq!(feed(&mut st, &map, ev('j')), KeyOutcome::Unbound);
        assert!(st.pending().is_empty());
        assert_eq!(
            feed(&mut st, &map, ev('j')),
            KeyOutcome::Action(Action::CursorDown)
        );
    }

    #[test]
    fn escape_cancels_pending() {
        let map = Keymap::default();
        let mut st = KeymapState::default();
        assert_eq!(feed(&mut st, &map, ev('g')), KeyOutcome::Pending);
        assert_eq!(
            feed(&mut st, &map, named(NamedKey::Escape)),
            KeyOutcome::Unbound
        );
        assert!(st.pending().is_empty());
        // Without a pending sequence, escape uses its binding.
        assert_eq!(
            feed(&mut st, &map, named(NamedKey::Escape)),
            KeyOutcome::Action(Action::Cancel)
        );
    }

    #[test]
    fn three_key_sequence_and_reset() {
        let mut map = Keymap::empty();
        map.bind(trigger("a>b>c"), Action::Quit);
        let mut st = KeymapState::default();
        assert_eq!(feed(&mut st, &map, ev('a')), KeyOutcome::Pending);
        assert_eq!(feed(&mut st, &map, ev('b')), KeyOutcome::Pending);
        assert_eq!(st.pending().len(), 2);
        st.reset();
        assert!(st.pending().is_empty());
        assert_eq!(feed(&mut st, &map, ev('a')), KeyOutcome::Pending);
        assert_eq!(feed(&mut st, &map, ev('b')), KeyOutcome::Pending);
        assert_eq!(
            feed(&mut st, &map, ev('c')),
            KeyOutcome::Action(Action::Quit)
        );
    }

    #[test]
    fn later_binding_replaces_leaf_and_branch() {
        let mut map = Keymap::empty();
        map.bind(trigger("g"), Action::Open);
        map.bind(trigger("g>g"), Action::CursorTop);
        let mut st = KeymapState::default();
        assert_eq!(feed(&mut st, &map, ev('g')), KeyOutcome::Pending);
        assert_eq!(
            feed(&mut st, &map, ev('g')),
            KeyOutcome::Action(Action::CursorTop)
        );

        map.bind(trigger("g"), Action::Quit);
        assert_eq!(
            feed(&mut st, &map, ev('g')),
            KeyOutcome::Action(Action::Quit)
        );
        assert_eq!(map.bindings().len(), 1);

        map.bind(trigger("g"), Action::Help);
        assert_eq!(
            feed(&mut st, &map, ev('g')),
            KeyOutcome::Action(Action::Help)
        );
    }

    #[test]
    fn unbind_removes_and_prunes() {
        let mut map = Keymap::empty();
        map.bind(trigger("g>g"), Action::CursorTop);
        map.bind(trigger("g>d"), Action::Open);
        map.bind(trigger("g>g"), Action::Unbind);
        assert_eq!(map.bindings().len(), 1);
        map.bind(trigger("g>d"), Action::Unbind);
        assert!(map.bindings().is_empty());
        assert_eq!(map, Keymap::empty());
        // Unbind of a missing trigger does nothing.
        map.bind(trigger("x>y"), Action::Unbind);
        assert_eq!(map, Keymap::empty());
        let mut st = KeymapState::default();
        assert_eq!(feed(&mut st, &map, ev('g')), KeyOutcome::Unbound);
    }

    #[test]
    fn unbind_default_binding() {
        let mut map = Keymap::default();
        let (t, a) = Keymap::parse_binding("j=unbind").unwrap();
        map.bind(t, a);
        let mut st = KeymapState::default();
        assert_eq!(feed(&mut st, &map, ev('j')), KeyOutcome::Unbound);
    }

    #[test]
    fn clear_removes_everything() {
        let mut map = Keymap::default();
        map.clear();
        assert!(map.bindings().is_empty());
    }

    #[test]
    fn bindings_are_sorted_by_action_then_trigger() {
        let mut map = Keymap::empty();
        map.bind(trigger("z"), Action::Open);
        map.bind(trigger("a"), Action::Open);
        map.bind(trigger("m"), Action::Back);
        let list: Vec<String> = map
            .bindings()
            .iter()
            .map(|(t, a)| format!("{a} {t}"))
            .collect();
        assert_eq!(list, ["back m", "open a", "open z"]);
    }

    #[test]
    fn stale_pending_after_keymap_change() {
        let mut map = Keymap::empty();
        map.bind(trigger("g>g"), Action::CursorTop);
        let mut st = KeymapState::default();
        assert_eq!(feed(&mut st, &map, ev('g')), KeyOutcome::Pending);
        map.clear();
        assert_eq!(feed(&mut st, &map, ev('g')), KeyOutcome::Unbound);
        assert!(st.pending().is_empty());
    }
}
