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
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "light" => Ok(Self::Light),
            "dark" => Ok(Self::Dark),
            _ => Err(format!("unknown theme: {s} (use auto, light, dark)")),
        }
    }
}

impl fmt::Display for ThemeMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Auto => "auto",
            Self::Light => "light",
            Self::Dark => "dark",
        })
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
        match s.trim().to_ascii_lowercase().as_str() {
            "native" => Ok(Self::Native),
            "transparent" => Ok(Self::Transparent),
            "hidden" => Ok(Self::Hidden),
            _ => Err(format!(
                "unknown titlebar style: {s} (use native, transparent, hidden)"
            )),
        }
    }
}

impl fmt::Display for TitlebarStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Native => "native",
            Self::Transparent => "transparent",
            Self::Hidden => "hidden",
        })
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
        match s.trim().to_ascii_lowercase().as_str() {
            "size" => Ok(Self::Size),
            "modified" => Ok(Self::Modified),
            "extension" => Ok(Self::Extension),
            _ => Err(format!(
                "unknown column: {s} (use size, modified, extension)"
            )),
        }
    }
}

impl fmt::Display for Column {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Size => "size",
            Self::Modified => "modified",
            Self::Extension => "extension",
        })
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

const DEFAULT_FONT_SIZE: f64 = 13.0;
const DEFAULT_OPACITY: f64 = 1.0;
const DEFAULT_WINDOW_WIDTH: f64 = 1000.0;
const DEFAULT_WINDOW_HEIGHT: f64 = 650.0;
const DEFAULT_DATE_FORMAT: &str = "%Y-%m-%d %H:%M";
const DEFAULT_TERMINAL: &str = "Terminal";
const DEFAULT_COLUMNS: [Column; 2] = [Column::Size, Column::Modified];

