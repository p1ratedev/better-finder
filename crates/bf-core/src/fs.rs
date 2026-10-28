//! Directory listing and path helpers.
//!
//! Speed rules:
//! - `read_dir` does one `lstat` for each entry, and one `stat` only for symlinks.
//! - It never follows symlinks to read sizes.
//! - The UI calls `read_dir` on a background thread, never on the main thread.

use std::ffi::OsStr;
use std::fs::Metadata;
use std::io::{self, Read as _};
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

/// The `UF_HIDDEN` file flag. Finder hides files with this flag.
const UF_HIDDEN: u32 = 0x8000;

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
    /// The birth time from the same `lstat` (no extra syscall).
    pub created: Option<SystemTime>,
    /// The name starts with `.`, or the `UF_HIDDEN` flag is set (like `~/Library`).
    pub hidden: bool,
    /// A regular file with any execute bit set.
    pub executable: bool,
}

impl Entry {
    /// The extension without the dot, or `""`. A leading dot does not start
    /// an extension: `.zshrc` has no extension.
    pub fn extension(&self) -> &str {
        match self.name.rfind('.') {
            Some(index) if index > 0 => &self.name[index + 1..],
            _ => "",
        }
    }
}

/// Extensions of directories that macOS shows as one file. Compare without case.
pub const PACKAGE_EXTENSIONS: &[&str] = &[
    "app",
    "appex",
    "bundle",
    "framework",
    "kext",
    "pkg",
    "mpkg",
    "plugin",
    "prefpane",
    "qlgenerator",
    "mdimporter",
    "saver",
    "xpc",
    "xcodeproj",
    "xcworkspace",
    "playground",
    "rtfd",
    "pages",
    "numbers",
    "key",
    "photoslibrary",
    "musiclibrary",
    "tvlibrary",
    "logicx",
    "band",
    "fcpbundle",
    "imovielibrary",
    "scptd",
    "wdgt",
];

/// Lists all entries of `path`, hidden entries included. The order is not
/// defined; the caller sorts. When the metadata of one entry fails (for
/// example a broken symlink or no permission), the entry is still in the
/// result with `size = 0` and `modified = None`. Only a failure to open the
/// directory itself returns `Err`.
pub fn read_dir(path: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in std::fs::read_dir(path)? {
        // An error on one item (for example a race with a delete) skips the item.
        let Ok(item) = item else { continue };
        let metadata = item.metadata();
        let fallback = item.file_type().ok();
        entries.push(build_entry(
            &item.file_name(),
            item.path(),
            metadata,
            fallback,
        ));
    }
    Ok(entries)
}

/// The entry for one path, with the same rules as `read_dir`.
pub fn entry_for(path: &Path) -> io::Result<Entry> {
    let metadata = std::fs::symlink_metadata(path)?;
    let name = match path.file_name() {
        Some(name) => name.to_os_string(),
        None => path.as_os_str().to_os_string(),
    };
    Ok(build_entry(&name, path.to_path_buf(), Ok(metadata), None))
}

/// Makes an `Entry` from the `lstat` result. `fallback` gives the kind when
/// the metadata is not available.
fn build_entry(
    name: &OsStr,
    path: PathBuf,
    metadata: io::Result<Metadata>,
    fallback: Option<std::fs::FileType>,
) -> Entry {
    let name = name.to_string_lossy().into_owned();
    let mut hidden = name.starts_with('.');
    let (kind, size, modified, created, executable) = match &metadata {
        Ok(md) => {
            hidden |= md.st_flags() & UF_HIDDEN != 0;
            let file_type = md.file_type();
            let kind = kind_of(file_type);
            let executable = file_type.is_file() && md.permissions().mode() & 0o111 != 0;
            let size = if kind == EntryKind::Directory {
                0
            } else {
                md.len()
            };
            (
                kind,
                size,
                md.modified().ok(),
                md.created().ok(),
                executable,
            )
        }
        Err(_) => (
            fallback.map_or(EntryKind::Other, kind_of),
            0,
            None,
            None,
            false,
        ),
    };
    // Only a symlink needs a second call, to learn what it points at.
    let mut is_dir = match kind {
        EntryKind::Directory => true,
        EntryKind::Symlink => std::fs::metadata(&path).is_ok_and(|md| md.is_dir()),
        _ => false,
    };
    let mut is_package = false;
    if is_dir && has_package_extension(&name) {
        is_dir = false;
        is_package = true;
    }
    Entry {
        name,
        path,
        kind,
        is_dir,
        is_package,
        size,
        modified,
        created,
        hidden,
        executable,
    }
}

fn kind_of(file_type: std::fs::FileType) -> EntryKind {
    if file_type.is_symlink() {
        EntryKind::Symlink
    } else if file_type.is_dir() {
        EntryKind::Directory
    } else if file_type.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    }
}

