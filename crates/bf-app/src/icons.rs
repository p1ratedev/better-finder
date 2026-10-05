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
use objc2_app_kit::{NSImage, NSWorkspace};
use objc2_foundation::NSString;
use objc2_uniform_type_identifiers::{
    UTType, UTTypeData, UTTypeFolder, UTTypeSymbolicLink, UTTypeUnixExecutable,
};

use crate::app;

/// The number of path icons that one `resolve` call gets.
const BATCH: usize = 24;

#[derive(Default)]
struct IconCache {
    by_path: HashMap<PathBuf, Retained<NSImage>>,
    by_extension: HashMap<String, Retained<NSImage>>,
    folder: Option<Retained<NSImage>>,
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
