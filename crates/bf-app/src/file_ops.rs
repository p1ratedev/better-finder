//! File system helpers with data-safety rules.
//!
//! - `rename_no_replace` never replaces an existing item.
//! - `transfer` copies, moves, or makes aliases of items in a folder, on a
//!   background thread. It never replaces an item and never deletes a source.
//! - `compress` makes a zip archive with `ditto`, like Finder.
//! - `is_local_volume` tells a local disk from a network volume.
//! - `volume_info` and `mounted_from` read the volume of a path (`statfs`).

use std::ffi::{CString, OsString};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use bf_core::names::{self, NameStyle};
use bf_core::transfer::{self, Done, ItemResult};
use objc2::rc::autoreleasepool;
use objc2_foundation::{NSError, NSFileManager, NSURL, NSURLBookmarkCreationOptions};

use crate::app;

pub use bf_core::transfer::TransferMode;

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the path contains a NUL byte"))
}

/// Renames `from` to `to`. It never replaces an existing item.
///
/// - `to` is the same file as `from` (a change of case on a volume that
///   ignores case): a plain rename changes the case.
/// - `to` is a different item: the result is an `AlreadyExists` error. On a
///   case-sensitive volume, `readme` to `README` stops here when `README`
///   exists.
/// - `to` does not exist: `renamex_np` with `RENAME_EXCL` does the check and
///   the rename in one atomic step.
pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    if let Ok(target) = std::fs::symlink_metadata(to) {
        let source = std::fs::symlink_metadata(from)?;
        if (source.dev(), source.ino()) == (target.dev(), target.ino()) {
            return std::fs::rename(from, to);
        }
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    rename_exclusive(from, to)
}

/// Renames `from` to `to` when `to` does not exist. The check and the
/// rename are one atomic step where the file system supports it.
fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    let (c_from, c_to) = (c_path(from)?, c_path(to)?);
    // SAFETY: Both arguments are valid NUL-terminated C strings.
    let status = unsafe { libc::renamex_np(c_from.as_ptr(), c_to.as_ptr(), libc::RENAME_EXCL) };
    if status == 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    if err.raw_os_error() != Some(libc::ENOTSUP) {
        return Err(err);
    }
    // Some file systems (for example SMB) do not support `RENAME_EXCL`. Then
    // only a separate check is possible.
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    std::fs::rename(from, to)
}

/// Creates the file `path` with `contents`, and its parent directories.
/// Returns false and changes nothing when the file exists: `create_new` does
/// the check and the create in one atomic step, so it never truncates.
pub fn create_new_file(path: &Path, contents: &[u8]) -> io::Result<bool> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path);
    match file {
        Ok(mut file) => {
            file.write_all(contents)?;
            Ok(true)
        }
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(err) => Err(err),
    }
}

/// The result of `transfer`, for `app::transfer_finished`.
pub struct TransferReport {
    pub dest_dir: PathBuf,
    /// One result for each source, in order.
    pub results: Vec<ItemResult>,
    /// The folders with a change: the destination, and the folders that a
    /// move took items from.
    pub changed: Vec<PathBuf>,
}

/// Copies, moves, or makes aliases of `sources` in `dest_dir`. The work
/// runs on a background thread. Then, on the main thread,
/// `app::transfer_finished` reads the changed folder again, puts the cursor
/// on the first new item when the list shows it, and shows one message.
///
/// The rules (see `bf_core::transfer`):
/// - It never replaces an item. A name that exists gives the Keep Both
///   name (`file 2.txt`). A copy into the folder of the source gets the
///   duplicate name (`file copy.txt`).
/// - `Move` renames the item only on the same volume. On another volume it
///   copies, and the source stays. A move into the folder of the source
///   does nothing.
/// - A folder never goes into itself or into one of its subfolders.
/// - `Alias` makes a Finder alias file (`file.txt alias`) in `dest_dir`.
pub fn transfer(sources: Vec<PathBuf>, dest_dir: PathBuf, mode: TransferMode) {
    if sources.is_empty() {
        return;
    }
    let names: Vec<String> = sources.iter().map(|p| lossy_name(p)).collect();
    let progress = transfer::progress_text(mode, &names);
    // The caller can hold the app state: show the message after this event.
    app::after_now(move || app::show_progress(progress));
    let spawned = app::in_background(
        "bf-transfer",
        move || run_transfer(&sources, dest_dir, mode),
        app::transfer_finished,
    );
    if let Err(err) = spawned {
        app::after_now(move || app::show_error(format!("Cannot start a thread: {err}")));
    }
}

