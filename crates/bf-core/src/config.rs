//! The config file, Ghostty style.
//!
//! # Location
//!
//! `$XDG_CONFIG_HOME/better-finder/config`, or `~/.config/better-finder/config`
//! when `XDG_CONFIG_HOME` is not set. A missing file is not an error: the app
//! uses the defaults.
//!
//! # Syntax
//!
//! - One `key = value` on each line. Spaces around `=` are not significant.
//! - A line that starts with `#` (after optional spaces) is a comment. A `#`
//!   in a value is part of the value (`background = #1e1e2e`). There are no
//!   comments at the end of a line.
//! - A value in double quotes loses the quotes: `font-family = "SF Mono"`.
//! - An empty value resets the key to its default: `font-size =`.
//! - An unknown key or an invalid value gives a `Diagnostic`. The parser then
//!   continues and keeps the previous value of that key.
//! - `keybind` can occur many times. `keybind = clear` removes all bindings,
//!   the defaults too. `keybind = x=unbind` removes one binding.
//! - `config-file = <path>` loads one more file. A relative path is relative
//!   to the directory of the file that has the line. A `?` before the path
//!   makes the file optional (no diagnostic when it does not exist). Included
//!   files load after the current file is complete, in order. A cycle gives a
//!   diagnostic and stops at the repeated file.
//!
//! # Keys
//!
//! See `default_config_template` for the full list with defaults and help.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::color::Rgba;
use crate::filter::FilterMode;
use crate::keymap::Keymap;
use crate::sort::SortOptions;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    /// Follow the macOS appearance (light or dark).
    #[default]
    Auto,
    Light,
    Dark,
}

/// Text forms: `auto`, `light`, `dark`.
impl FromStr for ThemeMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let _ = s;
        todo!()
    }
}

impl fmt::Display for ThemeMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = f;
        todo!()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TitlebarStyle {
    /// Standard macOS title bar.
    Native,
    /// The title bar uses the theme background color.
    #[default]
    Transparent,
    /// No title text. The traffic light buttons stay.
    Hidden,
}

/// Text forms: `native`, `transparent`, `hidden`.
impl FromStr for TitlebarStyle {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let _ = s;
        todo!()
    }
}

impl fmt::Display for TitlebarStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = f;
        todo!()
    }
}

/// Columns after the name column. The name column is always first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Column {
    Size,
    Modified,
    Extension,
}

/// Text forms: `size`, `modified`, `extension`.
impl FromStr for Column {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let _ = s;
        todo!()
    }
}

impl fmt::Display for Column {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = f;
        todo!()
    }
}

/// All the colors that the UI draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub background: Rgba,
    pub foreground: Rgba,
    pub selection_background: Rgba,
    pub selection_foreground: Rgba,
    pub directory: Rgba,
    pub symlink: Rgba,
    pub executable: Rgba,
    /// Names of hidden entries.
    pub hidden: Rgba,
    /// Size and date columns.
    pub secondary: Rgba,
    pub status_background: Rgba,
    pub status_foreground: Rgba,
    /// Prompt text, filter text, pending key sequence.
    pub accent: Rgba,
}

impl Theme {
    pub fn default_dark() -> Self {
        Self {
            background: Rgba::rgb(0x18, 0x18, 0x1b),
            foreground: Rgba::rgb(0xe4, 0xe4, 0xe7),
            selection_background: Rgba::rgb(0x3b, 0x5b, 0xdb),
            selection_foreground: Rgba::rgb(0xff, 0xff, 0xff),
            directory: Rgba::rgb(0x7a, 0xa2, 0xf7),
            symlink: Rgba::rgb(0x7d, 0xcf, 0xff),
            executable: Rgba::rgb(0x9e, 0xce, 0x6a),
            hidden: Rgba::rgb(0x71, 0x71, 0x7a),
            secondary: Rgba::rgb(0xa1, 0xa1, 0xaa),
            status_background: Rgba::rgb(0x11, 0x11, 0x13),
            status_foreground: Rgba::rgb(0xa1, 0xa1, 0xaa),
            accent: Rgba::rgb(0xe0, 0xaf, 0x68),
        }
    }

    pub fn default_light() -> Self {
        Self {
            background: Rgba::rgb(0xff, 0xff, 0xff),
            foreground: Rgba::rgb(0x18, 0x18, 0x1b),
            selection_background: Rgba::rgb(0x25, 0x63, 0xeb),
            selection_foreground: Rgba::rgb(0xff, 0xff, 0xff),
            directory: Rgba::rgb(0x1d, 0x4e, 0xd8),
            symlink: Rgba::rgb(0x0e, 0x74, 0x90),
            executable: Rgba::rgb(0x15, 0x80, 0x3d),
            hidden: Rgba::rgb(0xa1, 0xa1, 0xaa),
            secondary: Rgba::rgb(0x71, 0x71, 0x7a),
            status_background: Rgba::rgb(0xf4, 0xf4, 0xf5),
            status_foreground: Rgba::rgb(0x52, 0x52, 0x5b),
            accent: Rgba::rgb(0xb4, 0x53, 0x09),
        }
    }
}

