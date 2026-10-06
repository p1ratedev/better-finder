//! Theme: colors, fonts, row metrics, and cached text attributes.
//!
//! - `theme = system` (the default) uses the macOS semantic colors, like
//!   Finder. These `NSColor` objects are dynamic: they resolve at draw time
//!   for the appearance and the accent color of the view.
//! - `theme = auto | light | dark` use the custom palette from `bf-core`.
//! - A color key in the config file replaces that color in all themes.
//!
//! `Style::new` builds all objects once. The draw code only reads them. A
//! config reload or an appearance change builds a new `Style`.

use bf_core::Rgba;
use bf_core::config::{Column, Config, Theme, ThemeMode};
use bf_core::{Entry, EntryKind};
use objc2::Message;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSColor, NSFont,
    NSFontAttributeName, NSFontWeightRegular, NSFontWeightSemibold, NSForegroundColorAttributeName,
    NSLineBreakMode, NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSStringDrawing,
    NSTextAlignment,
};
use objc2_foundation::{NSArray, NSAttributedStringKey, NSDictionary, NSString};

/// A text attribute dictionary for `drawInRect:withAttributes:`.
pub type Attrs = Retained<NSDictionary<NSAttributedStringKey, AnyObject>>;

/// The space on the left of the list, before the icon.
pub const LEFT_PAD: f64 = 16.0;
/// The space on the right of the list.
pub const RIGHT_PAD: f64 = 16.0;
/// The space between the icon and the name.
pub const ICON_GAP: f64 = 6.0;
/// The space between two columns.
pub const COLUMN_GAP: f64 = 12.0;
/// The cursor row rectangle: inset from the left and right edges, radius.
pub const SELECTION_INSET: f64 = 8.0;
pub const SELECTION_RADIUS: f64 = 5.0;
/// The name column hides other columns when it gets narrower than this.
pub const MIN_NAME_WIDTH: f64 = 160.0;
/// The height of the column header and of the path and status bar.
pub const BAR_HEIGHT: f64 = 24.0;

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

/// The width of `text` when it draws with `attrs`.
pub fn text_width(text: &str, attrs: &Attrs) -> f64 {
    // SAFETY: `attrs` has only string keys with the correct value types.
    unsafe { NSString::from_str(text).sizeWithAttributes(Some(attrs)) }.width
}

/// The height of one line of text in `font`.
pub fn line_height(font: &NSFont) -> f64 {
    (font.ascender() - font.descender() + font.leading()).ceil()
}

/// Builds the style with `appearance` as the drawing appearance. The system
/// colors with a changed alpha (for example the hidden names at 50%)
/// resolve when they are made, so they must resolve in the appearance of
/// the window, not in the appearance of the process.
pub fn style_for(config: &Config, appearance: &NSAppearance) -> (Style, Vec<String>) {
    let dark = is_dark(appearance);
    let out = std::cell::RefCell::new(None);
    let block = block2::StackBlock::new(|| {
        *out.borrow_mut() = Some(Style::new(config, dark));
    });
    appearance.performAsCurrentDrawingAppearance(&block);
    out.into_inner().unwrap_or_else(|| Style::new(config, dark))
}

/// All the colors that the UI draws.
#[derive(Clone)]
pub struct Palette {
    /// The list background. The scroll view draws it.
    pub list_bg: Retained<NSColor>,
    pub name: Retained<NSColor>,
    pub hidden: Retained<NSColor>,
    pub directory: Retained<NSColor>,
    pub symlink: Retained<NSColor>,
    pub executable: Retained<NSColor>,
    /// Size, kind, and date columns.
    pub secondary: Retained<NSColor>,
    /// The cursor row when the list has the focus and the window is key.
    pub selection_bg: Retained<NSColor>,
    pub selection_fg: Retained<NSColor>,
    /// The cursor row in other states.
    pub selection_bg_inactive: Retained<NSColor>,
    pub selection_fg_inactive: Retained<NSColor>,
    /// The column header, the bars, and the prompt.
    pub bar_bg: Retained<NSColor>,
    pub bar_fg: Retained<NSColor>,
    pub header_fg: Retained<NSColor>,
    pub header_sort_fg: Retained<NSColor>,
    pub accent: Retained<NSColor>,
    pub separator: Retained<NSColor>,
    /// The background of the help panel.
    pub panel_bg: Retained<NSColor>,
}