pub(crate) fn has_package_extension(name: &str) -> bool {
    match name.rfind('.') {
        Some(index) if index > 0 => {
            let ext = &name[index + 1..];
            PACKAGE_EXTENSIONS
                .iter()
                .any(|p| p.eq_ignore_ascii_case(ext))
        }
        _ => false,
    }
}

/// The number of bytes that `is_text_file` reads.
const TEXT_PROBE_LEN: usize = 8192;

/// Is the file a text file? The check reads the first 8192 bytes.
/// A text file has no NUL byte and holds valid UTF-8. An empty file is text.
/// The cut at the end of the probe may split a multibyte character. That is allowed.
/// Any I/O error gives `false`, for example for a directory.
pub fn is_text_file(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = Vec::with_capacity(TEXT_PROBE_LEN);
    if file
        .take(TEXT_PROBE_LEN as u64)
        .read_to_end(&mut buf)
        .is_err()
    {
        return false;
    }
    if buf.contains(&0) {
        return false;
    }
    match std::str::from_utf8(&buf) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none() && buf.len() == TEXT_PROBE_LEN,
    }
}

/// Expands user input to an absolute path:
/// - `~` and `~/x` use the home directory.
/// - A relative path joins to `cwd`.
/// - The result goes through `normalize`.
pub fn expand_path(input: &str, cwd: &Path) -> PathBuf {
    let input = input.trim();
    if input.is_empty() {
        return normalize(cwd);
    }
    let joined = if input == "~" {
        home_dir()
    } else if let Some(rest) = input.strip_prefix("~/") {
        home_dir().join(rest)
    } else {
        // `join` keeps an absolute `input` and drops `cwd`.
        cwd.join(input)
    };
    normalize(&joined)
}

