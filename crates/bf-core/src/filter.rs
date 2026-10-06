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
        match s.trim().to_ascii_lowercase().as_str() {
            "substring" => Ok(Self::Substring),
            "fuzzy" => Ok(Self::Fuzzy),
            _ => Err(format!("unknown filter mode: {s} (use substring or fuzzy)")),
        }
    }
}

impl fmt::Display for FilterMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Substring => "substring",
            Self::Fuzzy => "fuzzy",
        })
    }
}

/// True when the text of the location and filter field is a path, not a
/// filter: it starts with `~`, contains `/`, or is `.` or `..`. A path goes
/// to a folder. Other text filters the current folder.
pub fn is_path_query(text: &str) -> bool {
    text.starts_with('~') || text.contains('/') || text == "." || text == ".."
}

/// Smart case: when `query` has no uppercase letter, the match ignores case.
/// When `query` has an uppercase letter, the match is case-sensitive.
/// An empty query matches all names.
pub fn matches(mode: FilterMode, query: &str, name: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let sensitive = query.chars().any(char::is_uppercase);
    if sensitive {
        match mode {
            FilterMode::Substring => name.contains(query),
            FilterMode::Fuzzy => is_subsequence(query.chars(), name),
        }
    } else {
        let name = name.to_lowercase();
        match mode {
            FilterMode::Substring => name.contains(query),
            FilterMode::Fuzzy => is_subsequence(query.chars(), &name),
        }
    }
}

/// True when the chars of `query` occur in `name` in the same order.
fn is_subsequence(query: impl Iterator<Item = char>, name: &str) -> bool {
    let mut name = name.chars();
    for q in query {
        if !name.any(|c| c == q) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use FilterMode::*;

    #[test]
    fn empty_query_matches() {
        assert!(matches(Substring, "", "x"));
        assert!(matches(Fuzzy, "", ""));
    }

    #[test]
    fn substring_smart_case() {
        assert!(matches(Substring, "read", "README.md"));
        assert!(matches(Substring, "READ", "README.md"));
        assert!(!matches(Substring, "Read", "README.md"));
        assert!(matches(Substring, "Read", "Readme"));
        assert!(!matches(Substring, "rdm", "readme"));
    }

    #[test]
    fn fuzzy_smart_case() {
        assert!(matches(Fuzzy, "rdm", "README.md"));
        assert!(matches(Fuzzy, "RM", "README.md"));
        assert!(!matches(Fuzzy, "Re", "README.md"));
        assert!(!matches(Fuzzy, "mdr", "readme.md"));
        assert!(matches(Fuzzy, "aa", "banana"));
        assert!(!matches(Fuzzy, "aaaa", "banana"));
    }

    #[test]
    fn path_queries() {
        for text in [
            "~", "~/", "~/Doc", "/", "/tmp", "a/b", "src/", ".", "..", "../x",
        ] {
            assert!(is_path_query(text), "{text}");
        }
        for text in ["", "read", ".git", "...", "a.b", "x~", " "] {
            assert!(!is_path_query(text), "{text}");
        }
        // A tilde starts a path, also without a slash (`~user` style).
        assert!(is_path_query("~x"));
    }

    #[test]
    fn mode_text_round_trips() {
        for m in [Substring, Fuzzy] {
            assert_eq!(m.to_string().parse::<FilterMode>().unwrap(), m);
        }
        assert!("x".parse::<FilterMode>().is_err());
    }
}