impl Palette {
    /// The macOS semantic colors, with the user color keys on top.
    fn system(config: &Config, opacity: f64) -> Palette {
        let o = &config.colors;
        let pick =
            |user: Option<Rgba>, system: Retained<NSColor>| user.map(color).unwrap_or(system);
        let list_bg = match o.background {
            Some(c) => color_alpha(c, opacity),
            None => NSColor::controlBackgroundColor().colorWithAlphaComponent(opacity),
        };
        let name = pick(o.foreground, NSColor::labelColor());
        let selection_bg = pick(
            o.selection_background,
            NSColor::selectedContentBackgroundColor(),
        );
        let selection_fg = pick(
            o.selection_foreground,
            NSColor::alternateSelectedControlTextColor(),
        );
        let secondary = pick(o.secondary, NSColor::secondaryLabelColor());
        Palette {
            hidden: o
                .hidden
                .map(color)
                .unwrap_or_else(|| NSColor::labelColor().colorWithAlphaComponent(0.5)),
            directory: pick(o.directory, name.clone()),
            symlink: pick(o.symlink, name.clone()),
            executable: pick(o.executable, name.clone()),
            selection_bg_inactive: o
                .selection_background
                .map(color)
                .unwrap_or_else(NSColor::unemphasizedSelectedContentBackgroundColor),
            selection_fg_inactive: pick(o.selection_foreground, NSColor::labelColor()),
            bar_bg: o
                .status_background
                .map(|c| color_alpha(c, opacity))
                .unwrap_or_else(|| list_bg.clone()),
            bar_fg: pick(o.status_foreground, NSColor::secondaryLabelColor()),
            header_fg: secondary.clone(),
            header_sort_fg: name.clone(),
            accent: pick(o.accent, NSColor::controlAccentColor()),
            separator: NSColor::separatorColor(),
            panel_bg: NSColor::windowBackgroundColor(),
            list_bg,
            name,
            secondary,
            selection_bg,
            selection_fg,
        }
    }

    /// The custom palette (the Ghostty look).
    fn custom(theme: &Theme, opacity: f64) -> Palette {
        let selection_bg = color(theme.selection_background);
        let selection_fg = color(theme.selection_foreground);
        Palette {
            list_bg: color_alpha(theme.background, opacity),
            name: color(theme.foreground),
            hidden: color(theme.hidden),
            directory: color(theme.directory),
            symlink: color(theme.symlink),
            executable: color(theme.executable),
            secondary: color(theme.secondary),
            selection_bg_inactive: selection_bg.clone(),
            selection_fg_inactive: selection_fg.clone(),
            selection_bg,
            selection_fg,
            bar_bg: color_alpha(theme.status_background, opacity),
            bar_fg: color(theme.status_foreground),
            header_fg: color(theme.secondary),
            header_sort_fg: color(theme.foreground),
            accent: color(theme.accent),
            separator: color_alpha(theme.secondary, 0.3),
            panel_bg: color(theme.status_background),
        }
    }
}

/// One column after the name column, with its width and alignment.
#[derive(Clone, Copy, Debug)]
pub struct ColumnSpec {
    pub column: Column,
    pub width: f64,
    pub right_aligned: bool,
}

/// The x positions of the columns for one list width.
#[derive(Clone, Debug)]
pub struct ColumnLayout {
    pub name_x: f64,
    /// The end of the name text.
    pub name_end: f64,
    /// The columns that fit, with their x positions.
    pub columns: Vec<(ColumnSpec, f64)>,
}

/// The three states of a row for the text colors.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    Normal,
    /// The cursor row, with the focus in the list and the window key.
    Selected,
    /// The cursor row in other states.
    SelectedInactive,
}

/// The text attributes for one row state.
#[derive(Clone)]
pub struct RowAttrs {
    pub name: Attrs,
    pub hidden: Attrs,
    pub directory: Attrs,
    pub symlink: Attrs,
    pub executable: Attrs,
    /// Column text: left aligned, right aligned, middle truncation.
    pub left: Attrs,
    pub right: Attrs,
    pub middle: Attrs,
    /// The hidden rule goes first (the system theme dims hidden folders too).
    pub hidden_first: bool,
}