impl Default for Config {
    fn default() -> Self {
        Self {
            font_family: None,
            font_size: DEFAULT_FONT_SIZE,
            row_height: None,
            theme: ThemeMode::default(),
            colors: ThemeOverrides::default(),
            background_opacity: DEFAULT_OPACITY,
            titlebar_style: TitlebarStyle::default(),
            window_width: DEFAULT_WINDOW_WIDTH,
            window_height: DEFAULT_WINDOW_HEIGHT,
            show_hidden: false,
            show_icons: true,
            sort: SortOptions::default(),
            columns: DEFAULT_COLUMNS.to_vec(),
            date_format: DEFAULT_DATE_FORMAT.to_string(),
            filter_mode: FilterMode::default(),
            start_directory: None,
            terminal: DEFAULT_TERMINAL.to_string(),
            auto_refresh: true,
            keymap: Keymap::default(),
        }
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
        match &self.file {
            Some(file) => write!(f, "{}:{}: {}", file.display(), self.line, self.message),
            None => write!(f, "line {}: {}", self.line, self.message),
        }
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
        let base = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => crate::fs::home_dir().join(".config"),
        };
        base.join("better-finder").join("config")
    }

    /// Loads `default_path()` and its includes.
    pub fn load() -> LoadResult {
        Self::load_from(&Self::default_path())
    }

    /// Loads `path` and its includes. A missing `path` gives the defaults and
    /// no diagnostic.
    pub fn load_from(path: &Path) -> LoadResult {
        let mut loader = Loader {
            config: Config::default(),
            diagnostics: Vec::new(),
            files: Vec::new(),
            stack: Vec::new(),
        };
        loader.load_file(path, None);
        LoadResult {
            config: loader.config,
            diagnostics: loader.diagnostics,
            files: loader.files,
        }
    }

    /// Parses text without a file. `config-file` lines give a diagnostic.
    pub fn parse_str(src: &str) -> (Config, Vec<Diagnostic>) {
        let mut config = Config::default();
        let mut diagnostics = Vec::new();
        let includes = config.parse_text(src, None, &mut diagnostics);
        for include in includes {
            diagnostics.push(Diagnostic {
                file: None,
                line: include.line,
                message: "config-file needs a file. It does not work in plain text".to_string(),
            });
        }
        (config, diagnostics)
    }

    /// The theme for the current appearance. `system_is_dark` is the macOS
    /// appearance; `theme = light` or `theme = dark` ignores it. The user
    /// color overrides apply on top.
    pub fn resolved_theme(&self, system_is_dark: bool) -> Theme {
        let dark = match self.theme {
            ThemeMode::Auto => system_is_dark,
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
        };
        let base = if dark {
            Theme::default_dark()
        } else {
            Theme::default_light()
        };
        let o = &self.colors;
        Theme {
            background: o.background.unwrap_or(base.background),
            foreground: o.foreground.unwrap_or(base.foreground),
            selection_background: o.selection_background.unwrap_or(base.selection_background),
            selection_foreground: o.selection_foreground.unwrap_or(base.selection_foreground),
            directory: o.directory.unwrap_or(base.directory),
            symlink: o.symlink.unwrap_or(base.symlink),
            executable: o.executable.unwrap_or(base.executable),
            hidden: o.hidden.unwrap_or(base.hidden),
            secondary: o.secondary.unwrap_or(base.secondary),
            status_background: o.status_background.unwrap_or(base.status_background),
            status_foreground: o.status_foreground.unwrap_or(base.status_foreground),
            accent: o.accent.unwrap_or(base.accent),
        }
    }

    /// `row_height`, or the value from `font_size`.
    pub fn effective_row_height(&self) -> f64 {
        self.row_height
            .unwrap_or_else(|| (self.font_size * 1.7).round())
    }

    /// Applies each line of `src`. Returns the `config-file` lines; the
    /// caller loads them.
    fn parse_text(
        &mut self,
        src: &str,
        file: Option<&Path>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Vec<Include> {
        let mut includes = Vec::new();
        for (index, raw) in src.lines().enumerate() {
            let line = index + 1;
            let text = raw.trim();
            if text.is_empty() || text.starts_with('#') {
                continue;
            }
            let Some((key, value)) = text.split_once('=') else {
                let message = format!("expected key = value, got \"{text}\"");
                diagnostics.push(Diagnostic {
                    file: file.map(Path::to_path_buf),
                    line,
                    message,
                });
                continue;
            };
            let (key, value) = (key.trim(), unquote(value.trim()));
            let result = if key == "config-file" {
                match value.strip_prefix('?').map(str::trim).unwrap_or(value) {
                    "" => Err("config-file needs a path".to_string()),
                    path => {
                        let optional = value.starts_with('?');
                        includes.push(Include {
                            path: path.to_string(),
                            optional,
                            line,
                        });
                        Ok(())
                    }
                }
            } else {
                self.apply(key, value)
            };
            if let Err(message) = result {
                diagnostics.push(Diagnostic {
                    file: file.map(Path::to_path_buf),
                    line,
                    message,
                });
            }
        }
        includes
    }

    /// Sets one key. An empty `value` resets the key to its default. On `Err`
    /// the old value stays.
    fn apply(&mut self, key: &str, value: &str) -> Result<(), String> {
        let empty = value.is_empty();
        match key {
            "font-family" => self.font_family = (!empty).then(|| value.to_string()),
            "font-size" => {
                self.font_size = if empty {
                    DEFAULT_FONT_SIZE
                } else {
                    number(key, value, 6.0, 72.0)?
                }
            }
            "row-height" => {
                self.row_height = if empty {
                    None
                } else {
                    Some(number(key, value, 12.0, 200.0)?)
                }
            }
            "theme" => {
                self.theme = if empty {
                    ThemeMode::default()
                } else {
                    value.parse()?
                }
            }
            "macos-titlebar-style" => {
                self.titlebar_style = if empty {
                    TitlebarStyle::default()
                } else {
                    value.parse()?
                }
            }
            "background-opacity" => {
                self.background_opacity = if empty {
                    DEFAULT_OPACITY
                } else {
                    number(key, value, 0.0, 1.0)?
                }
            }
            "window-width" => {
                self.window_width = if empty {
                    DEFAULT_WINDOW_WIDTH
                } else {
                    number(key, value, 200.0, f64::MAX)?
                }
            }
            "window-height" => {
                self.window_height = if empty {
                    DEFAULT_WINDOW_HEIGHT
                } else {
                    number(key, value, 200.0, f64::MAX)?
                }
            }
            "show-hidden" => self.show_hidden = if empty { false } else { boolean(key, value)? },
            "show-icons" => self.show_icons = if empty { true } else { boolean(key, value)? },
            "auto-refresh" => self.auto_refresh = if empty { true } else { boolean(key, value)? },
            "sort-by" => {
                self.sort.key = if empty {
                    SortOptions::default().key
                } else {
                    value.parse()?
                }
            }
            "sort-reverse" => self.sort.reverse = if empty { false } else { boolean(key, value)? },
            "directories-first" => {
                self.sort.directories_first = if empty { true } else { boolean(key, value)? }
            }
            "columns" => {
                self.columns = if empty {
                    DEFAULT_COLUMNS.to_vec()
                } else {
                    parse_columns(value)?
                }
            }
            "date-format" => {
                if !empty && !valid_date_format(value) {
                    return Err(format!("invalid date-format: {value}"));
                }
                self.date_format = if empty {
                    DEFAULT_DATE_FORMAT.to_string()
                } else {
                    value.to_string()
                };
            }
            "filter-mode" => {
                self.filter_mode = if empty {
                    FilterMode::default()
                } else {
                    value.parse()?
                }
            }
            "start-directory" => {
                self.start_directory =
                    (!empty).then(|| crate::fs::expand_path(value, &crate::fs::home_dir()))
            }
            "terminal" => {
                self.terminal = if empty {
                    DEFAULT_TERMINAL.to_string()
                } else {
                    value.to_string()
                }
            }
            "keybind" => {
                if value == "clear" {
                    self.keymap.clear();
                } else {
                    let (trigger, action) = Keymap::parse_binding(value)?;
                    self.keymap.bind(trigger, action);
                }
            }
            _ => {
                let slot = self
                    .color_slot(key)
                    .ok_or_else(|| format!("unknown key: {key}"))?;
                *slot = if empty {
                    None
                } else {
                    Some(value.parse::<Rgba>().map_err(|e| format!("{key}: {e}"))?)
                };
            }
        }
        Ok(())
    }

    fn color_slot(&mut self, key: &str) -> Option<&mut Option<Rgba>> {
        let c = &mut self.colors;
        Some(match key {
            "background" => &mut c.background,
            "foreground" => &mut c.foreground,
            "selection-background" => &mut c.selection_background,
            "selection-foreground" => &mut c.selection_foreground,
            "directory-color" => &mut c.directory,
            "symlink-color" => &mut c.symlink,
            "executable-color" => &mut c.executable,
            "hidden-color" => &mut c.hidden,
            "secondary-color" => &mut c.secondary,
            "status-background" => &mut c.status_background,
            "status-foreground" => &mut c.status_foreground,
            "accent-color" => &mut c.accent,
            _ => return None,
        })
    }
}