/// The file name of `path` as text, for names and messages.
fn lossy_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The work of `transfer`. It accesses the disk: background thread only.
fn run_transfer(sources: &[PathBuf], dest_dir: PathBuf, mode: TransferMode) -> TransferReport {
    let dest = std::fs::canonicalize(&dest_dir)
        .and_then(|real| std::fs::metadata(&real).map(|meta| (real, meta.dev())));
    let mut changed = vec![dest_dir.clone()];
    let results = sources
        .iter()
        .map(|source| {
            let name = lossy_name(source);
            let result = match &dest {
                Ok((real, dev)) => transfer_one(source, &name, &dest_dir, real, *dev, mode),
                Err(err) => Err(err.to_string()),
            };
            match result {
                Ok(Some((done, new))) => {
                    if done == Done::Moved {
                        if let Some(parent) = source.parent() {
                            if !changed.iter().any(|p| p == parent) {
                                changed.push(parent.to_path_buf());
                            }
                        }
                    }
                    ItemResult::Done {
                        done,
                        source: name,
                        new,
                    }
                }
                Ok(None) => ItemResult::Skipped { name },
                Err(reason) => {
                    ItemResult::Failed(format!("Cannot {} \"{name}\": {reason}", mode.verb()))
                }
            }
        })
        .collect();
    TransferReport {
        dest_dir,
        results,
        changed,
    }
}

/// Transfers one item. `dest_real` is the canonical path of `dest_dir`, and
/// `dest_dev` its device. The result is the work and the new name, `None`
/// for a move into the same folder, or the reason of a failure.
fn transfer_one(
    source: &Path,
    name: &str,
    dest_dir: &Path,
    dest_real: &Path,
    dest_dev: u64,
    mode: TransferMode,
) -> Result<Option<(Done, String)>, String> {
    let meta = std::fs::symlink_metadata(source).map_err(|e| e.to_string())?;
    let parent_real = source.parent().and_then(|p| std::fs::canonicalize(p).ok());
    let same_folder =
        source.parent() == Some(dest_dir) || parent_real.as_deref() == Some(dest_real);
    if mode == TransferMode::Move && same_folder {
        return Ok(None);
    }
    // A symlink copies as a link, so only a real folder can go into itself.
    if meta.is_dir() && mode != TransferMode::Alias {
        let source_real = std::fs::canonicalize(source).map_err(|e| e.to_string())?;
        if transfer::is_within(dest_real, &source_real) {
            return Err("the destination is inside the folder".to_string());
        }
    }
    let entry = bf_core::fs::entry_for(source).map_err(|e| e.to_string())?;
    let folder = entry.kind == bf_core::EntryKind::Directory && !entry.is_package;
    let style = mode.name_style(same_folder);
    let result = match mode {
        TransferMode::Alias => make_alias_in(source, name, dest_dir).map(|n| (Done::Aliased, n)),
        TransferMode::Move if meta.dev() == dest_dev => {
            match first_free_op(dest_dir, style, name, folder, |to| {
                rename_exclusive(source, to)
            }) {
                Ok(new) => Ok((Done::Moved, new)),
                // Another volume under the same device number: copy.
                Err(err) if err.raw_os_error() == Some(libc::EXDEV) => {
                    copy_free(source, name, dest_dir, style, folder).map(|n| (Done::Copied, n))
                }
                Err(err) => Err(err),
            }
        }
        TransferMode::Move | TransferMode::Copy => {
            copy_free(source, name, dest_dir, style, folder).map(|n| (Done::Copied, n))
        }
    };
    result.map(Some).map_err(|e| e.to_string())
}

