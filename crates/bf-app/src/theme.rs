//! Theme: `Rgba` to `NSColor`, fonts, row metrics, and cached text attributes.
//!
//! `Style::new` builds all objects once. The draw code only reads them. A
//! config reload or an appearance change builds a new `Style`.

use std::time::SystemTime;

use bf_core::config::{Column, Config, Theme};
use bf_core::{Entry, EntryKind, Rgba};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSColor, NSFont,
    NSFontAttributeName, NSFontWeightRegular, NSForegroundColorAttributeName, NSLineBreakMode,
    NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSStringDrawing, NSTextAlignment,
};
use objc2_foundation::{NSArray, NSAttributedStringKey, NSDictionary, NSString};

/// A text attribute dictionary for `drawInRect:withAttributes:`.
pub type Attrs = Retained<NSDictionary<NSAttributedStringKey, AnyObject>>;

/// The space between two columns, in points.
pub const COLUMN_GAP: f64 = 20.0;
/// The space on the left of the list.
pub const LEFT_PAD: f64 = 14.0;
/// The space on the right of the list.
pub const RIGHT_PAD: f64 = 16.0;
/// The space between the icon and the name.
pub const ICON_GAP: f64 = 8.0;
/// The name column hides other columns when it gets narrower than this.
pub const MIN_NAME_WIDTH: f64 = 160.0;

/// Converts a theme color to an sRGB `NSColor`.
pub fn color(c: Rgba) -> Retained<NSColor> {
    let (r, g, b, a) = c.to_f64();
    NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a)
}

/// Converts a theme color and multiplies its alpha by `alpha`.
pub fn color_alpha(c: Rgba, alpha: f64) -> Retained<NSColor> {
    let (r, g, b, a) = c.to_f64();
    NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a * alpha.clamp(0.0, 1.0))
}

/// True when `appearance` is a dark appearance.
pub fn is_dark(appearance: &NSAppearance) -> bool {
    // SAFETY: The appearance names are immutable statics from AppKit.
    let (aqua, dark) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
    let names = NSArray::from_slice(&[aqua, dark]);
    appearance
        .bestMatchFromAppearancesWithNames(&names)
        .is_some_and(|best| &*best == dark)
}

/// A paragraph style with one line break mode and one alignment.
fn paragraph(mode: NSLineBreakMode, align: NSTextAlignment) -> Retained<NSMutableParagraphStyle> {
    let p = NSMutableParagraphStyle::new();
    p.setLineBreakMode(mode);
    p.setAlignment(align);
    p
}

/// Makes an attribute dictionary with a font, a color, and a paragraph style.
pub fn attrs(font: &NSFont, color: &NSColor, para: &NSMutableParagraphStyle) -> Attrs {
    // SAFETY: The attribute keys are immutable statics from AppKit.
    let keys: [&NSAttributedStringKey; 3] = unsafe {
        [
            NSFontAttributeName,
            NSForegroundColorAttributeName,
            NSParagraphStyleAttributeName,
        ]
    };
    let objects: [&AnyObject; 3] = [font, color, para];
    NSDictionary::from_slices(&keys, &objects)
}

/// The size of `text` when it draws with `attrs`.
pub fn text_width(text: &str, attrs: &Attrs) -> f64 {
    // SAFETY: `attrs` has only string keys with the correct value types.
    unsafe { NSString::from_str(text).sizeWithAttributes(Some(attrs)) }.width
}

/// The height of one line of text in `font`.
pub fn line_height(font: &NSFont) -> f64 {
    (font.ascender() - font.descender() + font.leading()).ceil()
}

/// One column after the name column, with its width in points.
#[derive(Clone, Debug)]
pub struct ColumnSpec {
    pub column: Column,
    pub width: f64,
}

/// The text attributes for the name column.
#[derive(Clone)]
pub struct NameAttrs {
    pub normal: Attrs,
    pub directory: Attrs,
    pub symlink: Attrs,
    pub executable: Attrs,
    pub hidden: Attrs,
    pub selected: Attrs,
}

