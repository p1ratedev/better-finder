//! New names for copies, aliases, and archives, like Finder.
//!
//! | Style | Names |
//! | --- | --- |
//! | `Duplicate` | `report copy.pdf`, `report copy 2.pdf`, ... |
//! | `KeepBoth` | `report.pdf`, `report 2.pdf`, ... |
//! | `Alias` | `report.pdf alias`, `report.pdf alias 2`, ... |
//! | `Zip` | `report.pdf.zip`, `report.pdf 2.zip`, ... |
//!
//! These functions only make names. The caller tries the names in order,
//! with an operation that fails when the name exists, and takes the first
//! one that works. So a new item never replaces an item.

/// How a new name comes from the name of the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameStyle {
    /// A copy in the same folder: `report copy.pdf`.
    Duplicate,
    /// A copy or a move to another folder. The first name is the source
    /// name. When it exists, the Finder "Keep Both" names follow:
    /// `report 2.pdf`.
    KeepBoth,
    /// A Finder alias file: `report.pdf alias`.
    Alias,
    /// A zip archive: `report.pdf.zip`.
    Zip,
}

/// The most names that `candidates` gives.
pub const MAX_CANDIDATES: u32 = 10_000;

/// Splits `name` into the stem and the extension (without the dot).
///
/// - A plain folder (`folder` is true) has no extension. A package (for
///   example `Safari.app`) is not a plain folder: it has an extension.
/// - A leading dot does not start an extension: `.zshrc` has none.
/// - An empty extension (`notes.`) or an extension with a space
///   (`Mr. Smith`) is not an extension.
pub fn split_extension(name: &str, folder: bool) -> (&str, Option<&str>) {
    if folder {
        return (name, None);
    }
    match name.rfind('.') {
        Some(i) if i > 0 => {
            let ext = &name[i + 1..];
            if ext.is_empty() || ext.contains(' ') {
                (name, None)
            } else {
                (&name[..i], Some(ext))
            }
        }
        _ => (name, None),
    }
}

/// Removes a Finder copy suffix (` copy` or ` copy 3`) from a stem. So a
/// duplicate of `report copy.pdf` is `report copy 2.pdf`, not
/// `report copy copy.pdf`, like Finder.
fn strip_copy_suffix(stem: &str) -> &str {
    if let Some(base) = stem.strip_suffix(" copy") {
        if !base.is_empty() {
            return base;
        }
    }
    if let Some((base, number)) = stem.rsplit_once(" copy ") {
        if !base.is_empty() && !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) {
            return base;
        }
    }
    stem
}

/// Joins a stem, a suffix, and an optional extension.
fn join(stem: &str, suffix: &str, ext: Option<&str>) -> String {
    match ext {
        Some(ext) => format!("{stem}{suffix}.{ext}"),
        None => format!("{stem}{suffix}"),
    }
}

/// The name number `n` (the first is 1) for a new item from `name`.
/// `folder` is true for a plain folder (see `split_extension`).
pub fn candidate(style: NameStyle, name: &str, folder: bool, n: u32) -> String {
    match style {
        NameStyle::Duplicate => {
            let (stem, ext) = split_extension(name, folder);
            let base = strip_copy_suffix(stem);
            if n <= 1 {
                join(base, " copy", ext)
            } else {
                join(base, &format!(" copy {n}"), ext)
            }
        }
        NameStyle::KeepBoth => {
            if n <= 1 {
                return name.to_string();
            }
            let (stem, ext) = split_extension(name, folder);
            join(stem, &format!(" {n}"), ext)
        }
        NameStyle::Alias => {
            if n <= 1 {
                format!("{name} alias")
            } else {
                format!("{name} alias {n}")
            }
        }
        NameStyle::Zip => {
            if n <= 1 {
                format!("{name}.zip")
            } else {
                format!("{name} {n}.zip")
            }
        }
    }
}

/// All names for a new item from `name`, in order (the first
/// `MAX_CANDIDATES`).
pub fn candidates(style: NameStyle, name: &str, folder: bool) -> impl Iterator<Item = String> + '_ {
    (1..=MAX_CANDIDATES).map(move |n| candidate(style, name, folder, n))
}

/// The first name for which `exists` is false. `None` when all
/// `MAX_CANDIDATES` names exist.
pub fn first_free(
    style: NameStyle,
    name: &str,
    folder: bool,
    mut exists: impl FnMut(&str) -> bool,
) -> Option<String> {
    candidates(style, name, folder).find(|candidate| !exists(candidate))
}

