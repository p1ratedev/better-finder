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
        Self { key: SortKey::Name, reverse: false, directories_first: true }
    }
}

/// Text forms: `name`, `size`, `modified`, `extension`.
impl FromStr for SortKey {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let _ = s;
        todo!()
    }
}

impl fmt::Display for SortKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = f;
        todo!()
    }
}

/// Natural order, like Finder: case-insensitive, and digit runs compare as
/// numbers (`file2` < `file10`). When two names are equal by this rule, a
/// plain byte compare breaks the tie, so the order is total and stable.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let _ = (a, b);
    todo!()
}

/// Sorts in place.
///
/// - `Name`: `natural_cmp` on the name.
/// - `Size`: smallest first. Directories have no size; they sort by name.
/// - `Modified`: newest first. Unknown time goes last.
/// - `Extension`: `natural_cmp` on the extension, then on the name.
///
/// Ties always fall back to `natural_cmp` on the name. `reverse` flips the
/// order of the key, but `directories_first` still applies.
pub fn sort_entries(entries: &mut [Entry], opts: SortOptions) {
    let _ = (entries, opts);
    todo!()
}
