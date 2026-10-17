//! The pure rules of a file transfer: copy, move, or alias of items into a
//! folder. `bf-app` (`file_ops::transfer`) does the file system work.
//!
//! - A transfer never replaces an item. A name that exists gives the next
//!   name of the `NameStyle` (see `names`).
//! - A copy into the folder of the source gets the duplicate name
//!   (`file copy.txt`). Other copies and moves get the Keep Both names
//!   (`file 2.txt`).
//! - A move into the folder of the source does nothing.
//! - A folder never goes into itself or into one of its subfolders.

use std::path::Path;

use crate::names::NameStyle;

/// What a transfer does with each source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferMode {
    Copy,
    /// A rename on the same volume. Across volumes, a move is a copy: the
    /// source stays.
    Move,
    /// A Finder alias file of the source.
    Alias,
}

impl TransferMode {
    /// The name style of the new item. `same_folder` is true when the
    /// source is in the destination folder already.
    pub fn name_style(self, same_folder: bool) -> NameStyle {
        match self {
            Self::Alias => NameStyle::Alias,
            Self::Copy if same_folder => NameStyle::Duplicate,
            Self::Copy | Self::Move => NameStyle::KeepBoth,
        }
    }

    /// The verb for messages: `copy`, `move`, `make an alias of`.
    pub fn verb(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Move => "move",
            Self::Alias => "make an alias of",
        }
    }
}

/// The progress message of a transfer: `Copying "a"…`, `Moving 3 items…`.
pub fn progress_text(mode: TransferMode, names: &[String]) -> String {
    let verb = match mode {
        TransferMode::Copy => "Copying",
        TransferMode::Move => "Moving",
        TransferMode::Alias => "Making",
    };
    match (names, mode) {
        ([name], TransferMode::Alias) => format!("Making an alias of \"{name}\"\u{2026}"),
        ([name], _) => format!("{verb} \"{name}\"\u{2026}"),
        (_, TransferMode::Alias) => format!("Making {} aliases\u{2026}", names.len()),
        _ => format!("{verb} {} items\u{2026}", names.len()),
    }
}

/// True when `path` is `folder` or is inside it. Use canonical paths (no
/// symlinks, no `..`): the test compares path components.
pub fn is_within(path: &Path, folder: &Path) -> bool {
    path.starts_with(folder)
}

/// The work that a transfer did for one source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Done {
    Copied,
    Moved,
    Aliased,
}

/// The result of a transfer for one source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemResult {
    /// A new item. `source` and `new` are file names.
    Done {
        done: Done,
        source: String,
        new: String,
    },
    /// A move into the folder of the source: nothing to do.
    Skipped { name: String },
    /// A full sentence, for example `Cannot copy "x": reason`.
    Failed(String),
}

/// The text of one group of results: `Copied "a"`, `Copied "a" as "b"`,
/// or `Copied 3 items`.
fn group_text(done: Done, items: &[(&String, &String)]) -> Option<String> {
    let (verb, plural) = match done {
        Done::Copied => ("Copied", "items"),
        Done::Moved => ("Moved", "items"),
        Done::Aliased => ("Made", "aliases"),
    };
    match items {
        [] => None,
        [(source, new)] => Some(match done {
            Done::Aliased => format!("Made the alias \"{new}\""),
            _ if source != new => format!("{verb} \"{source}\" as \"{new}\""),
            _ => format!("{verb} \"{new}\""),
        }),
        many => Some(format!("{verb} {} {plural}", many.len())),
    }
}

