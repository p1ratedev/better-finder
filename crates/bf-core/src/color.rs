//! Colors for themes.
//!
//! Accepted text forms (like Ghostty): `#rgb`, `#rrggbb`, `#rrggbbaa`, and the
//! same forms without `#`. Hex digits ignore case.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Components in the range 0.0..=1.0, for `NSColor`.
    pub fn to_f64(self) -> (f64, f64, f64, f64) {
        todo!()
    }

    pub fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseColorError(pub String);

impl fmt::Display for ParseColorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid color: {}", self.0)
    }
}

impl std::error::Error for ParseColorError {}

impl FromStr for Rgba {
    type Err = ParseColorError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let _ = s;
        todo!()
    }
}

/// Writes `#rrggbb`, or `#rrggbbaa` when alpha is not 255.
impl fmt::Display for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = f;
        todo!()
    }
}
