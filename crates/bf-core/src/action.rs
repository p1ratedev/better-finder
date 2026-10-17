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
    /// Show or hide the sidebar.
    ToggleSidebar,
    /// Move the keyboard focus to the sidebar, or back to the list.
    FocusSidebar,

    // File operations.
    Rename,
    NewFolder,
    /// Move the entry under the cursor to the Trash. This is recoverable.
    Trash,
    CopyPath,
    CopyName,
    /// Put the entry under the cursor on the pasteboard as a file, so that
    /// a paste in Finder or in another app gets the file.
    Copy,
    /// Copy the files on the pasteboard into the current directory. It
    /// never replaces an item.
    Paste,
    /// Copy the entry under the cursor in its directory: `report copy.pdf`.
    Duplicate,
    /// Make a Finder alias file of the entry under the cursor:
    /// `report.pdf alias`.
    MakeAlias,
    /// Make a zip archive of the entry under the cursor: `report.pdf.zip`.
    Compress,
    /// Show the Finder Info window of the entry under the cursor, or of the
    /// current directory.
    GetInfo,
    /// Show the macOS share menu for the entry under the cursor.
    Share,
    /// Open the configured terminal app in the current directory.
    OpenTerminal,
    RevealInFinder,
    /// Eject a volume: the selected sidebar row, the volume under the
    /// cursor, or the volume of the current folder.
    Eject,

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
        CATALOG
    }

    /// One-line description for help output.
    pub fn description(&self) -> &'static str {
        let name = match self {
            Self::Goto(_) => "goto:<path>".to_string(),
            Self::Sort(_) => "sort:<key>".to_string(),
            other => other.to_string(),
        };
        CATALOG
            .iter()
            .find(|(n, _)| *n == name)
            .map_or("", |(_, d)| *d)
    }
}

impl FromStr for Action {
    type Err = ParseActionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let (name, param) = match s.split_once(':') {
            Some((name, param)) => (name.trim(), Some(param.trim())),
            None => (s, None),
        };
        let err = ParseActionError;
        let simple = match name {
            "goto" | "sort" => {
                let param = param.filter(|p| !p.is_empty()).ok_or_else(|| {
                    err(format!(
                        "{name} needs a parameter, for example {name}:<value>"
                    ))
                })?;
                return if name == "goto" {
                    Ok(Self::Goto(param.to_string()))
                } else {
                    param.parse().map(Self::Sort).map_err(err)
                };
            }
            "cursor_up" => Self::CursorUp,
            "cursor_down" => Self::CursorDown,
            "cursor_top" => Self::CursorTop,
            "cursor_bottom" => Self::CursorBottom,
            "page_up" => Self::PageUp,
            "page_down" => Self::PageDown,
            "half_page_up" => Self::HalfPageUp,
            "half_page_down" => Self::HalfPageDown,
            "open" => Self::Open,
            "parent" => Self::Parent,
            "back" => Self::Back,
            "forward" => Self::Forward,
            "goto_prompt" => Self::GotoPrompt,
            "quick_look" => Self::QuickLook,
            "toggle_hidden" => Self::ToggleHidden,
            "filter" => Self::Filter,
            "cancel" => Self::Cancel,
            "toggle_sort_reverse" => Self::ToggleSortReverse,
            "refresh" => Self::Refresh,
            "help" => Self::Help,
            "toggle_sidebar" => Self::ToggleSidebar,
            "focus_sidebar" => Self::FocusSidebar,
            "rename" => Self::Rename,
            "new_folder" => Self::NewFolder,
            "trash" => Self::Trash,
            "copy_path" => Self::CopyPath,
            "copy_name" => Self::CopyName,
            "copy" => Self::Copy,
            "paste" => Self::Paste,
            "duplicate" => Self::Duplicate,
            "make_alias" => Self::MakeAlias,
            "compress" => Self::Compress,
            "get_info" => Self::GetInfo,
            "share" => Self::Share,
            "open_terminal" => Self::OpenTerminal,
            "reveal_in_finder" => Self::RevealInFinder,
            "eject" => Self::Eject,
            "open_config" => Self::OpenConfig,
            "reload_config" => Self::ReloadConfig,
            "quit" => Self::Quit,
            "unbind" => Self::Unbind,
            _ => return Err(err(s.to_string())),
        };
        if param.is_some() {
            return Err(err(format!("{name} does not take a parameter")));
        }
        Ok(simple)
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::CursorUp => "cursor_up",
            Self::CursorDown => "cursor_down",
            Self::CursorTop => "cursor_top",
            Self::CursorBottom => "cursor_bottom",
            Self::PageUp => "page_up",
            Self::PageDown => "page_down",
            Self::HalfPageUp => "half_page_up",
            Self::HalfPageDown => "half_page_down",
            Self::Open => "open",
            Self::Parent => "parent",
            Self::Back => "back",
            Self::Forward => "forward",
            Self::Goto(path) => return write!(f, "goto:{path}"),
            Self::GotoPrompt => "goto_prompt",
            Self::QuickLook => "quick_look",
            Self::ToggleHidden => "toggle_hidden",
            Self::Filter => "filter",
            Self::Cancel => "cancel",
            Self::Sort(key) => return write!(f, "sort:{key}"),
            Self::ToggleSortReverse => "toggle_sort_reverse",
            Self::Refresh => "refresh",
            Self::Help => "help",
            Self::ToggleSidebar => "toggle_sidebar",
            Self::FocusSidebar => "focus_sidebar",
            Self::Rename => "rename",
            Self::NewFolder => "new_folder",
            Self::Trash => "trash",
            Self::CopyPath => "copy_path",
            Self::CopyName => "copy_name",
            Self::Copy => "copy",
            Self::Paste => "paste",
            Self::Duplicate => "duplicate",
            Self::MakeAlias => "make_alias",
            Self::Compress => "compress",
            Self::GetInfo => "get_info",
            Self::Share => "share",
            Self::OpenTerminal => "open_terminal",
            Self::RevealInFinder => "reveal_in_finder",
            Self::Eject => "eject",
            Self::OpenConfig => "open_config",
            Self::ReloadConfig => "reload_config",
            Self::Quit => "quit",
            Self::Unbind => "unbind",
        };
        f.write_str(name)
    }
}