impl NameAttrs {
    /// The attributes for `entry`. The order of the rules: directory,
    /// symlink, executable, hidden, foreground. A package (`.app`) shows as
    /// a file, like in Finder.
    pub fn for_entry(&self, entry: &Entry, selected: bool) -> &Attrs {
        if selected {
            &self.selected
        } else if entry.kind == EntryKind::Directory && !entry.is_package {
            &self.directory
        } else if entry.kind == EntryKind::Symlink {
            &self.symlink
        } else if entry.executable {
            &self.executable
        } else if entry.hidden {
            &self.hidden
        } else {
            &self.normal
        }
    }
}

/// All the derived look values: colors, fonts, metrics, attributes.
#[derive(Clone)]
pub struct Style {
    pub theme: Theme,
    pub dark: bool,
    pub opacity: f64,
    pub font: Retained<NSFont>,
    pub row_height: f64,
    pub line_height: f64,
    pub status_line_height: f64,
    pub status_height: f64,
    pub prompt_height: f64,
    pub icon_size: f64,
    pub show_icons: bool,
    pub columns: Vec<ColumnSpec>,
    pub date_format: String,

    pub window_bg: Retained<NSColor>,
    pub selection_bg: Retained<NSColor>,
    pub status_bg: Retained<NSColor>,
    pub panel_bg: Retained<NSColor>,
    pub border: Retained<NSColor>,
    pub foreground: Retained<NSColor>,
    pub accent: Retained<NSColor>,

    pub name: NameAttrs,
    pub column: Attrs,
    pub column_selected: Attrs,
    pub empty: Attrs,
    pub status: Attrs,
    pub status_accent: Attrs,
    pub help_title: Attrs,
    pub help_key: Attrs,
    pub help_action: Attrs,
    pub help_desc: Attrs,
    /// The font of the status bar.
    pub status_font: Retained<NSFont>,
}

impl Style {
    /// Builds the style. The second value lists problems for the status bar
    /// (for example a font that does not exist).
    pub fn new(config: &Config, system_is_dark: bool) -> (Style, Vec<String>) {
        let mut warnings = Vec::new();
        let theme = config.resolved_theme(system_is_dark);
        let dark = match config.theme {
            bf_core::config::ThemeMode::Auto => system_is_dark,
            bf_core::config::ThemeMode::Dark => true,
            bf_core::config::ThemeMode::Light => false,
        };
        let opacity = config.background_opacity.clamp(0.0, 1.0);
        let size = config.font_size;
        let status_size = (size - 1.0).max(9.0);

        let custom = config.font_family.as_deref().and_then(|name| {
            let font = NSFont::fontWithName_size(&NSString::from_str(name), size);
            if font.is_none() {
                warnings.push(format!("font-family: no font named \"{name}\""));
            }
            font
        });
        let font = custom
            .clone()
            .unwrap_or_else(|| NSFont::systemFontOfSize(size));
        // SAFETY: `NSFontWeightRegular` is an immutable static from AppKit.
        let regular = unsafe { NSFontWeightRegular };
        let column_font = match &custom {
            Some(f) => f.clone(),
            None => NSFont::monospacedDigitSystemFontOfSize_weight(size, regular),
        };
        let status_font = match config.font_family.as_deref() {
            Some(name) if custom.is_some() => {
                NSFont::fontWithName_size(&NSString::from_str(name), status_size)
                    .unwrap_or_else(|| NSFont::systemFontOfSize(status_size))
            }
            _ => NSFont::monospacedDigitSystemFontOfSize_weight(status_size, regular),
        };

        let row_height = config
            .effective_row_height()
            .max(line_height(&font) + 2.0)
            .round();
        let line_h = line_height(&font);
        let status_line_height = line_height(&status_font);
        let status_height = (status_line_height + 10.0).round();
        let prompt_height = (line_h + 14.0).round();
        let icon_size = (row_height - 6.0).clamp(12.0, 32.0).round();

        let middle = paragraph(NSLineBreakMode::ByTruncatingMiddle, NSTextAlignment::Left);
        let tail = paragraph(NSLineBreakMode::ByTruncatingTail, NSTextAlignment::Left);
        let right = paragraph(NSLineBreakMode::ByTruncatingTail, NSTextAlignment::Right);
        let center = paragraph(NSLineBreakMode::ByTruncatingTail, NSTextAlignment::Center);

        let name_attrs = |c: Rgba| attrs(&font, &color(c), &middle);
        let name = NameAttrs {
            normal: name_attrs(theme.foreground),
            directory: name_attrs(theme.directory),
            symlink: name_attrs(theme.symlink),
            executable: name_attrs(theme.executable),
            hidden: name_attrs(theme.hidden),
            selected: name_attrs(theme.selection_foreground),
        };
        let column = attrs(&column_font, &color(theme.secondary), &right);
        let column_selected = attrs(
            &column_font,
            &color_alpha(theme.selection_foreground, 0.85),
            &right,
        );
        let empty = attrs(&font, &color(theme.secondary), &center);
        let status = attrs(&status_font, &color(theme.status_foreground), &tail);
        let status_accent = attrs(&status_font, &color(theme.accent), &tail);

        let bold = NSFont::boldSystemFontOfSize(size + 1.0);
        let help_title = attrs(&bold, &color(theme.foreground), &tail);
        let help_key = attrs(&column_font, &color(theme.accent), &tail);
        let help_action = attrs(&font, &color(theme.foreground), &tail);
        let help_desc = attrs(&font, &color(theme.secondary), &tail);

        let columns = column_specs(config, &column);

        let style = Style {
            theme,
            dark,
            opacity,
            font: font.clone(),
            row_height,
            line_height: line_h,
            status_line_height,
            status_height,
            prompt_height,
            icon_size,
            show_icons: config.show_icons,
            columns,
            date_format: config.date_format.clone(),
            window_bg: color_alpha(theme.background, opacity),
            selection_bg: color(theme.selection_background),
            status_bg: color_alpha(theme.status_background, opacity),
            panel_bg: color(theme.status_background),
            border: color_alpha(theme.secondary, 0.35),
            foreground: color(theme.foreground),
            accent: color(theme.accent),
            name,
            column,
            column_selected,
            empty,
            status,
            status_accent,
            help_title,
            help_key,
            help_action,
            help_desc,
            status_font,
        };
        (style, warnings)
    }

