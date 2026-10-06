//! Sort order for directory entries.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use crate::fs::Entry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Modified,
    /// The birth time. Newest first, like `Modified`.
    Created,
    Extension,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortOptions {
    pub key: SortKey,
    pub reverse: bool,
    /// Directories (and symlinks to directories) come before files.
    /// Packages such as `.app` count as files. `reverse` does not change this.
    pub directories_first: bool,
}

impl Default for SortOptions {
    fn default() -> Self {
        Self {
            key: SortKey::Name,
            reverse: false,
            directories_first: true,
        }
    }
}

/// Text forms: `name`, `size`, `modified`, `extension`.
impl FromStr for SortKey {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "name" => Ok(Self::Name),
            "size" => Ok(Self::Size),
            "modified" => Ok(Self::Modified),
            "created" => Ok(Self::Created),
            "extension" => Ok(Self::Extension),
            _ => Err(format!(
                "unknown sort key: {s} (use name, size, modified, created, or extension)"
            )),
        }
    }
}

impl fmt::Display for SortKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Name => "name",
            Self::Size => "size",
            Self::Modified => "modified",
            Self::Created => "created",
            Self::Extension => "extension",
        })
    }
}

/// Natural order, like Finder: case-insensitive, and digit runs compare as
/// numbers (`file2` < `file10`). When two names are equal by this rule, a
/// plain byte compare breaks the tie, so the order is total and stable.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut a_chars = a.chars().peekable();
    let mut b_chars = b.chars().peekable();
    loop {
        match (a_chars.peek().copied(), b_chars.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let run_a = take_digits(&mut a_chars);
                let run_b = take_digits(&mut b_chars);
                let order = cmp_digit_runs(&run_a, &run_b);
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                a_chars.next();
                b_chars.next();
                let order = cmp_lower(x, y);
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

fn take_digits(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut run = String::new();
    while let Some(c) = chars.peek().copied().filter(char::is_ascii_digit) {
        run.push(c);
        chars.next();
    }
    run
}

/// Compares two digit runs as numbers, with no integer conversion.
fn cmp_digit_runs(a: &str, b: &str) -> Ordering {
    let a = a.trim_start_matches('0');
    let b = b.trim_start_matches('0');
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

fn cmp_lower(a: char, b: char) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    a.to_lowercase().cmp(b.to_lowercase())
}

/// Sorts in place.
///
/// - `Name`: `natural_cmp` on the name.
/// - `Size`: smallest first. Directories have no size; they sort by name.
/// - `Modified`: newest first. Unknown time goes last.
/// - `Created`: like `Modified`, with the birth time.
/// - `Extension`: `natural_cmp` on the extension, then on the name.
///
/// Ties always fall back to `natural_cmp` on the name. `reverse` flips the
/// order of the key, but `directories_first` still applies.
pub fn sort_entries(entries: &mut [Entry], opts: SortOptions) {
    let SortOptions {
        key,
        reverse,
        directories_first,
    } = opts;
    let flip = |order: Ordering| if reverse { order.reverse() } else { order };
    let by_name = |a: &Entry, b: &Entry| flip(natural_cmp(&a.name, &b.name));
    let by_key = |a: &Entry, b: &Entry| -> Ordering {
        match key {
            SortKey::Name => by_name(a, b),
            SortKey::Size => {
                // Directories have no size: they count as zero and sort by name.
                let size = |e: &Entry| if e.is_dir { 0 } else { e.size };
                flip(size(a).cmp(&size(b))).then_with(|| by_name(a, b))
            }
            SortKey::Modified | SortKey::Created => {
                let time = |e: &Entry| {
                    if key == SortKey::Modified {
                        e.modified
                    } else {
                        e.created
                    }
                };
                match (time(a), time(b)) {
                    // Unknown time goes last in both directions.
                    (None, None) => by_name(a, b),
                    (None, Some(_)) => Ordering::Greater,
                    (Some(_), None) => Ordering::Less,
                    (Some(x), Some(y)) => flip(y.cmp(&x)).then_with(|| by_name(a, b)),
                }
            }
            SortKey::Extension => {
                flip(natural_cmp(a.extension(), b.extension())).then_with(|| by_name(a, b))
            }
        }
    };
    if directories_first {
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| by_key(a, b)));
    } else {
        entries.sort_by(by_key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::EntryKind;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn entry(name: &str, is_dir: bool, size: u64, secs: Option<u64>) -> Entry {
        Entry {
            name: name.to_string(),
            path: PathBuf::from("/t").join(name),
            kind: if is_dir {
                EntryKind::Directory
            } else {
                EntryKind::File
            },
            is_dir,
            is_package: false,
            size,
            modified: secs.map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s)),
            created: secs.map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(1000 - s)),
            hidden: false,
            executable: false,
        }
    }

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn natural_numbers() {
        assert_eq!(natural_cmp("file2", "file10"), Ordering::Less);
        assert_eq!(natural_cmp("file10", "file2"), Ordering::Greater);
        assert_eq!(natural_cmp("a1b2", "a1b10"), Ordering::Less);
    }

    #[test]
    fn natural_ignores_case_then_breaks_tie() {
        assert_eq!(natural_cmp("Apple", "banana"), Ordering::Less);
        assert_eq!(natural_cmp("apple", "Banana"), Ordering::Less);
        assert_eq!(natural_cmp("a", "A"), "a".cmp("A"));
        assert_eq!(natural_cmp("same", "same"), Ordering::Equal);
    }

    #[test]
    fn natural_leading_zeros() {
        assert_eq!(natural_cmp("a007", "a8"), Ordering::Less);
        assert_eq!(natural_cmp("a08", "a8"), "a08".cmp("a8"));
        assert_eq!(natural_cmp("a0", "a00"), "a0".cmp("a00"));
    }

    #[test]
    fn natural_long_digit_runs() {
        let big = "9".repeat(60);
        let bigger = format!("1{}", "0".repeat(60));
        let a = format!("f{big}");
        let b = format!("f{bigger}");
        assert_eq!(natural_cmp(&a, &b), Ordering::Less);
        assert_eq!(natural_cmp(&b, &a), Ordering::Greater);
        assert_eq!(natural_cmp(&a, &a), Ordering::Equal);
    }

    #[test]
    fn natural_prefix_is_less() {
        assert_eq!(natural_cmp("a", "ab"), Ordering::Less);
        assert_eq!(natural_cmp("a1", "a1b"), Ordering::Less);
    }

    #[test]
    fn sort_key_text() {
        for k in [
            SortKey::Name,
            SortKey::Size,
            SortKey::Modified,
            SortKey::Created,
            SortKey::Extension,
        ] {
            assert_eq!(k.to_string().parse::<SortKey>().unwrap(), k);
        }
        assert!("bogus".parse::<SortKey>().is_err());
    }

    fn sample() -> Vec<Entry> {
        vec![
            entry("b.txt", false, 30, Some(200)),
            entry("a.rs", false, 10, Some(300)),
            entry("Zdir", true, 0, Some(100)),
            entry("c.md", false, 20, None),
            entry("adir", true, 0, Some(400)),
        ]
    }

    fn opts(key: SortKey, reverse: bool, directories_first: bool) -> SortOptions {
        SortOptions {
            key,
            reverse,
            directories_first,
        }
    }

    #[test]
    fn sort_by_name() {
        let mut e = sample();
        sort_entries(&mut e, opts(SortKey::Name, false, true));
        assert_eq!(names(&e), ["adir", "Zdir", "a.rs", "b.txt", "c.md"]);
        sort_entries(&mut e, opts(SortKey::Name, false, false));
        assert_eq!(names(&e), ["a.rs", "adir", "b.txt", "c.md", "Zdir"]);
    }

    #[test]
    fn reverse_keeps_directories_first() {
        let mut e = sample();
        sort_entries(&mut e, opts(SortKey::Name, true, true));
        assert_eq!(names(&e), ["Zdir", "adir", "c.md", "b.txt", "a.rs"]);
    }

    #[test]
    fn sort_by_size() {
        let mut e = sample();
        sort_entries(&mut e, opts(SortKey::Size, false, true));
        assert_eq!(names(&e), ["adir", "Zdir", "a.rs", "c.md", "b.txt"]);
        sort_entries(&mut e, opts(SortKey::Size, true, true));
        assert_eq!(names(&e), ["Zdir", "adir", "b.txt", "c.md", "a.rs"]);
    }

    #[test]
    fn sort_by_modified_none_last() {
        let mut e = sample();
        sort_entries(&mut e, opts(SortKey::Modified, false, false));
        assert_eq!(names(&e), ["adir", "a.rs", "b.txt", "Zdir", "c.md"]);
        sort_entries(&mut e, opts(SortKey::Modified, true, false));
        assert_eq!(names(&e), ["Zdir", "b.txt", "a.rs", "adir", "c.md"]);
    }

    #[test]
    fn sort_by_created_none_last() {
        // `entry` sets the birth time to 1000 - the modified time.
        let mut e = sample();
        sort_entries(&mut e, opts(SortKey::Created, false, false));
        assert_eq!(names(&e), ["Zdir", "b.txt", "a.rs", "adir", "c.md"]);
        sort_entries(&mut e, opts(SortKey::Created, true, false));
        assert_eq!(names(&e), ["adir", "a.rs", "b.txt", "Zdir", "c.md"]);
    }

    #[test]
    fn sort_by_extension_then_name() {
        let mut e = vec![
            entry("z.rs", false, 0, None),
            entry("a.txt", false, 0, None),
            entry("b.rs", false, 0, None),
            entry("noext", false, 0, None),
        ];
        sort_entries(&mut e, opts(SortKey::Extension, false, true));
        assert_eq!(names(&e), ["noext", "b.rs", "z.rs", "a.txt"]);
        sort_entries(&mut e, opts(SortKey::Extension, true, true));
        assert_eq!(names(&e), ["a.txt", "z.rs", "b.rs", "noext"]);
    }
}
