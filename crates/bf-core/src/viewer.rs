//! The open requests that other apps send to the file viewer.
//!
//! When Better Finder is the file viewer of the Mac, two kinds of request
//! come to it as an open request (the Apple Event `aevt`/`odoc`):
//!
//! - An app opens a folder in Better Finder, for example
//!   `open -a "Better Finder" ~/Downloads`. Launch Services sends it.
//! - An app shows an item in its folder, for example "Show in Finder" or
//!   `open -R <file>`. `NSWorkspace` sends it to the app in the
//!   `NSFileViewer` default.
//!
//! The event is the same for the two kinds. `target` converts its paths to
//! one folder and the name to put the cursor on: a folder opens, and a
//! file opens its folder with the cursor on the file. The app has one
//! window and one cursor, so the first usable path decides. Pure
//! functions, except `Kind::of`.

use std::path::{Path, PathBuf};

/// What is at a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Folder,
    /// A file, or any other item that is not a folder.
    File,
    /// Nothing is at the path, or the app cannot read it.
    Missing,
}

impl Kind {
    /// Reads the kind of `path` from the disk. It follows symlinks, so a
    /// symlink to a folder is a folder. A read on a network volume can be
    /// slow: call it on a background thread.
    pub fn of(path: &Path) -> Kind {
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_dir() => Kind::Folder,
            Ok(_) => Kind::File,
            Err(_) => Kind::Missing,
        }
    }
}

/// The folder to show and the entry to put the cursor on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub folder: PathBuf,
    /// The name of the entry in `folder`. `None` keeps the normal cursor
    /// rule of the folder.
    pub select: Option<String>,
}

/// The target of a request with `items` (each path with its kind). The
/// first item that gives a target wins. A missing item and a relative path
/// give no target. `None` when no item gives a target.
pub fn target(items: &[(PathBuf, Kind)]) -> Option<Target> {
    items
        .iter()
        .find_map(|(path, kind)| item_target(path, *kind))
}

fn item_target(path: &Path, kind: Kind) -> Option<Target> {
    if !path.is_absolute() {
        return None;
    }
    match kind {
        Kind::Missing => None,
        // The components drop a trailing slash.
        Kind::Folder => Some(Target {
            folder: path.components().collect(),
            select: None,
        }),
        Kind::File => {
            let (parent, name) = (path.parent()?, path.file_name()?);
            Some(Target {
                folder: parent.to_path_buf(),
                select: Some(name.to_string_lossy().into_owned()),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str, kind: Kind) -> (PathBuf, Kind) {
        (PathBuf::from(path), kind)
    }

    fn at(folder: &str, select: Option<&str>) -> Option<Target> {
        Some(Target {
            folder: PathBuf::from(folder),
            select: select.map(str::to_string),
        })
    }

    #[test]
    fn a_folder_opens() {
        let items = [item("/Users/me/Downloads", Kind::Folder)];
        assert_eq!(target(&items), at("/Users/me/Downloads", None));
    }

    #[test]
    fn a_file_opens_its_folder_with_the_cursor_on_it() {
        let items = [item("/Users/me/notes.txt", Kind::File)];
        assert_eq!(target(&items), at("/Users/me", Some("notes.txt")));
    }

    #[test]
    fn a_trailing_slash_goes_away() {
        let items = [item("/Users/me/Downloads/", Kind::Folder)];
        let target = target(&items).unwrap();
        assert_eq!(target.folder.as_os_str(), "/Users/me/Downloads");
        assert_eq!(target.select, None);
    }

    #[test]
    fn root_opens() {
        let items = [item("/", Kind::Folder)];
        assert_eq!(target(&items), at("/", None));
    }

    #[test]
    fn the_first_item_wins() {
        let items = [
            item("/a/one.txt", Kind::File),
            item("/b/two.txt", Kind::File),
        ];
        assert_eq!(target(&items), at("/a", Some("one.txt")));
    }

    #[test]
    fn missing_and_relative_items_are_skipped() {
        let items = [
            item("/gone/old.txt", Kind::Missing),
            item("relative.txt", Kind::File),
            item("/b", Kind::Folder),
        ];
        assert_eq!(target(&items), at("/b", None));
    }

    #[test]
    fn no_usable_item_gives_no_target() {
        assert_eq!(target(&[]), None);
        assert_eq!(target(&[item("/gone", Kind::Missing)]), None);
    }

    #[test]
    fn kind_reads_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file.txt");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(Kind::of(dir.path()), Kind::Folder);
        assert_eq!(Kind::of(&file), Kind::File);
        assert_eq!(Kind::of(&dir.path().join("missing")), Kind::Missing);
    }

    #[test]
    fn kind_follows_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(dir.path(), &link).unwrap();
        assert_eq!(Kind::of(&link), Kind::Folder);
        let broken = dir.path().join("broken");
        std::os::unix::fs::symlink(dir.path().join("none"), &broken).unwrap();
        assert_eq!(Kind::of(&broken), Kind::Missing);
    }
}
