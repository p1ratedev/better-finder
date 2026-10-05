//! Directory listing and path helpers.
//!
//! Speed rules:
//! - `read_dir` does one `lstat` for each entry, and one `stat` only for symlinks.
//! - It never follows symlinks to read sizes.
//! - The UI calls `read_dir` on a background thread, never on the main thread.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    Directory,
    File,
    Symlink,
    /// Sockets, FIFOs, devices.
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// File name. Invalid UTF-8 becomes U+FFFD (lossy).
    pub name: String,
    pub path: PathBuf,
    pub kind: EntryKind,
    /// True for a directory and for a symlink to a directory. False for a package.
    pub is_dir: bool,
    /// True for a macOS package: a directory that Finder shows as one file,
    /// for example `.app`. `Open` launches a package; it does not enter it.
    pub is_package: bool,
    /// `lstat` size in bytes. Zero for directories.
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// The name starts with `.`, or the `UF_HIDDEN` flag is set (like `~/Library`).
    pub hidden: bool,
    /// A regular file with any execute bit set.
    pub executable: bool,
}

impl Entry {
    /// The extension without the dot, or `""`. A leading dot does not start
    /// an extension: `.zshrc` has no extension.
    pub fn extension(&self) -> &str {
        todo!()
    }
}

/// Extensions of directories that macOS shows as one file. Compare without case.
pub const PACKAGE_EXTENSIONS: &[&str] = &[
    "app", "appex", "bundle", "framework", "kext", "pkg", "mpkg", "plugin", "prefpane",
    "qlgenerator", "mdimporter", "saver", "xpc", "xcodeproj", "xcworkspace", "playground",
    "rtfd", "pages", "numbers", "key", "photoslibrary", "musiclibrary", "tvlibrary",
    "logicx", "band", "fcpbundle", "imovielibrary", "scptd", "wdgt",
];

/// Lists all entries of `path`, hidden entries included. The order is not
/// defined; the caller sorts. When the metadata of one entry fails (for
/// example a broken symlink or no permission), the entry is still in the
/// result with `size = 0` and `modified = None`. Only a failure to open the
/// directory itself returns `Err`.
pub fn read_dir(path: &Path) -> io::Result<Vec<Entry>> {
    let _ = path;
    todo!()
}

/// The entry for one path, with the same rules as `read_dir`.
pub fn entry_for(path: &Path) -> io::Result<Entry> {
    let _ = path;
    todo!()
}

/// Expands user input to an absolute path:
/// - `~` and `~/x` use the home directory.
/// - A relative path joins to `cwd`.
/// - The result goes through `normalize`.
pub fn expand_path(input: &str, cwd: &Path) -> PathBuf {
    let _ = (input, cwd);
    todo!()
}

/// Lexical clean-up, no disk access: removes `.` components and resolves
/// `..` against the previous component. Never goes above `/`.
pub fn normalize(path: &Path) -> PathBuf {
    let _ = path;
    todo!()
}

/// `~/x` form of `path` when it is inside the home directory, for titles and
/// the status bar.
pub fn display_path(path: &Path) -> String {
    let _ = path;
    todo!()
}

/// The home directory. Falls back to `/` when it is not known.
pub fn home_dir() -> PathBuf {
    todo!()
}
