//! Actions: the things a key binding can do.
//!
//! Text form (config file, CLI output): `snake_case` name, with an optional
//! parameter after a colon. Examples: `cursor_down`, `goto:~/Downloads`,
//! `sort:size`. `Display` and `FromStr` must round-trip.

use std::fmt;
use std::str::FromStr;

use crate::sort::SortKey;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Action {
    // Cursor movement.
    CursorUp,
    CursorDown,
    CursorTop,
    CursorBottom,
    PageUp,
    PageDown,
    HalfPageUp,
    HalfPageDown,

    // Navigation.
    /// Enter the directory under the cursor, or open the file with its default app.
    Open,
    /// Go to the parent directory. The cursor lands on the directory you came from.
    Parent,
    Back,
    Forward,
    /// `goto:<path>`. The path accepts `~` and paths relative to the current directory.
    Goto(String),
    /// Show the "go to folder" prompt.
    GotoPrompt,

    // View.
    /// Show or hide the macOS Quick Look panel for the entry under the cursor.
    QuickLook,
    ToggleHidden,
    /// Show the filter prompt. Typing filters the list live.
    Filter,
    /// Cancel the most recent mode: key sequence, then Quick Look, then filter.
    Cancel,
    /// `sort:<key>`. Sets the sort key. The direction (`reverse`) does not change.
    Sort(SortKey),
    ToggleSortReverse,
    Refresh,
    /// Show the key binding help overlay.
    Help,

    // File operations.
    Rename,
    NewFolder,
    /// Move the entry under the cursor to the Trash. This is recoverable.
    Trash,
    CopyPath,
    CopyName,
    /// Open the configured terminal app in the current directory.
    OpenTerminal,
    RevealInFinder,

    // App.
    /// Open the config file. Create it from the template when it does not exist.
    OpenConfig,
    ReloadConfig,
    Quit,

    /// Only valid in a `keybind` line: remove the binding for the trigger.
    Unbind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseActionError(pub String);

impl fmt::Display for ParseActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown action: {}", self.0)
    }
}

impl std::error::Error for ParseActionError {}

impl Action {
    /// All action names in their text form. Parameterized actions show the
    /// parameter as a placeholder, for example `goto:<path>`.
    /// The `+list-actions` CLI command prints this list.
    pub fn catalog() -> &'static [(&'static str, &'static str)] {
        todo!("(name, one-line description) for each action")
    }

    /// One-line description for help output.
    pub fn description(&self) -> &'static str {
        todo!()
    }
}

impl FromStr for Action {
    type Err = ParseActionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let _ = s;
        todo!()
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = f;
        todo!()
    }
}
