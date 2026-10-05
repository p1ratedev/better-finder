//! File icons from `NSWorkspace`, with a cache.
//!
//! - Directories, packages, and symlinks: `iconForFile:`, cached by path.
//!   Special folders (Downloads, Applications) and apps have their own icons.
//! - Regular files: `iconForContentType:`, cached by lowercase extension.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use bf_core::{Entry, EntryKind};
use objc2::rc::Retained;
use objc2_app_kit::{NSImage, NSWorkspace};
use objc2_foundation::NSString;
use objc2_uniform_type_identifiers::{UTType, UTTypeData, UTTypeUnixExecutable};

#[derive(Default)]
struct IconCache {
    by_path: HashMap<PathBuf, Retained<NSImage>>,
    by_extension: HashMap<String, Retained<NSImage>>,
    executable: Option<Retained<NSImage>>,
    document: Option<Retained<NSImage>>,
}

thread_local! {
    static CACHE: RefCell<IconCache> = RefCell::new(IconCache::default());
}

/// The icon for `entry`. Main thread only.
pub fn icon_for(entry: &Entry) -> Option<Retained<NSImage>> {
    CACHE.with(|cache| cache.try_borrow_mut().ok().map(|mut c| c.get(entry)))
}

/// Clears the path cache. Call it when the directory changes.
pub fn clear_paths() {
    CACHE.with(|cache| {
        if let Ok(mut c) = cache.try_borrow_mut() {
            c.by_path.clear();
        }
    });
}

impl IconCache {
    fn get(&mut self, entry: &Entry) -> Retained<NSImage> {
        let workspace = NSWorkspace::sharedWorkspace();
        let by_path =
            matches!(entry.kind, EntryKind::Directory | EntryKind::Symlink) || entry.is_package;
        if by_path {
            if let Some(icon) = self.by_path.get(&entry.path) {
                return icon.clone();
            }
            let path = NSString::from_str(&entry.path.to_string_lossy());
            let icon = workspace.iconForFile(&path);
            self.by_path.insert(entry.path.clone(), icon.clone());
            return icon;
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
