//! File icons from `NSWorkspace`, with a cache.
//!
//! The draw code uses only type icons (`iconForContentType:`). They need no
//! disk access:
//! - Directories: the folder icon. Symlinks: the symlink icon.
//! - Packages and files: the icon of the type of the extension.
//!
//! Path-specific icons (`iconForFile:`: special folders, app icons, custom
//! icons) can need disk or network access. So:
//! - The app asks for them only on a local volume (the loader decides).
//! - `drawRect:` only puts a path in a queue. After the frame, `resolve`
//!   gets a few icons, caches them by path, and redraws the list.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use bf_core::{Entry, EntryKind};
use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSImage, NSImageSymbolConfiguration, NSWorkspace};
use objc2_foundation::{NSArray, NSString};
use objc2_uniform_type_identifiers::{
    UTType, UTTypeData, UTTypeFolder, UTTypeSymbolicLink, UTTypeUnixExecutable, UTTypeVolume,
};

use crate::app;

/// The number of path icons that one `resolve` call gets.
const BATCH: usize = 24;

#[derive(Default)]
struct IconCache {
    by_path: HashMap<PathBuf, Retained<NSImage>>,
    by_extension: HashMap<String, Retained<NSImage>>,
    folder: Option<Retained<NSImage>>,
    volume: Option<Retained<NSImage>>,
    symlink: Option<Retained<NSImage>>,
    executable: Option<Retained<NSImage>>,
    document: Option<Retained<NSImage>>,
    /// Paths that wait for `resolve`.
    wanted: Vec<PathBuf>,
    /// Paths that are in `wanted` or that `resolve` got already.
    requested: HashSet<PathBuf>,
    resolve_scheduled: bool,
    /// Changes when the directory changes. `resolve` drops old results.
    generation: u64,
}

thread_local! {
    static CACHE: RefCell<IconCache> = RefCell::new(IconCache::default());
}

/// The icon for `entry`. Main thread only. It never accesses the disk.
/// With `path_icons`, an entry that can have its own icon goes into the
/// queue for `resolve`. Call `schedule_resolve` after the draw.
pub fn icon_for(entry: &Entry, path_icons: bool) -> Option<Retained<NSImage>> {
    CACHE.with(|cache| {
        cache
            .try_borrow_mut()
            .ok()
            .map(|mut c| c.get(entry, path_icons))
    })
}

/// The generic folder icon (a type icon: no disk access).
pub fn folder_icon() -> Option<Retained<NSImage>> {
    CACHE.with(|cache| {
        let mut c = cache.try_borrow_mut().ok()?;
        // SAFETY: `UTTypeFolder` is an immutable static.
        let kind = unsafe { UTTypeFolder };
        Some(
            c.folder
                .get_or_insert_with(|| NSWorkspace::sharedWorkspace().iconForContentType(kind))
                .clone(),
        )
    })
}

/// The generic volume icon (a type icon: no disk access).
pub fn volume_icon() -> Option<Retained<NSImage>> {
    CACHE.with(|cache| {
        let mut c = cache.try_borrow_mut().ok()?;
        // SAFETY: `UTTypeVolume` is an immutable static.
        let kind = unsafe { UTTypeVolume };
        Some(
            c.volume
                .get_or_insert_with(|| NSWorkspace::sharedWorkspace().iconForContentType(kind))
                .clone(),
        )
    })
}

/// Draws `image` into `rect` (flipped view).
pub fn draw_in(image: &NSImage, rect: objc2_foundation::NSRect) {
    // SAFETY: A `None` hints dictionary is valid. The views are flipped.
    unsafe {
        image.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
            rect,
            objc2_foundation::NSRect::ZERO,
            objc2_app_kit::NSCompositingOperation::SourceOver,
            1.0,
            true,
            None,
        );
    }
}

/// Clears the path cache. Call it when the directory changes.
pub fn clear_paths() {
    CACHE.with(|cache| {
        if let Ok(mut c) = cache.try_borrow_mut() {
            c.by_path.clear();
            c.wanted.clear();
            c.requested.clear();
            c.generation += 1;
        }
    });
}

/// Starts `resolve` on the main queue when paths wait for their icons.
pub fn schedule_resolve() {
    let schedule = CACHE.with(|cache| {
        let Ok(mut c) = cache.try_borrow_mut() else {
            return false;
        };
        if c.wanted.is_empty() || c.resolve_scheduled {
            return false;
        }
        c.resolve_scheduled = true;
        true
    });
    if schedule {
        app::after_now(resolve);
    }
}