/// Lexical clean-up, no disk access: removes `.` components and resolves
/// `..` against the previous component. Never goes above `/`.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    let mut rooted = false;
    let mut depth = 0usize;
    for component in path.components() {
        match component {
            Component::Prefix(_) => out.push(component.as_os_str()),
            Component::RootDir => {
                rooted = true;
                out.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if depth > 0 {
                    out.pop();
                    depth -= 1;
                } else if !rooted {
                    // A relative path keeps the `..` that it cannot resolve.
                    out.push("..");
                }
            }
            Component::Normal(part) => {
                out.push(part);
                depth += 1;
            }
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// `~/x` form of `path` when it is inside the home directory, for titles and
/// the status bar.
pub fn display_path(path: &Path) -> String {
    let home = home_dir();
    if home != Path::new("/") {
        if path == home {
            return "~".to_string();
        }
        if let Ok(rest) = path.strip_prefix(&home) {
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

/// The home directory. Falls back to `/` when it is not known.
pub fn home_dir() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) if !home.is_empty() => PathBuf::from(home),
        _ => std::env::home_dir().unwrap_or_else(|| PathBuf::from("/")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn text_probe(bytes: &[u8]) -> bool {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        fs::write(&path, bytes).unwrap();
        is_text_file(&path)
    }

    #[test]
    fn text_file_accepts_ascii_and_utf8() {
        assert!(text_probe(b"hello\nworld\n"));
        assert!(text_probe("caf\u{e9} \u{4e16}\u{754c}\n".as_bytes()));
    }

    #[test]
    fn text_file_accepts_an_empty_file() {
        assert!(text_probe(b""));
    }

    #[test]
    fn text_file_rejects_a_nul_byte() {
        assert!(!text_probe(b"ab\0cd"));
    }

    #[test]
    fn text_file_rejects_invalid_utf8() {
        assert!(!text_probe(b"ab\xe9cd"));
    }

    #[test]
    fn text_file_accepts_a_cut_at_a_character_boundary() {
        let mut bytes = vec![b'a'; TEXT_PROBE_LEN - 1];
        bytes.extend("\u{e9}".as_bytes());
        bytes.extend(b"tail");
        assert!(bytes.len() > TEXT_PROBE_LEN);
        assert!(text_probe(&bytes));
    }

    #[test]
    fn text_file_rejects_a_short_incomplete_sequence() {
        assert!(!text_probe(b"ab\xc3"));
    }

    #[test]
    fn text_file_rejects_a_directory_and_a_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_text_file(dir.path()));
        assert!(!is_text_file(&dir.path().join("missing")));
    }

    fn find<'a>(entries: &'a [Entry], name: &str) -> &'a Entry {
        entries
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("no entry {name}"))
    }

    #[test]
    fn extension_rules() {
        let mut e = entry_for(Path::new("/")).unwrap();
        for (name, ext) in [
            ("a.txt", "txt"),
            (".zshrc", ""),
            ("a.tar.gz", "gz"),
            ("noext", ""),
            (".a.b", "b"),
            ("a.", ""),
        ] {
            e.name = name.to_string();
            assert_eq!(e.extension(), ext, "{name}");
        }
    }

    #[test]
    fn read_dir_kinds() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("file.txt"), b"hello").unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join(".dot"), b"").unwrap();
        symlink(root.join("sub"), root.join("link_dir")).unwrap();
        symlink(root.join("nowhere"), root.join("broken")).unwrap();
        fs::write(root.join("run.sh"), b"#!/bin/sh").unwrap();
        fs::set_permissions(root.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::create_dir(root.join("Foo.app")).unwrap();
        symlink(root.join("Foo.app"), root.join("link.app")).unwrap();

        let entries = read_dir(root).unwrap();
        assert_eq!(entries.len(), 8);

        let f = find(&entries, "file.txt");
        assert_eq!(
            (f.kind, f.is_dir, f.is_package, f.size),
            (EntryKind::File, false, false, 5)
        );
        assert!(f.modified.is_some() && !f.hidden && !f.executable);
        assert!(f.created.is_some());
        assert_eq!(f.path, root.join("file.txt"));

        let d = find(&entries, "sub");
        assert_eq!((d.kind, d.is_dir, d.size), (EntryKind::Directory, true, 0));

        assert!(find(&entries, ".dot").hidden);

        let l = find(&entries, "link_dir");
        assert_eq!(
            (l.kind, l.is_dir, l.is_package),
            (EntryKind::Symlink, true, false)
        );

        let b = find(&entries, "broken");
        assert_eq!((b.kind, b.is_dir), (EntryKind::Symlink, false));

        let x = find(&entries, "run.sh");
        assert!(x.executable);

        let app = find(&entries, "Foo.app");
        assert_eq!(
            (app.kind, app.is_dir, app.is_package),
            (EntryKind::Directory, false, true)
        );
        let app_link = find(&entries, "link.app");
        assert_eq!(
            (app_link.kind, app_link.is_dir, app_link.is_package),
            (EntryKind::Symlink, false, true)
        );
    }

    #[test]
    fn package_extension_ignores_case() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("X.APP")).unwrap();
        fs::create_dir(dir.path().join(".app")).unwrap();
        let entries = read_dir(dir.path()).unwrap();
        assert!(find(&entries, "X.APP").is_package);
        // A leading dot is not an extension.
        assert!(find(&entries, ".app").is_dir);
    }

    #[test]
    fn hidden_flag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flagged");
        fs::write(&path, b"x").unwrap();
        let ok = std::process::Command::new("chflags")
            .arg("hidden")
            .arg(&path)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return;
        }
        let entries = read_dir(dir.path()).unwrap();
        assert!(find(&entries, "flagged").hidden);
    }

    #[test]
    fn read_dir_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_dir(&dir.path().join("missing")).is_err());
        fs::write(dir.path().join("f"), b"").unwrap();
        assert!(read_dir(&dir.path().join("f")).is_err());
    }

    #[test]
    fn entry_for_paths() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), b"abc").unwrap();
        let e = entry_for(&dir.path().join("a.txt")).unwrap();
        assert_eq!((e.name.as_str(), e.size), ("a.txt", 3));
        assert_eq!(entry_for(Path::new("/")).unwrap().name, "/");
        assert!(entry_for(&dir.path().join("nope")).is_err());
    }

    #[test]
    fn normalize_cases() {
        let cases = [
            ("/a/./b/../c", "/a/c"),
            ("/../..", "/"),
            ("/a/b/..", "/a"),
            ("/", "/"),
            ("a/../..", ".."),
            ("a/..", "."),
            ("/a//b/", "/a/b"),
        ];
        for (input, want) in cases {
            assert_eq!(normalize(Path::new(input)), PathBuf::from(want), "{input}");
        }
    }

    #[test]
    fn expand_path_cases() {
        let cwd = Path::new("/work/dir");
        let home = home_dir();
        assert_eq!(expand_path("", cwd), PathBuf::from("/work/dir"));
        assert_eq!(expand_path("~", cwd), normalize(&home));
        assert_eq!(expand_path("~/x/../y", cwd), normalize(&home.join("y")));
        assert_eq!(expand_path("sub", cwd), PathBuf::from("/work/dir/sub"));
        assert_eq!(expand_path("../up", cwd), PathBuf::from("/work/up"));
        assert_eq!(expand_path("/abs/./p", cwd), PathBuf::from("/abs/p"));
        assert_eq!(expand_path("~user", cwd), PathBuf::from("/work/dir/~user"));
    }

    #[test]
    fn display_path_cases() {
        let home = home_dir();
        if home != Path::new("/") {
            assert_eq!(display_path(&home), "~");
            assert_eq!(display_path(&home.join("a/b")), "~/a/b");
        }
        assert_eq!(display_path(Path::new("/usr/lib")), "/usr/lib");
    }
}