    /// The x position where the name starts.
    pub fn name_x(&self) -> f64 {
        if self.show_icons {
            LEFT_PAD + self.icon_size + ICON_GAP
        } else {
            LEFT_PAD
        }
    }

    /// The columns that fit in `width`. Columns on the left go first when
    /// the name column gets too narrow.
    pub fn visible_columns(&self, width: f64) -> &[ColumnSpec] {
        let mut cols: &[ColumnSpec] = &self.columns;
        while !cols.is_empty() {
            let used: f64 = cols.iter().map(|c| c.width + COLUMN_GAP).sum();
            if width - RIGHT_PAD - used - self.name_x() >= MIN_NAME_WIDTH {
                break;
            }
            cols = &cols[1..];
        }
        cols
    }
}

/// Measures the width of each configured column.
fn column_specs(config: &Config, attrs: &Attrs) -> Vec<ColumnSpec> {
    let widest = |samples: &[&str]| {
        samples
            .iter()
            .map(|s| text_width(s, attrs))
            .fold(0.0_f64, f64::max)
            .ceil()
            + 2.0
    };
    config
        .columns
        .iter()
        .map(|&column| {
            let width = match column {
                Column::Size => widest(&["999.9 MB", "999 bytes", "--"]),
                Column::Modified => {
                    if config.date_format == "relative" {
                        widest(&["Yesterday 23:59", "Today 23:59", "2026-12-28"])
                    } else {
                        let sample = sample_time();
                        let text =
                            bf_core::format::format_time(sample, &config.date_format, sample);
                        widest(&[text.as_str(), "--"]).min(320.0)
                    }
                }
                Column::Extension => widest(&["xxxxxxx"]),
            };
            ColumnSpec { column, width }
        })
        .collect()
}

/// A wide sample date (December 28, 23:58) to measure the date column.
fn sample_time() -> SystemTime {
    use chrono::{Local, TimeZone};
    Local
        .with_ymd_and_hms(2026, 12, 28, 23, 58, 58)
        .single()
        .map(SystemTime::from)
        .unwrap_or_else(SystemTime::now)
}