/// A `config-file` line.
struct Include {
    path: String,
    optional: bool,
    line: usize,
}

/// Loads a file and its includes into one `Config`.
struct Loader {
    config: Config,
    diagnostics: Vec<Diagnostic>,
    files: Vec<PathBuf>,
    /// The files that are open now. Used to find cycles.
    stack: Vec<PathBuf>,
}

impl Loader {
    /// `origin` is the file and line of the `config-file` line, if any.
    fn load_file(&mut self, path: &Path, origin: Option<(&Path, usize, bool)>) {
        let identity = std::fs::canonicalize(path).unwrap_or_else(|_| crate::fs::normalize(path));
        if self.stack.contains(&identity) {
            let (file, line, _) = origin.unzip_origin();
            self.diagnostics.push(Diagnostic {
                file,
                line,
                message: format!("config-file cycle: {} is already loaded", path.display()),
            });
            return;
        }
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) => {
                let optional = origin.is_none_or(|(_, _, optional)| optional);
                if err.kind() == std::io::ErrorKind::NotFound && optional {
                    return;
                }
                let (file, line, _) = origin.unzip_origin();
                self.diagnostics.push(Diagnostic {
                    file: file.or_else(|| Some(path.to_path_buf())),
                    line,
                    message: format!("cannot read config file {}: {err}", path.display()),
                });
                return;
            }
        };
        if !self.files.iter().any(|f| f == path) {
            self.files.push(path.to_path_buf());
        }
        self.stack.push(identity);
        let includes = self
            .config
            .parse_text(&text, Some(path), &mut self.diagnostics);
        let dir = path.parent().unwrap_or(Path::new("."));
        for include in includes {
            let target = crate::fs::expand_path(&include.path, dir);
            self.load_file(&target, Some((path, include.line, include.optional)));
        }
        self.stack.pop();
    }
}

trait OriginExt {
    fn unzip_origin(self) -> (Option<PathBuf>, usize, bool);
}

impl OriginExt for Option<(&Path, usize, bool)> {
    fn unzip_origin(self) -> (Option<PathBuf>, usize, bool) {
        match self {
            Some((file, line, optional)) => (Some(file.to_path_buf()), line, optional),
            None => (None, 1, false),
        }
    }
}

/// A value in double quotes loses the quotes.
fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}

fn boolean(key: &str, value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!(
            "invalid value for {key}: {value} (use true or false)"
        )),
    }
}

fn number(key: &str, value: &str, min: f64, max: f64) -> Result<f64, String> {
    match value.parse::<f64>() {
        Ok(n) if (min..=max).contains(&n) => Ok(n),
        Ok(_) if max == f64::MAX => Err(format!("{key} must be at least {min}, got {value}")),
        Ok(_) => Err(format!("{key} must be from {min} to {max}, got {value}")),
        Err(_) => Err(format!("invalid number for {key}: {value}")),
    }
}