/// Color keys from the config file. A color that the user sets applies to
/// the light theme and to the dark theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ThemeOverrides {
    pub background: Option<Rgba>,
    pub foreground: Option<Rgba>,
    pub selection_background: Option<Rgba>,
    pub selection_foreground: Option<Rgba>,
    pub directory: Option<Rgba>,
    pub symlink: Option<Rgba>,
    pub executable: Option<Rgba>,
    pub hidden: Option<Rgba>,
    pub secondary: Option<Rgba>,
    pub status_background: Option<Rgba>,
    pub status_foreground: Option<Rgba>,
    pub accent: Option<Rgba>,
}

/// The parsed config. Field docs give the config key and the default.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// `font-family`. Default: none (the system font).
    pub font_family: Option<String>,
    /// `font-size`, in points. Default: `13`. Valid: 6 to 72.
    pub font_size: f64,
    /// `row-height`, in points. Default: none (`font_size * 1.7`, rounded).
    pub row_height: Option<f64>,
    /// `theme`. Default: `auto`.
    pub theme: ThemeMode,
    /// The color keys: `background`, `foreground`, `selection-background`,
    /// `selection-foreground`, `directory-color`, `symlink-color`,
    /// `executable-color`, `hidden-color`, `secondary-color`,
    /// `status-background`, `status-foreground`, `accent-color`.
    pub colors: ThemeOverrides,
    /// `background-opacity`. Default: `1.0`. Valid: 0.0 to 1.0.
    pub background_opacity: f64,
    /// `macos-titlebar-style`. Default: `transparent`.
    pub titlebar_style: TitlebarStyle,
    /// `window-width` and `window-height`, in points. Default: 1000 x 650.
    pub window_width: f64,
    pub window_height: f64,
    /// `show-hidden`. Default: `false`.
    pub show_hidden: bool,
    /// `show-icons`. Default: `true`.
    pub show_icons: bool,
    /// `sort-by` (default `name`), `sort-reverse` (default `false`),
    /// `directories-first` (default `true`).
    pub sort: SortOptions,
    /// `columns`, a comma-separated list. Default: `size,modified`.
    /// The value `none` shows only names. (An empty value resets to the default.)
    pub columns: Vec<Column>,
    /// `date-format`. Default: `%Y-%m-%d %H:%M`. See `format::format_time`.
    pub date_format: String,
    /// `filter-mode`. Default: `substring`.
    pub filter_mode: FilterMode,
    /// `start-directory`. Default: none (the home directory). Accepts `~`.
    /// A path on the command line wins over this key.
    pub start_directory: Option<PathBuf>,
    /// `terminal`: the app name for `open_terminal`. Default: `Terminal`.
    pub terminal: String,
    /// `auto-refresh`: reload the directory when it changes on disk. Default: `true`.
    pub auto_refresh: bool,
    /// `keybind`, repeatable. Default: `Keymap::default()`.
    pub keymap: Keymap,
}

impl Default for Config {
    fn default() -> Self {
        todo!()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// `None` for text from `Config::parse_str`.
    pub file: Option<PathBuf>,
    /// 1-based line number.
    pub line: usize,
    pub message: String,
}

/// `<file>:<line>: <message>`, or `line <line>: <message>` without a file.
impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = f;
        todo!()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadResult {
    pub config: Config,
    pub diagnostics: Vec<Diagnostic>,
    /// All files that loaded, the main file first. The app watches them.
    pub files: Vec<PathBuf>,
}

impl Config {
    /// See the module docs.
    pub fn default_path() -> PathBuf {
        todo!()
    }

    /// Loads `default_path()` and its includes.
    pub fn load() -> LoadResult {
        Self::load_from(&Self::default_path())
    }

    /// Loads `path` and its includes. A missing `path` gives the defaults and
    /// no diagnostic.
    pub fn load_from(path: &Path) -> LoadResult {
        let _ = path;
        todo!()
    }

    /// Parses text without a file. `config-file` lines give a diagnostic.
    pub fn parse_str(src: &str) -> (Config, Vec<Diagnostic>) {
        let _ = src;
        todo!()
    }

    /// The theme for the current appearance. `system_is_dark` is the macOS
    /// appearance; `theme = light` or `theme = dark` ignores it. The user
    /// color overrides apply on top.
    pub fn resolved_theme(&self, system_is_dark: bool) -> Theme {
        let _ = system_is_dark;
        todo!()
    }

    /// `row_height`, or the value from `font_size`.
    pub fn effective_row_height(&self) -> f64 {
        todo!()
    }
}

/// A complete config file with every key commented out at its default
/// value, and one or two lines of help above each key. The default bindings
/// show as commented `keybind` lines. The parse of this text (with the `#`
/// removed from each key line) must give `Config::default()`.
/// `+show-config --default` prints it. `open_config` writes it when the
/// config file does not exist.
pub fn default_config_template() -> String {
    todo!()
}
