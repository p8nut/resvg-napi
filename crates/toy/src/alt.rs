//! A second module, mirrored into a namespace of its own (`alt`). Its
//! [`Color`] has the name of [`crate::Color`]: two classes of one JS name, in
//! two namespaces.

use std::str::FromStr;

/// A colour as plain data: every field public.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Color {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Color {
    pub fn new(red: u8, green: u8, blue: u8) -> Self {
        Color { red, green, blue }
    }

    pub fn black() -> Self {
        Color::default()
    }

    /// Pure red: a static named like a field, as upstream crates have.
    pub fn red() -> Self {
        Color::new(255, 0, 0)
    }
}

impl FromStr for Color {
    type Err = crate::ParseError;

    /// From `"r,g,b"`.
    fn from_str(s: &str) -> Result<Self, crate::ParseError> {
        if s.trim().is_empty() {
            return Err(crate::ParseError::Empty);
        }
        let mut parts = s.split(',');
        Ok(Color::new(
            crate::number(parts.next(), "red")?,
            crate::number(parts.next(), "green")?,
            crate::number(parts.next(), "blue")?,
        ))
    }
}

/// A colour by name, or any: an input enum's shape, taken only from the
/// crate root ([`crate::Tally::hue_name`]).
#[derive(Clone, Debug, PartialEq)]
pub enum Hue {
    Any,
    Named(String),
}