/// All the derived look values: colors, fonts, metrics, attributes.
#[derive(Clone)]
pub struct Style {
    /// `theme = system`.
    pub system: bool,
    pub dark: bool,
    pub opacity: f64,
    pub font: Retained<NSFont>,
    pub row_height: f64,
    pub line_height: f64,
    pub icon_size: f64,
    pub show_icons: bool,
    pub columns: Vec<ColumnSpec>,
    pub date_format: String,
    pub colors: Palette,
    /// The font of the column header and of the bars.
    pub small_font: Retained<NSFont>,
    pub small_line_height: f64,
    pub header_height: f64,
    pub bar_height: f64,
    pub prompt_height: f64,

    pub normal: RowAttrs,
    pub selected: RowAttrs,
    pub selected_inactive: RowAttrs,
    pub empty: Attrs,
    pub header: Attrs,
    pub header_sort: Attrs,
    pub bar: Attrs,
    pub bar_accent: Attrs,
    pub help_title: Attrs,
    pub help_key: Attrs,
    pub help_action: Attrs,
    pub help_desc: Attrs,
}

impl Style {
    /// Builds the style. The second value lists problems for the status bar
    /// (for example a font that does not exist).
    pub fn new(config: &Config, system_is_dark: bool) -> (Style, Vec<String>) {
        let mut warnings = Vec::new();
        let theme = config.resolved_theme(system_is_dark);
        let system = config.theme == ThemeMode::System;
        let dark = match config.theme {
            ThemeMode::System | ThemeMode::Auto => system_is_dark,
            ThemeMode::Dark => true,
            ThemeMode::Light => false,
        };
        let opacity = config.background_opacity.clamp(0.0, 1.0);
        let colors = if system {
            Palette::system(config, opacity)
        } else {
            Palette::custom(&theme, opacity)
        };
        let size = config.font_size;
        let scale = size / 13.0;

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
        // SAFETY: The font weights are immutable statics from AppKit.
        let (regular, semibold) = unsafe { (NSFontWeightRegular, NSFontWeightSemibold) };
        let column_font = match &custom {
            Some(f) => f.clone(),
            None => NSFont::monospacedDigitSystemFontOfSize_weight(size, regular),
        };
        let small_size = (size - 2.0).max(9.0);
        let small_font = NSFont::monospacedDigitSystemFontOfSize_weight(small_size, regular);

        let line_h = line_height(&font);
        let row_height = config.effective_row_height().max(line_h + 2.0).round();
        let small_line_height = line_height(&small_font);
        let bar_height = BAR_HEIGHT.max(small_line_height + 8.0).round();
        let icon_size = (row_height - 4.0).clamp(12.0, 32.0).round();

        let middle_p = paragraph(NSLineBreakMode::ByTruncatingMiddle, NSTextAlignment::Left);
        let tail_p = paragraph(NSLineBreakMode::ByTruncatingTail, NSTextAlignment::Left);
        let right_p = paragraph(NSLineBreakMode::ByTruncatingTail, NSTextAlignment::Right);
        let center_p = paragraph(NSLineBreakMode::ByTruncatingTail, NSTextAlignment::Center);

        let c = &colors;
        let row_attrs = |state: RowState| {
            let pick = |normal: &NSColor| -> Retained<NSColor> {
                match state {
                    RowState::Normal => normal.retain(),
                    RowState::Selected => c.selection_fg.clone(),
                    RowState::SelectedInactive => c.selection_fg_inactive.clone(),
                }
            };
            let secondary = match state {
                RowState::Normal => c.secondary.clone(),
                RowState::Selected => c.selection_fg.colorWithAlphaComponent(0.85),
                RowState::SelectedInactive => c.secondary.clone(),
            };
            RowAttrs {
                name: attrs(&font, &pick(&c.name), &middle_p),
                hidden: attrs(&font, &pick(&c.hidden), &middle_p),
                directory: attrs(&font, &pick(&c.directory), &middle_p),
                symlink: attrs(&font, &pick(&c.symlink), &middle_p),
                executable: attrs(&font, &pick(&c.executable), &middle_p),
                left: attrs(&column_font, &secondary, &tail_p),
                right: attrs(&column_font, &secondary, &right_p),
                middle: attrs(&column_font, &secondary, &middle_p),
                hidden_first: system,
            }
        };

        let header_font = NSFont::systemFontOfSize_weight(small_size, regular);
        let header_sort_font = NSFont::systemFontOfSize_weight(small_size, semibold);
        let bold = NSFont::boldSystemFontOfSize(size + 1.0);

        let mut columns: Vec<ColumnSpec> = config
            .columns
            .iter()
            .map(|&column| {
                let (width, right_aligned) = match column {
                    Column::Size => (80.0, true),
                    Column::Kind => (130.0, false),
                    Column::Modified | Column::Created => (170.0, false),
                    Column::Extension => (70.0, false),
                };
                ColumnSpec {
                    column,
                    width: (width * scale).round(),
                    right_aligned,
                }
            })
            .collect();
        // A custom date format can be wider than 170 pt.
        if config.date_format != "finder" {
            let attrs = attrs(&column_font, &c.secondary, &tail_p);
            let sample = crate::columns::sample_date_text(&config.date_format);
            let needed = (text_width(&sample, &attrs) + 4.0).ceil();
            for spec in &mut columns {
                if matches!(spec.column, Column::Modified | Column::Created) {
                    spec.width = spec.width.max(needed).min(320.0);
                }
            }
        }

        let style = Style {
            system,
            dark,
            opacity,
            font: font.clone(),
            row_height,
            line_height: line_h,
            icon_size,
            show_icons: config.show_icons,
            columns,
            date_format: config.date_format.clone(),
            small_font: small_font.clone(),
            small_line_height,
            header_height: bar_height,
            bar_height,
            prompt_height: (line_h + 14.0).round(),
            normal: row_attrs(RowState::Normal),
            selected: row_attrs(RowState::Selected),
            selected_inactive: row_attrs(RowState::SelectedInactive),
            empty: attrs(&font, &c.secondary, &center_p),
            header: attrs(&header_font, &c.header_fg, &tail_p),
            header_sort: attrs(&header_sort_font, &c.header_sort_fg, &tail_p),
            bar: attrs(&small_font, &c.bar_fg, &tail_p),
            bar_accent: attrs(&small_font, &c.accent, &tail_p),
            help_title: attrs(&bold, &c.name, &tail_p),
            help_key: attrs(&column_font, &c.accent, &tail_p),
            help_action: attrs(&font, &c.name, &tail_p),
            help_desc: attrs(&font, &c.secondary, &tail_p),
            colors,
        };
        (style, warnings)
    }