/// Gets the path icons of one batch, then redraws the list.
fn resolve() {
    let batch = CACHE.with(|cache| {
        let Ok(mut c) = cache.try_borrow_mut() else {
            return None;
        };
        c.resolve_scheduled = false;
        let count = c.wanted.len().min(BATCH);
        let batch: Vec<PathBuf> = c.wanted.drain(..count).collect();
        Some((batch, c.generation))
    });
    let Some((batch, generation)) = batch else {
        return;
    };
    if batch.is_empty() {
        return;
    }
    let workspace = NSWorkspace::sharedWorkspace();
    let icons: Vec<(PathBuf, Retained<NSImage>)> = batch
        .into_iter()
        .map(|path| {
            let icon = workspace.iconForFile(&NSString::from_str(&path.to_string_lossy()));
            (path, icon)
        })
        .collect();
    CACHE.with(|cache| {
        if let Ok(mut c) = cache.try_borrow_mut() {
            if c.generation == generation {
                c.by_path.extend(icons);
            }
        }
    });
    app::redraw_list();
    schedule_resolve();
}

impl IconCache {
    fn get(&mut self, entry: &Entry, path_icons: bool) -> Retained<NSImage> {
        let own_icon =
            matches!(entry.kind, EntryKind::Directory | EntryKind::Symlink) || entry.is_package;
        if own_icon {
            if let Some(icon) = self.by_path.get(&entry.path) {
                return icon.clone();
            }
            if path_icons && self.requested.insert(entry.path.clone()) {
                self.wanted.push(entry.path.clone());
            }
        }
        let workspace = NSWorkspace::sharedWorkspace();
        if entry.kind == EntryKind::Symlink {
            // SAFETY: `UTTypeSymbolicLink` is an immutable static.
            let kind = unsafe { UTTypeSymbolicLink };
            return self
                .symlink
                .get_or_insert_with(|| workspace.iconForContentType(kind))
                .clone();
        }
        if entry.kind == EntryKind::Directory && !entry.is_package {
            // SAFETY: `UTTypeFolder` is an immutable static.
            let kind = unsafe { UTTypeFolder };
            return self
                .folder
                .get_or_insert_with(|| workspace.iconForContentType(kind))
                .clone();
        }
        let extension = entry.extension().to_lowercase();
        if extension.is_empty() {
            let (slot, kind) = if entry.executable {
                // SAFETY: `UTTypeUnixExecutable` is an immutable static.
                (&mut self.executable, unsafe { UTTypeUnixExecutable })
            } else {
                // SAFETY: `UTTypeData` is an immutable static.
                (&mut self.document, unsafe { UTTypeData })
            };
            return slot
                .get_or_insert_with(|| workspace.iconForContentType(kind))
                .clone();
        }
        self.by_extension
            .entry(extension)
            .or_insert_with_key(|ext| {
                let kind = UTType::typeWithFilenameExtension(&NSString::from_str(ext));
                match kind {
                    Some(kind) => workspace.iconForContentType(&kind),
                    // SAFETY: `UTTypeData` is an immutable static.
                    None => workspace.iconForContentType(unsafe { UTTypeData }),
                }
            })
            .clone()
    }
}

thread_local! {
    /// SF Symbol images by (name, point size key, color key).
    static SYMBOLS: RefCell<HashMap<(String, u32, usize), Retained<NSImage>>> =
        RefCell::new(HashMap::new());
}

/// An SF Symbol in one color, at `point_size`. `color` can be a dynamic
/// system color: it resolves when the image draws. The cache key uses the
/// address of the color object, so keep the color alive (the `Style` does).
pub fn symbol(name: &str, point_size: f64, color: &NSColor) -> Option<Retained<NSImage>> {
    let key = (
        name.to_string(),
        (point_size * 10.0) as u32,
        std::ptr::from_ref(color) as usize,
    );
    if let Some(image) = SYMBOLS.with(|s| s.borrow().get(&key).cloned()) {
        return Some(image);
    }
    let base = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    )?;
    // SAFETY: The weight is an immutable static from AppKit.
    let weight = unsafe { objc2_app_kit::NSFontWeightRegular };
    let size = NSImageSymbolConfiguration::configurationWithPointSize_weight(point_size, weight);
    let colors = NSArray::from_slice(&[color]);
    let palette = NSImageSymbolConfiguration::configurationWithPaletteColors(&colors);
    let config = size.configurationByApplyingConfiguration(&palette);
    let image = base.imageWithSymbolConfiguration(&config)?;
    SYMBOLS.with(|s| s.borrow_mut().insert(key, image.clone()));
    Some(image)
}

/// Draws `image` centered in `rect` (flipped view).
pub fn draw_centered(image: &NSImage, rect: objc2_foundation::NSRect) {
    let size = image.size();
    let x = rect.origin.x + ((rect.size.width - size.width) / 2.0).round();
    let y = rect.origin.y + ((rect.size.height - size.height) / 2.0).round();
    let target = objc2_foundation::NSRect::new(objc2_foundation::NSPoint::new(x, y), size);
    // SAFETY: A `None` hints dictionary is valid. The views are flipped.
    unsafe {
        image.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
            target,
            objc2_foundation::NSRect::ZERO,
            objc2_app_kit::NSCompositingOperation::SourceOver,
            1.0,
            true,
            None,
        );
    }
}