fn parse_columns(value: &str) -> Result<Vec<Column>, String> {
    if value == "none" {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(|part| part.trim().parse::<Column>())
        .collect()
}

/// A `strftime` pattern that chrono accepts, or the word `relative`.
fn valid_date_format(pattern: &str) -> bool {
    use chrono::format::{Item, StrftimeItems};
    pattern == "relative" || !StrftimeItems::new(pattern).any(|item| matches!(item, Item::Error))
}

/// The config keys that have a line in the template, with the default
/// text, the help text, and the section title.
struct TemplateKey {
    key: &'static str,
    default: &'static str,
    help: &'static [&'static str],
}

const fn tk(
    key: &'static str,
    default: &'static str,
    help: &'static [&'static str],
) -> TemplateKey {
    TemplateKey { key, default, help }
}

const TEMPLATE_SECTIONS: &[(&str, &[TemplateKey])] = &[
    (
        "Font",
        &[
            tk(
                "font-family",
                "",
                &["The font name. Empty means the system font."],
            ),
            tk(
                "font-size",
                "13",
                &["The font size in points. Valid values: 6 to 72."],
            ),
            tk(
                "row-height",
                "",
                &[
                    "The row height in points. Valid values: 12 to 200.",
                    "Empty means the font size times 1.7, rounded.",
                ],
            ),
        ],
    ),
    (
        "Theme and colors",
        &[
            tk(
                "theme",
                "auto",
                &[
                    "The color theme: auto, light, or dark.",
                    "Auto follows the macOS appearance.",
                ],
            ),
            tk(
                "background",
                "",
                &[
                    "Color keys change the light theme and the dark theme.",
                    "Forms: #rgb, #rrggbb, #rrggbbaa. Empty means the theme color.",
                    "Example value: #1e1e2e",
                ],
            ),
            tk("foreground", "", &["The color of the file names."]),
            tk(
                "selection-background",
                "",
                &["The background of the row under the cursor."],
            ),
            tk(
                "selection-foreground",
                "",
                &["The text color of the row under the cursor."],
            ),
            tk("directory-color", "", &["The color of directory names."]),
            tk("symlink-color", "", &["The color of symlink names."]),
            tk(
                "executable-color",
                "",
                &["The color of executable file names."],
            ),
            tk("hidden-color", "", &["The color of hidden entry names."]),
            tk(
                "secondary-color",
                "",
                &["The color of the size and date columns."],
            ),
            tk(
                "status-background",
                "",
                &["The background of the status bar."],
            ),
            tk(
                "status-foreground",
                "",
                &["The text color of the status bar."],
            ),
            tk(
                "accent-color",
                "",
                &["The color of the prompt, the filter, and pending keys."],
            ),
        ],
    ),
    (
        "Window",
        &[
            tk(
                "background-opacity",
                "1",
                &["The opacity of the window background. Valid values: 0 to 1."],
            ),
            tk(
                "macos-titlebar-style",
                "transparent",
                &["The title bar: native, transparent, or hidden."],
            ),
            tk(
                "window-width",
                "1000",
                &["The width of a new window in points. The minimum is 200."],
            ),
            tk(
                "window-height",
                "650",
                &["The height of a new window in points. The minimum is 200."],
            ),
        ],
    ),
    (
        "List",
        &[
            tk(
                "show-icons",
                "true",
                &["Show the file icons: true or false."],
            ),
            tk(
                "columns",
                "size,modified",
                &[
                    "The columns after the name: size, modified, extension.",
                    "The word none shows only names.",
                ],
            ),
            tk(
                "date-format",
                "%Y-%m-%d %H:%M",
                &[
                    "The date format. Use a strftime pattern.",
                    "The word relative shows Today, Yesterday, and then the date.",
                ],
            ),
            tk(
                "sort-by",
                "name",
                &["The sort key: name, size, modified, or extension."],
            ),
            tk(
                "sort-reverse",
                "false",
                &["Reverse the sort order: true or false."],
            ),
            tk(
                "directories-first",
                "true",
                &["Show directories before files: true or false."],
            ),
        ],
    ),
    (
        "Behavior",
        &[
            tk(
                "show-hidden",
                "false",
                &["Show hidden entries at start: true or false."],
            ),
            tk(
                "filter-mode",
                "substring",
                &[
                    "The live filter: substring or fuzzy.",
                    "The filter ignores case unless the query has an uppercase letter.",
                ],
            ),
            tk(
                "start-directory",
                "",
                &[
                    "The folder to open at start. It accepts ~.",
                    "Empty means the home folder. A path on the command line wins.",
                ],
            ),
            tk(
                "terminal",
                "Terminal",
                &["The app that the open_terminal action starts."],
            ),
            tk(
                "auto-refresh",
                "true",
                &["Read the folder again when it changes on disk: true or false."],
            ),
        ],
    ),
];