    /// The text attributes for a row state.
    pub fn row(&self, state: RowState) -> &RowAttrs {
        match state {
            RowState::Normal => &self.normal,
            RowState::Selected => &self.selected,
            RowState::SelectedInactive => &self.selected_inactive,
        }
    }

    /// The x position where the name starts.
    pub fn name_x(&self) -> f64 {
        if self.show_icons {
            LEFT_PAD + self.icon_size + ICON_GAP
        } else {
            LEFT_PAD
        }
    }

    /// The column positions for a list of `width`. The name column gets all
    /// the width that the other columns do not use. When the name column
    /// gets too narrow, the columns on the left go first.
    pub fn layout(&self, width: f64) -> ColumnLayout {
        let name_x = self.name_x();
        let mut cols: &[ColumnSpec] = &self.columns;
        while !cols.is_empty() {
            let used: f64 = cols.iter().map(|c| c.width + COLUMN_GAP).sum();
            if width - RIGHT_PAD - used - name_x >= MIN_NAME_WIDTH {
                break;
            }
            cols = &cols[1..];
        }
        let mut right = width - RIGHT_PAD;
        let mut placed: Vec<(ColumnSpec, f64)> = cols
            .iter()
            .rev()
            .map(|spec| {
                let x = right - spec.width;
                right = x - COLUMN_GAP;
                (*spec, x)
            })
            .collect();
        placed.reverse();
        ColumnLayout {
            name_x,
            name_end: right.max(name_x),
            columns: placed,
        }
    }
}

impl RowAttrs {
    /// The name attributes for `entry`. The order of the rules: directory,
    /// symlink, executable, hidden, normal. With `hidden_first`, the hidden
    /// rule goes first. A package (`.app`) shows as a file, like in Finder.
    pub fn for_entry(&self, entry: &Entry) -> &Attrs {
        if self.hidden_first && entry.hidden {
            &self.hidden
        } else if entry.kind == EntryKind::Directory && !entry.is_package {
            &self.directory
        } else if entry.kind == EntryKind::Symlink {
            &self.symlink
        } else if entry.executable {
            &self.executable
        } else if entry.hidden {
            &self.hidden
        } else {
            &self.name
        }
    }
}