static CATALOG: &[(&str, &str)] = &[
    ("cursor_up", "Move the cursor up one row."),
    ("cursor_down", "Move the cursor down one row."),
    ("cursor_top", "Move the cursor to the first entry."),
    ("cursor_bottom", "Move the cursor to the last entry."),
    ("page_up", "Move the cursor up one page."),
    ("page_down", "Move the cursor down one page."),
    ("half_page_up", "Move the cursor up half a page."),
    ("half_page_down", "Move the cursor down half a page."),
    (
        "open",
        "Enter the directory, or open the file with its default app.",
    ),
    ("parent", "Go to the parent directory."),
    ("back", "Go to the previous directory in the history."),
    ("forward", "Go to the next directory in the history."),
    ("goto:<path>", "Go to a directory. The path accepts ~."),
    ("goto_prompt", "Show the go to folder prompt."),
    ("quick_look", "Show or hide the Quick Look panel."),
    ("toggle_hidden", "Show or hide hidden entries."),
    (
        "filter",
        "Focus the filter field. A path in the field goes to a folder.",
    ),
    ("cancel", "Cancel the most recent mode."),
    (
        "sort:<key>",
        "Set the sort key: name, size, modified, created, or extension.",
    ),
    ("toggle_sort_reverse", "Reverse the sort order."),
    ("refresh", "Read the directory again."),
    ("help", "Show the key binding help."),
    ("toggle_sidebar", "Show or hide the sidebar."),
    (
        "focus_sidebar",
        "Move the keyboard focus to the sidebar, or back to the list.",
    ),
    ("rename", "Rename the entry under the cursor."),
    ("new_folder", "Make a new folder."),
    ("trash", "Move the entry under the cursor to the Trash."),
    (
        "copy_path",
        "Copy the full path of the entry to the clipboard.",
    ),
    ("copy_name", "Copy the name of the entry to the clipboard."),
    (
        "copy",
        "Copy the entry as a file, for a paste here or in another app.",
    ),
    (
        "paste",
        "Paste the copied files into this folder. It never replaces a file.",
    ),
    ("duplicate", "Make a copy of the entry in its folder."),
    ("make_alias", "Make a Finder alias of the entry."),
    ("compress", "Make a zip archive of the entry."),
    (
        "get_info",
        "Show the Finder Info window of the entry or the folder.",
    ),
    ("share", "Show the share menu for the entry."),
    (
        "open_terminal",
        "Open the terminal app in the current directory.",
    ),
    ("reveal_in_finder", "Show the entry in Finder."),
    (
        "eject",
        "Eject the volume of the sidebar row, the cursor, or the current folder.",
    ),
    ("open_config", "Open the config file."),
    ("reload_config", "Read the config file again."),
    ("quit", "Quit the app."),
    (
        "unbind",
        "Remove the binding for the trigger. Use it in a keybind line.",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn all_actions() -> Vec<Action> {
        CATALOG
            .iter()
            .map(|(name, _)| match *name {
                "goto:<path>" => "goto:~/x".parse().unwrap(),
                "sort:<key>" => "sort:size".parse().unwrap(),
                n => n.parse().unwrap(),
            })
            .collect()
    }

    #[test]
    fn round_trip_all() {
        for a in all_actions() {
            assert_eq!(a.to_string().parse::<Action>().unwrap(), a);
            assert!(!a.description().is_empty(), "{a}");
        }
        assert_eq!(all_actions().len(), 43);
    }

    #[test]
    fn parameters() {
        assert_eq!(
            "goto:~/Downloads".parse(),
            Ok(Action::Goto("~/Downloads".into()))
        );
        assert_eq!(" goto : /a:b ".parse(), Ok(Action::Goto("/a:b".into())));
        assert_eq!("sort:size".parse(), Ok(Action::Sort(SortKey::Size)));
        assert!("goto".parse::<Action>().is_err());
        assert!("goto:".parse::<Action>().is_err());
        assert!("sort".parse::<Action>().is_err());
        assert!("sort:bogus".parse::<Action>().is_err());
        assert!("quit:now".parse::<Action>().is_err());
        assert!("bogus".parse::<Action>().is_err());
        assert!("".parse::<Action>().is_err());
    }

    #[test]
    fn file_actions() {
        for (text, action) in [
            ("copy", Action::Copy),
            ("paste", Action::Paste),
            ("duplicate", Action::Duplicate),
            ("make_alias", Action::MakeAlias),
            ("compress", Action::Compress),
            ("get_info", Action::GetInfo),
            ("share", Action::Share),
        ] {
            assert_eq!(text.parse(), Ok(action.clone()));
            assert_eq!(action.to_string(), text);
            assert!(!action.description().is_empty(), "{text}");
        }
        assert!("paste:here".parse::<Action>().is_err());
    }

    #[test]
    fn trims_whitespace() {
        assert_eq!("  cursor_down ".parse(), Ok(Action::CursorDown));
    }
}