/// Tries the names of `style` in `dir` in order, with `op`, until one
/// works. `op` must fail with `AlreadyExists` when its target exists, so
/// nothing is replaced. Returns the name that worked.
fn first_free_op(
    dir: &Path,
    style: NameStyle,
    name: &str,
    folder: bool,
    mut op: impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<String> {
    for candidate in names::candidates(style, name, folder) {
        let target = dir.join(&candidate);
        if std::fs::symlink_metadata(&target).is_ok() {
            continue;
        }
        match op(&target) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::other("no free name"))
}

/// Copies `source` into `dir` with the first free name of `style`.
/// `NSFileManager` clones the file on APFS, keeps the metadata, copies a
/// folder with its content, copies a symlink as a link, and refuses a
/// target that exists.
fn copy_free(
    source: &Path,
    name: &str,
    dir: &Path,
    style: NameStyle,
    folder: bool,
) -> io::Result<String> {
    first_free_op(dir, style, name, folder, |target| copy_item(source, target))
}

/// Copies one item with `NSFileManager`. The default manager is thread-safe.
fn copy_item(source: &Path, target: &Path) -> io::Result<()> {
    autoreleasepool(|_| {
        let (Some(from), Some(to)) = (NSURL::from_file_path(source), NSURL::from_file_path(target))
        else {
            return Err(invalid_path());
        };
        NSFileManager::defaultManager()
            .copyItemAtURL_toURL_error(&from, &to)
            .map_err(|err| io_error(&err))
    })
}

fn invalid_path() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "the path is not valid")
}

/// An `io::Error` for an `NSError`. "The file exists" becomes `AlreadyExists`.
fn io_error(err: &NSError) -> io::Error {
    let domain = err.domain().to_string();
    let code = err.code();
    // `NSFileWriteFileExistsError` and `EEXIST`.
    let exists = (domain == "NSCocoaErrorDomain" && code == 516)
        || (domain == "NSPOSIXErrorDomain" && code == libc::EEXIST as isize);
    if exists {
        return io::ErrorKind::AlreadyExists.into();
    }
    io::Error::other(err.localizedDescription().to_string())
}

/// Puts a new file with the first free name of `style` in `dir`. It first
/// makes an empty file with `create_new`, so the name is ours, then `write`
/// fills it. When `write` fails, the file goes away again.
fn new_file_free(
    dir: &Path,
    style: NameStyle,
    name: &str,
    mut write: impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<String> {
    let new = first_free_op(dir, style, name, false, |target| {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)
            .map(drop)
    })?;
    let target = dir.join(&new);
    if let Err(err) = write(&target) {
        // This call made the file just now: no data of the user is in it.
        let _ = std::fs::remove_file(&target);
        return Err(err);
    }
    Ok(new)
}

/// Makes a Finder alias file of `source` in `dir`: `report.pdf alias`, then
/// `report.pdf alias 2`. The bookmark data comes from `NSURL`.
fn make_alias_in(source: &Path, name: &str, dir: &Path) -> io::Result<String> {
    autoreleasepool(|_| {
        let url = NSURL::from_file_path(source).ok_or_else(invalid_path)?;
        let data = url
            .bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
                NSURLBookmarkCreationOptions::SuitableForBookmarkFile,
                None,
                None,
            )
            .map_err(|err| io_error(&err))?;
        new_file_free(dir, NameStyle::Alias, name, |target| {
            let to = NSURL::from_file_path(target).ok_or_else(invalid_path)?;
            NSURL::writeBookmarkData_toURL_options_error(&data, &to, 0)
                .map_err(|err| io_error(&err))
        })
    })
}