fn push_help(out: &mut String, lines: &[&str]) {
    for line in lines {
        out.push_str("# ");
        out.push_str(line);
        out.push('\n');
    }
}

/// A complete config file with every key commented out at its default
/// value, and one or two lines of help above each key. The default bindings
/// show as commented `keybind` lines. The parse of this text (with the `#`
/// removed from each key line) must give `Config::default()`.
/// `+show-config --default` prints it. `open_config` writes it when the
/// config file does not exist.
pub fn default_config_template() -> String {
    let mut out = String::new();
    push_help(
        &mut out,
        &[
            "Better Finder config file.",
            "Each line below shows a key at its default value.",
            "Remove the # at the start of a key line to change the key.",
            "An empty value resets the key to its default.",
            "A line that starts with # is a comment.",
            "The key config-file loads one more file.",
            "Start the path with ? to make the file optional.",
        ],
    );
    for (title, keys) in TEMPLATE_SECTIONS {
        out.push('\n');
        push_help(&mut out, &[title]);
        for TemplateKey { key, default, help } in *keys {
            out.push('\n');
            push_help(&mut out, help);
            let line = format!("{key} = {default}");
            out.push_str("# ");
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    out.push('\n');
    push_help(&mut out, &["Keys"]);
    out.push('\n');
    push_help(
        &mut out,
        &[
            "A key binding has the form trigger=action.",
            "A trigger is one key or a sequence joined with >, for example g>g.",
            "Modifiers: cmd, ctrl, alt, shift. Join them with +, for example cmd+shift+g.",
            "The names plus, equal, and greater write the keys + = and >.",
            "The action unbind removes a binding, for example j=unbind.",
            "The line keybind = clear removes all bindings, the defaults too.",
            "Run the +list-actions command to see all actions.",
            "These are the default bindings.",
        ],
    );
    out.push('\n');
    for binding in crate::keymap::DEFAULT_BINDINGS {
        out.push_str("# keybind = ");
        out.push_str(binding);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> (Config, Vec<Diagnostic>) {
        Config::parse_str(src)
    }

    fn clean(src: &str) -> Config {
        let (config, diags) = parse(src);
        assert!(diags.is_empty(), "{diags:?}");
        config
    }

    fn rgb(s: &str) -> Rgba {
        s.parse().unwrap()
    }

    #[test]
    fn defaults() {
        let c = Config::default();
        assert_eq!(c.font_size, 13.0);
        assert_eq!(c.columns, [Column::Size, Column::Modified]);
        assert_eq!(c.effective_row_height(), 22.0);
        assert_eq!(clean(""), c);
    }

    #[test]
    fn comments_blank_lines_and_spaces() {
        let c =
            clean("# comment\n\n   # indented comment\nfont-size=15\n  show-hidden   =   true  \n");
        assert_eq!(c.font_size, 15.0);
        assert!(c.show_hidden);
    }

    #[test]
    fn quotes_and_hash_values() {
        let c =
            clean("font-family = \"SF Mono\"\nbackground = #1e1e2e\nforeground = \"#ffffff\"\n");
        assert_eq!(c.font_family.as_deref(), Some("SF Mono"));
        assert_eq!(c.colors.background, Some(rgb("#1e1e2e")));
        assert_eq!(c.colors.foreground, Some(rgb("#fff")));
    }

    #[test]
    fn empty_value_resets() {
        let c = clean(
            "font-size = 20\nfont-size =\nbackground = #000\nbackground =\nfont-family = X\nfont-family =\ncolumns = none\ncolumns =\nrow-height = 30\nrow-height =\n",
        );
        assert_eq!(c, Config::default());
    }

    #[test]
    fn unknown_key_has_line_number() {
        let (c, diags) = parse("font-size = 14\n\nbogus = 1\nshow-icons = false\n");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].line, 3);
        assert_eq!(diags[0].file, None);
        assert!(diags[0].message.contains("bogus"));
        // The parser goes on after the bad line.
        assert!(!c.show_icons);
        assert_eq!(c.font_size, 14.0);
    }

    #[test]
    fn line_without_equals() {
        let (_, diags) = parse("font-size\n");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].line, 1);
    }

    #[test]
    fn invalid_value_keeps_previous() {
        let src = "font-size = 20\nfont-size = 999\nfont-size = big\nbackground-opacity = 0.5\nbackground-opacity = 2\n\
                   window-width = 100\nwindow-height = 700\nrow-height = 5\nshow-hidden = yes\ntheme = blue\n\
                   sort-by = color\nbackground = #12\ncolumns = size,foo\ndate-format = %Q\nfilter-mode = x\nmacos-titlebar-style = big\n";
        let (c, diags) = parse(src);
        assert_eq!(diags.len(), 13, "{diags:#?}");
        assert_eq!(c.font_size, 20.0);
        assert_eq!(c.background_opacity, 0.5);
        assert_eq!(c.window_width, 1000.0);
        assert_eq!(c.window_height, 700.0);
        assert_eq!(c.row_height, None);
        assert!(!c.show_hidden);
        assert_eq!(c.theme, ThemeMode::Auto);
        assert_eq!(c.colors.background, None);
        assert_eq!(c.columns, [Column::Size, Column::Modified]);
        assert_eq!(c.date_format, "%Y-%m-%d %H:%M");
        assert_eq!(diags[0].line, 2);
        assert_eq!(diags[1].line, 3);
    }

    #[test]
    fn numeric_ranges() {
        assert_eq!(clean("font-size = 6").font_size, 6.0);
        assert_eq!(clean("font-size = 72").font_size, 72.0);
        assert_eq!(clean("font-size = 12.5").font_size, 12.5);
        assert_eq!(clean("background-opacity = 0").background_opacity, 0.0);
        assert_eq!(clean("row-height = 12").row_height, Some(12.0));
        assert_eq!(clean("window-width = 200").window_width, 200.0);
        for bad in [
            "font-size = 5.9",
            "font-size = 72.1",
            "font-size = nan",
            "background-opacity = -0.1",
            "row-height = 201",
            "window-height = 199",
        ] {
            assert_eq!(parse(bad).1.len(), 1, "{bad}");
        }
    }

    #[test]
    fn booleans_are_strict() {
        assert!(clean("show-hidden = true").show_hidden);
        assert!(!clean("show-icons = false").show_icons);
        for bad in ["show-hidden = True", "show-hidden = 1", "show-hidden = on"] {
            assert_eq!(parse(bad).1.len(), 1, "{bad}");
        }
    }

    #[test]
    fn columns_and_sort_keys() {
        assert_eq!(
            clean("columns = extension, size").columns,
            [Column::Extension, Column::Size]
        );
        assert!(clean("columns = none").columns.is_empty());
        assert_eq!(parse("columns = size,,modified").1.len(), 1);
        let c = clean("sort-by = size\nsort-reverse = true\ndirectories-first = false\n");
        assert_eq!(
            c.sort,
            SortOptions {
                key: crate::sort::SortKey::Size,
                reverse: true,
                directories_first: false
            }
        );
    }

    #[test]
    fn date_formats() {
        assert_eq!(clean("date-format = relative").date_format, "relative");
        assert_eq!(clean("date-format = %d/%m/%y").date_format, "%d/%m/%y");
        assert_eq!(parse("date-format = %").1.len(), 1);
    }

    #[test]
    fn start_directory_expands() {
        let c = clean("start-directory = ~/Downloads");
        assert_eq!(
            c.start_directory,
            Some(crate::fs::home_dir().join("Downloads"))
        );
        assert_eq!(
            clean("start-directory = /tmp/../usr").start_directory,
            Some(PathBuf::from("/usr"))
        );
    }

    #[test]
    fn all_color_keys() {
        let keys = [
            "background",
            "foreground",
            "selection-background",
            "selection-foreground",
            "directory-color",
            "symlink-color",
            "executable-color",
            "hidden-color",
            "secondary-color",
            "status-background",
            "status-foreground",
            "accent-color",
        ];
        let src: String = keys.iter().map(|k| format!("{k} = #010203\n")).collect();
        let c = clean(&src);
        let theme = c.resolved_theme(true);
        for color in [
            theme.background,
            theme.foreground,
            theme.selection_background,
            theme.selection_foreground,
            theme.directory,
            theme.symlink,
            theme.executable,
            theme.hidden,
            theme.secondary,
            theme.status_background,
            theme.status_foreground,
            theme.accent,
        ] {
            assert_eq!(color, Rgba::rgb(1, 2, 3));
        }
        assert_eq!(c.resolved_theme(false), theme);
    }

    #[test]
    fn theme_resolution() {
        let mut c = Config::default();
        assert_eq!(c.resolved_theme(true), Theme::default_dark());
        assert_eq!(c.resolved_theme(false), Theme::default_light());
        c.theme = ThemeMode::Light;
        assert_eq!(c.resolved_theme(true), Theme::default_light());
        c.theme = ThemeMode::Dark;
        assert_eq!(c.resolved_theme(false), Theme::default_dark());
        c.colors.accent = Some(Rgba::rgb(9, 9, 9));
        assert_eq!(c.resolved_theme(false).accent, Rgba::rgb(9, 9, 9));
        assert_eq!(
            c.resolved_theme(false).background,
            Theme::default_dark().background
        );
    }

    #[test]
    fn row_height() {
        let mut c = Config {
            font_size: 14.0,
            ..Config::default()
        };
        assert_eq!(c.effective_row_height(), 24.0);
        c.row_height = Some(30.0);
        assert_eq!(c.effective_row_height(), 30.0);
    }

    #[test]
    fn keybind_lines() {
        use crate::action::Action;
        use crate::keymap::{Key, KeyEvent, KeyOutcome, KeymapState, Mods};
        let ev = |c: char| KeyEvent {
            base: Key::Char(c),
            shifted: None,
            mods: Mods::default(),
        };
        let (c, diags) =
            parse("keybind = j=unbind\nkeybind = x=quit\nkeybind = bogus\nkeybind = y=nothing\n");
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].line, 3);
        let mut st = KeymapState::default();
        assert_eq!(st.feed(&c.keymap, &ev('j')), KeyOutcome::Unbound);
        assert_eq!(
            st.feed(&c.keymap, &ev('x')),
            KeyOutcome::Action(Action::Quit)
        );
        assert_eq!(
            st.feed(&c.keymap, &ev('k')),
            KeyOutcome::Action(Action::CursorUp)
        );

        let c = clean("keybind = clear\n");
        assert!(c.keymap.bindings().is_empty());
        let c = clean("keybind = clear\nkeybind = q=quit\n");
        assert_eq!(c.keymap.bindings().len(), 1);
    }

    #[test]
    fn diagnostic_display() {
        let d = Diagnostic {
            file: None,
            line: 4,
            message: "oops".into(),
        };
        assert_eq!(d.to_string(), "line 4: oops");
        let d = Diagnostic {
            file: Some("/a/config".into()),
            line: 2,
            message: "bad".into(),
        };
        assert_eq!(d.to_string(), "/a/config:2: bad");
    }

    #[test]
    fn enum_text_round_trips() {
        for t in [ThemeMode::Auto, ThemeMode::Light, ThemeMode::Dark] {
            assert_eq!(t.to_string().parse::<ThemeMode>().unwrap(), t);
        }
        for t in [
            TitlebarStyle::Native,
            TitlebarStyle::Transparent,
            TitlebarStyle::Hidden,
        ] {
            assert_eq!(t.to_string().parse::<TitlebarStyle>().unwrap(), t);
        }
        for t in [Column::Size, Column::Modified, Column::Extension] {
            assert_eq!(t.to_string().parse::<Column>().unwrap(), t);
        }
        assert!("x".parse::<ThemeMode>().is_err());
    }

    #[test]
    fn parse_str_rejects_config_file() {
        let (_, diags) = parse("font-size = 14\nconfig-file = other\n");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].line, 2);
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let r = Config::load_from(&dir.path().join("none"));
        assert_eq!(r.config, Config::default());
        assert!(r.diagnostics.is_empty());
        assert!(r.files.is_empty());
    }

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn load_with_relative_includes() {
        let dir = tempfile::tempdir().unwrap();
        let main = write(
            dir.path(),
            "config",
            "font-size = 14\nconfig-file = sub/extra\nfont-size = 15\nconfig-file = ?missing\n",
        );
        let extra = write(
            dir.path(),
            "sub/extra",
            "font-size = 20\nconfig-file = deeper\nshow-icons = false\n",
        );
        let deeper = write(dir.path(), "sub/deeper", "terminal = iTerm\n");
        let r = Config::load_from(&main);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        // Includes load after the including file, so they win.
        assert_eq!(r.config.font_size, 20.0);
        assert!(!r.config.show_icons);
        assert_eq!(r.config.terminal, "iTerm");
        assert_eq!(r.files, [main, extra, deeper]);
    }

    #[test]
    fn missing_required_include_gives_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        let main = write(
            dir.path(),
            "config",
            "font-size = 14\n\nconfig-file = nope\n",
        );
        let r = Config::load_from(&main);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].file.as_deref(), Some(main.as_path()));
        assert_eq!(r.diagnostics[0].line, 3);
        assert_eq!(r.config.font_size, 14.0);
    }

    #[test]
    fn optional_include_with_quotes_and_space() {
        let dir = tempfile::tempdir().unwrap();
        let main = write(dir.path(), "config", "config-file = ? local\n");
        assert!(Config::load_from(&main).diagnostics.is_empty());
        write(dir.path(), "local", "show-hidden = true\n");
        let r = Config::load_from(&main);
        assert!(r.config.show_hidden);
        assert_eq!(r.files.len(), 2);
    }

    #[test]
    fn include_cycle_gives_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a", "font-size = 14\nconfig-file = b\n");
        let b = write(dir.path(), "b", "show-hidden = true\n\nconfig-file = a\n");
        let r = Config::load_from(&a);
        assert_eq!(r.diagnostics.len(), 1, "{:?}", r.diagnostics);
        assert_eq!(r.diagnostics[0].file.as_deref(), Some(b.as_path()));
        assert_eq!(r.diagnostics[0].line, 3);
        assert!(r.diagnostics[0].message.contains("cycle"));
        assert!(r.config.show_hidden);
        assert_eq!(r.files, [a, b]);
    }

    #[test]
    fn self_include_is_a_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a", "config-file = a\n");
        let r = Config::load_from(&a);
        assert_eq!(r.diagnostics.len(), 1);
    }

    #[test]
    fn diagnostics_carry_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let main = write(dir.path(), "config", "config-file = inc\n");
        let inc = write(dir.path(), "inc", "# x\nbogus = 1\n");
        let r = Config::load_from(&main);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].file.as_deref(), Some(inc.as_path()));
        assert_eq!(r.diagnostics[0].line, 2);
        assert!(
            r.diagnostics[0]
                .to_string()
                .starts_with(&format!("{}:2: ", inc.display()))
        );
    }

    #[test]
    fn default_path_uses_xdg() {
        // Both cases run in one test: they change the environment.
        let saved = std::env::var_os("XDG_CONFIG_HOME");
        // SAFETY: this is the only test that touches XDG_CONFIG_HOME.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", "/xdg");
            assert_eq!(
                Config::default_path(),
                PathBuf::from("/xdg/better-finder/config")
            );
            std::env::set_var("XDG_CONFIG_HOME", "");
            assert_eq!(
                Config::default_path(),
                crate::fs::home_dir().join(".config/better-finder/config")
            );
            std::env::remove_var("XDG_CONFIG_HOME");
            assert_eq!(
                Config::default_path(),
                crate::fs::home_dir().join(".config/better-finder/config")
            );
            if let Some(v) = saved {
                std::env::set_var("XDG_CONFIG_HOME", v);
            }
        }
    }

    fn is_key_line(line: &str) -> bool {
        let Some(rest) = line.strip_prefix("# ") else {
            return false;
        };
        let name_len = rest
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || *c == '-')
            .count();
        name_len > 0 && rest[name_len..].starts_with(" =")
    }

    #[test]
    fn template_round_trips_to_default() {
        let template = default_config_template();
        let uncommented: String = template
            .lines()
            .filter(|l| is_key_line(l))
            .map(|l| format!("{}\n", &l[2..]))
            .collect();
        let (config, diags) = parse(&uncommented);
        assert!(diags.is_empty(), "{diags:#?}");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn template_has_every_key_and_binding() {
        let template = default_config_template();
        let key_lines: Vec<&str> = template.lines().filter(|l| is_key_line(l)).collect();
        let keys: Vec<&str> = key_lines
            .iter()
            .map(|l| l[2..].split(' ').next().unwrap())
            .collect();
        let expected = [
            "font-family",
            "font-size",
            "row-height",
            "theme",
            "background",
            "foreground",
            "selection-background",
            "selection-foreground",
            "directory-color",
            "symlink-color",
            "executable-color",
            "hidden-color",
            "secondary-color",
            "status-background",
            "status-foreground",
            "accent-color",
            "background-opacity",
            "macos-titlebar-style",
            "window-width",
            "window-height",
            "show-icons",
            "columns",
            "date-format",
            "sort-by",
            "sort-reverse",
            "directories-first",
            "show-hidden",
            "filter-mode",
            "start-directory",
            "terminal",
            "auto-refresh",
        ];
        for key in expected {
            assert!(keys.contains(&key), "missing {key}");
        }
        let binds = keys.iter().filter(|k| **k == "keybind").count();
        assert_eq!(binds, crate::keymap::DEFAULT_BINDINGS.len());
        assert_eq!(keys.len(), expected.len() + binds);
        for line in template.lines() {
            assert!(line.is_empty() || line.starts_with("# "), "{line}");
        }
    }
}