/// Makes the first letter lowercase (for a group after a comma).
fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// One status message for all results, and true when it is an error.
/// `None` when there are no results.
///
/// - The new items: `Moved 2 items, copied "a" as "a 2"`.
/// - The first failure, with `(+N more)` for the other failures. A failure
///   makes the message an error.
/// - Only skipped moves: `"x" is in this folder already`.
pub fn summary(results: &[ItemResult]) -> Option<(String, bool)> {
    let mut parts = Vec::new();
    for done in [Done::Moved, Done::Copied, Done::Aliased] {
        let items: Vec<(&String, &String)> = results
            .iter()
            .filter_map(|r| match r {
                ItemResult::Done {
                    done: d,
                    source,
                    new,
                } if *d == done => Some((source, new)),
                _ => None,
            })
            .collect();
        if let Some(text) = group_text(done, &items) {
            parts.push(if parts.is_empty() {
                text
            } else {
                lower_first(&text)
            });
        }
    }
    let failures: Vec<&String> = results
        .iter()
        .filter_map(|r| match r {
            ItemResult::Failed(text) => Some(text),
            _ => None,
        })
        .collect();
    let skipped: Vec<&String> = results
        .iter()
        .filter_map(|r| match r {
            ItemResult::Skipped { name } => Some(name),
            _ => None,
        })
        .collect();
    let done_text = parts.join(", ");
    if let Some(first) = failures.first() {
        let mut text = first.to_string();
        if failures.len() > 1 {
            text.push_str(&format!(" (+{} more)", failures.len() - 1));
        }
        if !done_text.is_empty() {
            text = format!("{done_text}. {text}");
        }
        return Some((text, true));
    }
    if !done_text.is_empty() {
        return Some((done_text, false));
    }
    match skipped.as_slice() {
        [] => None,
        [name] => Some((format!("\"{name}\" is in this folder already"), false)),
        _ => Some(("The items are in this folder already".to_string(), false)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(done: Done, source: &str, new: &str) -> ItemResult {
        ItemResult::Done {
            done,
            source: source.to_string(),
            new: new.to_string(),
        }
    }

    #[test]
    fn inside_itself() {
        assert!(is_within(Path::new("/a/b"), Path::new("/a/b")));
        assert!(is_within(Path::new("/a/b/c/d"), Path::new("/a/b")));
        assert!(!is_within(Path::new("/a/bc"), Path::new("/a/b")));
        assert!(!is_within(Path::new("/a"), Path::new("/a/b")));
        assert!(!is_within(Path::new("/x/b"), Path::new("/a/b")));
    }

    #[test]
    fn name_styles() {
        assert_eq!(TransferMode::Copy.name_style(true), NameStyle::Duplicate);
        assert_eq!(TransferMode::Copy.name_style(false), NameStyle::KeepBoth);
        assert_eq!(TransferMode::Move.name_style(false), NameStyle::KeepBoth);
        assert_eq!(TransferMode::Alias.name_style(true), NameStyle::Alias);
        assert_eq!(TransferMode::Alias.name_style(false), NameStyle::Alias);
    }

    #[test]
    fn progress() {
        let one = ["a.txt".to_string()];
        let two = ["a".to_string(), "b".to_string()];
        assert_eq!(
            progress_text(TransferMode::Copy, &one),
            "Copying \"a.txt\"\u{2026}"
        );
        assert_eq!(
            progress_text(TransferMode::Move, &two),
            "Moving 2 items\u{2026}"
        );
        assert_eq!(
            progress_text(TransferMode::Alias, &one),
            "Making an alias of \"a.txt\"\u{2026}"
        );
        assert_eq!(
            progress_text(TransferMode::Alias, &two),
            "Making 2 aliases\u{2026}"
        );
    }

    #[test]
    fn summary_of_one_item() {
        let one = |r| summary(&[r]).unwrap();
        assert_eq!(
            one(done(Done::Copied, "a.txt", "a.txt")),
            ("Copied \"a.txt\"".to_string(), false)
        );
        assert_eq!(
            one(done(Done::Copied, "a.txt", "a copy.txt")).0,
            "Copied \"a.txt\" as \"a copy.txt\""
        );
        assert_eq!(one(done(Done::Moved, "a", "a")).0, "Moved \"a\"");
        assert_eq!(
            one(done(Done::Aliased, "a.pdf", "a.pdf alias")).0,
            "Made the alias \"a.pdf alias\""
        );
    }

    #[test]
    fn summary_of_many_items() {
        let results = [
            done(Done::Copied, "a", "a"),
            done(Done::Copied, "b", "b 2"),
            done(Done::Moved, "c", "c"),
        ];
        assert_eq!(
            summary(&results),
            Some(("Moved \"c\", copied 2 items".to_string(), false))
        );
        let aliases = [
            done(Done::Aliased, "a", "a alias"),
            done(Done::Aliased, "b", "b alias"),
        ];
        assert_eq!(summary(&aliases).unwrap().0, "Made 2 aliases");
    }

    #[test]
    fn summary_with_failures() {
        let results = [
            done(Done::Copied, "a", "a"),
            ItemResult::Failed("Cannot copy \"b\": no space".to_string()),
            ItemResult::Failed("Cannot copy \"c\": no space".to_string()),
        ];
        assert_eq!(
            summary(&results),
            Some((
                "Copied \"a\". Cannot copy \"b\": no space (+1 more)".to_string(),
                true
            ))
        );
        let only = [ItemResult::Failed("Cannot move \"x\"".to_string())];
        assert_eq!(
            summary(&only),
            Some(("Cannot move \"x\"".to_string(), true))
        );
    }

    #[test]
    fn summary_of_skipped_moves() {
        let skipped = |name: &str| ItemResult::Skipped {
            name: name.to_string(),
        };
        assert_eq!(
            summary(&[skipped("x")]).unwrap().0,
            "\"x\" is in this folder already"
        );
        assert_eq!(
            summary(&[skipped("x"), skipped("y")]).unwrap().0,
            "The items are in this folder already"
        );
        // A done item hides the skipped ones.
        assert_eq!(
            summary(&[skipped("x"), done(Done::Moved, "y", "y")])
                .unwrap()
                .0,
            "Moved \"y\""
        );
        assert_eq!(summary(&[]), None);
    }
}
