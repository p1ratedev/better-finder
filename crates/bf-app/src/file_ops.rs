//! File system helpers with data-safety rules.
//!
//! - `rename_no_replace` never replaces an existing item.
//! - `is_local_volume` tells a local disk from a network volume.
//! - `volume_info` and `mounted_from` read the volume of a path (`statfs`).

use std::ffi::CString;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

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
