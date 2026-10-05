//! Live filter for the file list.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FilterMode {
    /// The name contains the query.
    #[default]
    Substring,
    /// The characters of the query occur in the name in the same order.
    Fuzzy,
}

/// Text forms: `substring`, `fuzzy`.
impl FromStr for FilterMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let _ = s;
        todo!()
    }
}

impl fmt::Display for FilterMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = f;
        todo!()
    }
}

/// Smart case: when `query` has no uppercase letter, the match ignores case.
/// When `query` has an uppercase letter, the match is case-sensitive.
/// An empty query matches all names.
pub fn matches(mode: FilterMode, query: &str, name: &str) -> bool {
    let _ = (mode, query, name);
    todo!()
}