/// Shortens `name` to `max` characters: the middle becomes `…`, like
/// Finder does in its menus. A short name does not change.
pub fn truncate_middle(name: &str, max: usize) -> String {
    let count = name.chars().count();
    if count <= max || max < 2 {
        return name.to_string();
    }
    let keep = max - 1;
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let start: String = name.chars().take(head).collect();
    let end: String = name.chars().skip(count - tail).collect();
    format!("{start}\u{2026}{end}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(style: NameStyle, name: &str, folder: bool, count: u32) -> Vec<String> {
        (1..=count)
            .map(|n| candidate(style, name, folder, n))
            .collect()
    }

    #[test]
    fn extensions() {
        assert_eq!(
            split_extension("report.pdf", false),
            ("report", Some("pdf"))
        );
        assert_eq!(split_extension("a.tar.gz", false), ("a.tar", Some("gz")));
        assert_eq!(split_extension("Makefile", false), ("Makefile", None));
        assert_eq!(split_extension(".zshrc", false), (".zshrc", None));
        assert_eq!(
            split_extension(".config.json", false),
            (".config", Some("json"))
        );
        assert_eq!(split_extension("notes.", false), ("notes.", None));
        assert_eq!(split_extension("Mr. Smith", false), ("Mr. Smith", None));
        assert_eq!(split_extension("v1.2", true), ("v1.2", None));
        assert_eq!(
            split_extension("Safari.app", false),
            ("Safari", Some("app"))
        );
    }

    #[test]
    fn duplicate_of_a_file() {
        assert_eq!(
            names(NameStyle::Duplicate, "report.pdf", false, 3),
            ["report copy.pdf", "report copy 2.pdf", "report copy 3.pdf"]
        );
    }

    #[test]
    fn duplicate_without_an_extension() {
        assert_eq!(
            names(NameStyle::Duplicate, "Makefile", false, 2),
            ["Makefile copy", "Makefile copy 2"]
        );
    }

    #[test]
    fn duplicate_of_a_dot_file() {
        assert_eq!(
            names(NameStyle::Duplicate, ".zshrc", false, 2),
            [".zshrc copy", ".zshrc copy 2"]
        );
    }

    #[test]
    fn duplicate_of_a_folder() {
        assert_eq!(
            names(NameStyle::Duplicate, "Folder", true, 2),
            ["Folder copy", "Folder copy 2"]
        );
        // A folder name with a dot keeps the dot in the stem.
        assert_eq!(
            candidate(NameStyle::Duplicate, "v1.2", true, 1),
            "v1.2 copy"
        );
    }

    #[test]
    fn duplicate_of_a_package_keeps_the_extension_at_the_end() {
        assert_eq!(
            names(NameStyle::Duplicate, "Safari.app", false, 2),
            ["Safari copy.app", "Safari copy 2.app"]
        );
    }

    #[test]
    fn duplicate_of_a_copy_counts_on() {
        assert_eq!(
            candidate(NameStyle::Duplicate, "report copy.pdf", false, 1),
            "report copy.pdf"
        );
        assert_eq!(
            candidate(NameStyle::Duplicate, "report copy.pdf", false, 2),
            "report copy 2.pdf"
        );
        assert_eq!(
            candidate(NameStyle::Duplicate, "report copy 7.pdf", false, 3),
            "report copy 3.pdf"
        );
        // Only a real suffix counts.
        assert_eq!(
            candidate(NameStyle::Duplicate, "copy.txt", false, 1),
            "copy copy.txt"
        );
        assert_eq!(
            candidate(NameStyle::Duplicate, "a copy x.txt", false, 1),
            "a copy x copy.txt"
        );
    }

    #[test]
    fn duplicate_counter_skips_names_that_exist() {
        let taken = ["report copy.pdf", "report copy 2.pdf"];
        let free = first_free(NameStyle::Duplicate, "report.pdf", false, |n| {
            taken.contains(&n)
        });
        assert_eq!(free.as_deref(), Some("report copy 3.pdf"));
        assert_eq!(first_free(NameStyle::Zip, "a", false, |_| true), None);
    }

    #[test]
    fn keep_both_names() {
        assert_eq!(
            names(NameStyle::KeepBoth, "file.txt", false, 3),
            ["file.txt", "file 2.txt", "file 3.txt"]
        );
        assert_eq!(
            names(NameStyle::KeepBoth, "Folder", true, 2),
            ["Folder", "Folder 2"]
        );
        assert_eq!(
            names(NameStyle::KeepBoth, ".zshrc", false, 2),
            [".zshrc", ".zshrc 2"]
        );
    }

    #[test]
    fn alias_names() {
        assert_eq!(
            names(NameStyle::Alias, "report.pdf", false, 3),
            [
                "report.pdf alias",
                "report.pdf alias 2",
                "report.pdf alias 3"
            ]
        );
        assert_eq!(
            candidate(NameStyle::Alias, "Folder", true, 1),
            "Folder alias"
        );
    }

    #[test]
    fn zip_names() {
        assert_eq!(
            names(NameStyle::Zip, "report.pdf", false, 3),
            ["report.pdf.zip", "report.pdf 2.zip", "report.pdf 3.zip"]
        );
        assert_eq!(candidate(NameStyle::Zip, "Folder", true, 2), "Folder 2.zip");
    }

    #[test]
    fn candidates_are_limited() {
        assert_eq!(
            candidates(NameStyle::KeepBoth, "a", false).count(),
            MAX_CANDIDATES as usize
        );
    }

    #[test]
    fn truncation_in_the_middle() {
        assert_eq!(truncate_middle("short.txt", 40), "short.txt");
        let long = "a".repeat(30) + &"b".repeat(30);
        let cut = truncate_middle(&long, 40);
        assert_eq!(cut.chars().count(), 40);
        assert!(cut.starts_with(&"a".repeat(20)));
        assert!(cut.ends_with(&"b".repeat(19)));
        assert!(cut.contains('\u{2026}'));
        // Characters, not bytes.
        assert_eq!(truncate_middle("ééééé", 4), "éé\u{2026}é");
    }
}