/// Compresses `source` into a zip archive next to it, like Finder:
/// `report.pdf.zip`, then `report.pdf 2.zip`. It waits for `ditto`: call it
/// on a background thread. Returns the name of the archive.
pub fn compress(source: &Path) -> io::Result<String> {
    let dir = source
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the item has no folder"))?;
    let name = lossy_name(source);
    // A folder keeps its name in the archive (`--keepParent`). A file needs
    // no parent: else the archive has the name of its folder too.
    let is_dir = std::fs::symlink_metadata(source)?.is_dir();
    new_file_free(dir, NameStyle::Zip, &name, |target| {
        let mut command = Command::new("/usr/bin/ditto");
        command.args(["-c", "-k", "--sequesterRsrc"]);
        if is_dir {
            command.arg("--keepParent");
        }
        let output = command
            .arg(source)
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.lines().next().unwrap_or("").trim();
        Err(io::Error::other(if detail.is_empty() {
            format!("ditto failed: {}", output.status)
        } else {
            detail.to_string()
        }))
    })
}

/// The `osascript` arguments that open the Finder Info window of `path`.
/// The path is an argument of the script, not a part of its text, so it
/// needs no quotes.
pub fn info_window_args(path: &Path) -> Vec<OsString> {
    let mut args = Vec::new();
    for line in [
        "on run argv",
        "tell application \"Finder\"",
        "activate",
        "open information window of (POSIX file (item 1 of argv) as alias)",
        "end tell",
        "end run",
    ] {
        args.push(OsString::from("-e"));
        args.push(OsString::from(line));
    }
    args.push(path.as_os_str().to_os_string());
    args
}

/// True when `path` is on a local volume (`MNT_LOCAL`). False for a
/// network volume and when `statfs` fails. Call it only on a background
/// thread: `statfs` on a network volume can wait for the network.
pub fn is_local_volume(path: &Path) -> bool {
    let Ok(c) = c_path(path) else {
        return false;
    };
    // SAFETY: `statfs` is plain data; zero is a valid bit pattern for it.
    let mut info: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid C string and `info` is a valid out pointer.
    let status = unsafe { libc::statfs(c.as_ptr(), &mut info) };
    status == 0 && info.f_flags & (libc::MNT_LOCAL as u32) != 0
}

/// The volume of a directory, for the path bar.
#[derive(Clone, Debug, PartialEq)]
pub struct VolumeInfo {
    /// The root of the volume, as the path bar shows it (`/` for the boot
    /// volume, also when the data volume holds the folder).
    pub mount: PathBuf,
    /// The localized volume name, for example `Macintosh HD`.
    pub name: String,
    /// The free bytes for a normal user (`f_bavail`).
    pub free: u64,
}

/// The volume of `path`. It accesses the disk: call it on a background
/// thread.
pub fn volume_info(path: &Path) -> Option<VolumeInfo> {
    let c = c_path(path).ok()?;
    // SAFETY: `statfs` is plain data; zero is a valid bit pattern for it.
    let mut info: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid C string and `info` is a valid out pointer.
    if unsafe { libc::statfs(c.as_ptr(), &mut info) } != 0 {
        return None;
    }
    let free = info.f_bavail.saturating_mul(u64::from(info.f_bsize));
    // SAFETY: `f_mntonname` is a NUL-terminated C string inside `info`.
    let mount = unsafe { std::ffi::CStr::from_ptr(info.f_mntonname.as_ptr()) };
    let mut mount = PathBuf::from(std::ffi::OsStr::from_bytes(mount.to_bytes()));
    // The boot volume group: the data volume is the same tree as `/`.
    if mount == Path::new("/System/Volumes/Data") {
        mount = PathBuf::from("/");
    }
    let name = volume_name(&mount).unwrap_or_else(|| {
        mount
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".to_string())
    });
    Some(VolumeInfo { mount, name, free })
}

