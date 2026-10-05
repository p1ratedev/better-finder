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
        let c = |v: u8| f64::from(v) / 255.0;
        (c(self.r), c(self.g), c(self.b), c(self.a))
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
        let err = || ParseColorError(s.to_string());
        let hex = s.trim();
        let hex = hex.strip_prefix('#').unwrap_or(hex);
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(err());
        }
        let digit = |c: char| c.to_digit(16).map(|d| d as u8).ok_or_else(err);
        let chars: Vec<char> = hex.chars().collect();
        match chars.len() {
            3 => {
                let mut v = [0u8; 3];
                for (slot, c) in v.iter_mut().zip(&chars) {
                    let d = digit(*c)?;
                    *slot = d * 17;
                }
                Ok(Rgba::rgb(v[0], v[1], v[2]))
            }
            6 | 8 => {
                let mut v = [255u8; 4];
                for (slot, pair) in v.iter_mut().zip(chars.chunks(2)) {
                    *slot = digit(pair[0])? * 16 + digit(pair[1])?;
                }
                Ok(Rgba {
                    r: v[0],
                    g: v[1],
                    b: v[2],
                    a: v[3],
                })
            }
            _ => Err(err()),
        }
    }
}

/// Writes `#rrggbb`, or `#rrggbbaa` when alpha is not 255.
impl fmt::Display for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.a == 255 {
            write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            write!(
                f,
                "#{:02x}{:02x}{:02x}{:02x}",
                self.r, self.g, self.b, self.a
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_forms() {
        assert_eq!("#fff".parse::<Rgba>().unwrap(), Rgba::rgb(255, 255, 255));
        assert_eq!(
            "#1e1E2e".parse::<Rgba>().unwrap(),
            Rgba::rgb(0x1e, 0x1e, 0x2e)
        );
        assert_eq!(
            "1e1e2e".parse::<Rgba>().unwrap(),
            Rgba::rgb(0x1e, 0x1e, 0x2e)
        );
        assert_eq!("abc".parse::<Rgba>().unwrap(), Rgba::rgb(0xaa, 0xbb, 0xcc));
        assert_eq!(
            "#11223344".parse::<Rgba>().unwrap(),
            Rgba {
                r: 0x11,
                g: 0x22,
                b: 0x33,
                a: 0x44
            }
        );
    }

    #[test]
    fn rejects_bad_input() {
        for bad in [
            "", "#", "#ff", "#ffff", "#fffff", "#ggg", "red", "#1234567", "#+12345",
        ] {
            assert!(bad.parse::<Rgba>().is_err(), "{bad}");
        }
    }

    #[test]
    fn display_round_trips() {
        let c = Rgba::rgb(0x18, 0x18, 0x1b);
        assert_eq!(c.to_string(), "#18181b");
        assert_eq!(c.to_string().parse::<Rgba>().unwrap(), c);
        let c = c.with_alpha(0x80);
        assert_eq!(c.to_string(), "#18181b80");
        assert_eq!(c.to_string().parse::<Rgba>().unwrap(), c);
    }

    #[test]
    fn to_f64_range() {
        assert_eq!(Rgba::rgb(0, 255, 0).to_f64(), (0.0, 1.0, 0.0, 1.0));
    }
}