/// The source of the volume of `path` (`f_mntfromname`), for example
/// `/dev/disk5s1`, or `//user@server/share` for a network volume. It
/// accesses the disk: call it on a background thread.
pub fn mounted_from(path: &Path) -> Option<String> {
    let c = c_path(path).ok()?;
    // SAFETY: `statfs` is plain data; zero is a valid bit pattern for it.
    let mut info: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid C string and `info` is a valid out pointer.
    if unsafe { libc::statfs(c.as_ptr(), &mut info) } != 0 {
        return None;
    }
    // SAFETY: `f_mntfromname` is a NUL-terminated C string inside `info`.
    let from = unsafe { std::ffi::CStr::from_ptr(info.f_mntfromname.as_ptr()) };
    Some(from.to_string_lossy().into_owned())
}

fn volume_name(mount: &Path) -> Option<String> {
    use objc2_foundation::{NSArray, NSString, NSURL, NSURLVolumeLocalizedNameKey};
    let url = NSURL::from_directory_path(mount)?;
    // SAFETY: The resource key is an immutable static from Foundation.
    let key = unsafe { NSURLVolumeLocalizedNameKey };
    let keys = NSArray::from_slice(&[key]);
    let values = url.resourceValuesForKeys_error(&keys).ok()?;
    let name = values.objectForKey(key)?.downcast::<NSString>().ok()?;
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("bf-file-ops-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn rename_refuses_an_existing_target() {
        let dir = temp_dir("exists");
        std::fs::write(dir.join("a"), "a").expect("write a");
        std::fs::write(dir.join("b"), "b").expect("write b");
        let err = rename_no_replace(&dir.join("a"), &dir.join("b")).expect_err("must refuse");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(dir.join("b")).expect("read b"), "b");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_to_a_new_name_and_change_of_case() {
        let dir = temp_dir("new");
        std::fs::write(dir.join("readme"), "x").expect("write");
        rename_no_replace(&dir.join("readme"), &dir.join("notes")).expect("rename");
        assert!(dir.join("notes").exists());
        // On a volume that ignores case, `NOTES` is the same file: the
        // rename must change the case. On a case-sensitive volume, `NOTES`
        // does not exist and the rename is a normal one.
        rename_no_replace(&dir.join("notes"), &dir.join("NOTES")).expect("change case");
        let names: Vec<_> = std::fs::read_dir(&dir)
            .expect("read dir")
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("NOTES")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_new_file_never_truncates() {
        let dir = temp_dir("create");
        let path = dir.join("sub").join("config");
        assert!(create_new_file(&path, b"template").expect("create"));
        std::fs::write(&path, "user text").expect("write");
        assert!(!create_new_file(&path, b"template").expect("exists"));
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "user text");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn write(path: &Path, text: &str) {
        std::fs::write(path, text).expect("write");
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).expect("read")
    }

    fn new_name(result: &ItemResult) -> Option<&str> {
        match result {
            ItemResult::Done { new, .. } => Some(new),
            _ => None,
        }
    }

    #[test]
    fn copies_never_replace() {
        let dir = temp_dir("copy");
        let other = dir.join("other");
        std::fs::create_dir(&other).expect("mkdir");
        write(&dir.join("a.txt"), "source");
        write(&other.join("a.txt"), "keep");
        // Keep Both in another folder.
        let report = run_transfer(&[dir.join("a.txt")], other.clone(), TransferMode::Copy);
        assert_eq!(
            report.results,
            [ItemResult::Done {
                done: Done::Copied,
                source: "a.txt".into(),
                new: "a 2.txt".into()
            }]
        );
        assert_eq!(read(&other.join("a.txt")), "keep");
        assert_eq!(read(&other.join("a 2.txt")), "source");
        // The duplicate names in the same folder.
        for expected in ["a copy.txt", "a copy 2.txt"] {
            let report = run_transfer(&[dir.join("a.txt")], dir.clone(), TransferMode::Copy);
            assert_eq!(new_name(&report.results[0]), Some(expected));
        }
        assert_eq!(read(&dir.join("a.txt")), "source");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folders_copy_with_their_content_but_not_into_themselves() {
        let dir = temp_dir("folder");
        let folder = dir.join("Folder");
        std::fs::create_dir_all(folder.join("sub")).expect("mkdir");
        write(&folder.join("sub/f.txt"), "x");
        let report = run_transfer(
            std::slice::from_ref(&folder),
            dir.clone(),
            TransferMode::Copy,
        );
        assert_eq!(new_name(&report.results[0]), Some("Folder copy"));
        assert_eq!(read(&dir.join("Folder copy/sub/f.txt")), "x");
        for mode in [TransferMode::Copy, TransferMode::Move] {
            let report = run_transfer(std::slice::from_ref(&folder), folder.join("sub"), mode);
            assert!(
                matches!(&report.results[0], ItemResult::Failed(_)),
                "{mode:?}"
            );
        }
        assert!(folder.join("sub/f.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn moves_rename_and_skip_the_same_folder() {
        let dir = temp_dir("move");
        let other = dir.join("other");
        std::fs::create_dir(&other).expect("mkdir");
        write(&dir.join("a.txt"), "moved");
        write(&other.join("a.txt"), "keep");
        let report = run_transfer(&[other.join("a.txt")], other.clone(), TransferMode::Move);
        assert_eq!(
            report.results,
            [ItemResult::Skipped {
                name: "a.txt".into()
            }]
        );
        let report = run_transfer(&[dir.join("a.txt")], other.clone(), TransferMode::Move);
        assert!(matches!(
            &report.results[0],
            ItemResult::Done { done: Done::Moved, new, .. } if new == "a 2.txt"
        ));
        assert!(!dir.join("a.txt").exists());
        assert_eq!(read(&other.join("a.txt")), "keep");
        assert_eq!(read(&other.join("a 2.txt")), "moved");
        assert!(report.changed.contains(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn aliases_and_archives_get_free_names() {
        let dir = temp_dir("alias");
        write(&dir.join("a.txt"), "x");
        write(&dir.join("a.txt alias"), "keep");
        write(&dir.join("a.txt.zip"), "keep");
        let report = run_transfer(&[dir.join("a.txt")], dir.clone(), TransferMode::Alias);
        assert_eq!(new_name(&report.results[0]), Some("a.txt alias 2"));
        let alias = std::fs::metadata(dir.join("a.txt alias 2")).expect("alias");
        assert!(alias.len() > 0);
        assert_eq!(compress(&dir.join("a.txt")).expect("zip"), "a.txt 2.zip");
        assert!(
            std::fs::metadata(dir.join("a.txt 2.zip"))
                .expect("zip")
                .len()
                > 0
        );
        assert_eq!(read(&dir.join("a.txt alias")), "keep");
        assert_eq!(read(&dir.join("a.txt.zip")), "keep");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn info_window_path_is_an_argument() {
        let path = "/tmp/a \"b\".txt";
        let args = info_window_args(Path::new(path));
        assert_eq!(
            args.last().map(|a| a.to_string_lossy().into_owned()),
            Some(path.into())
        );
        assert_eq!(args.iter().filter(|a| *a == "-e").count(), 6);
        assert!(
            !args[..args.len() - 1]
                .iter()
                .any(|a| a.to_string_lossy().contains(path))
        );
    }

    #[test]
    fn volume_of_the_home_folder() {
        let info = volume_info(&std::env::temp_dir()).expect("volume info");
        assert_eq!(info.mount, PathBuf::from("/"));
        assert!(!info.name.is_empty());
        assert!(info.free > 0);
    }

    #[test]
    fn the_boot_volume_comes_from_a_disk() {
        let from = mounted_from(&std::env::temp_dir()).expect("statfs");
        assert!(from.starts_with("/dev/disk"), "{from}");
        assert!(mounted_from(Path::new("/no/such/path/for/bf")).is_none());
    }

    #[test]
    fn local_volume() {
        assert!(is_local_volume(&std::env::temp_dir()));
        assert!(!is_local_volume(Path::new("/no/such/path/for/bf")));
    }
}
