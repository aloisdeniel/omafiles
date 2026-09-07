//! The palette's hues, by name.
//!
//! Every Omarchy theme carries the six terminal hues beside its roles, and
//! most carry `orange` too. They are what lets a window say *what kind* of
//! thing something is — a directory, a picture, an archive, a fresh change —
//! without writing a colour down: the hue is the theme's, so a listing on
//! `gruvbox` is warm and one on `nord` is cool, and both are legible.
//!
//! Read them through [`Theme::hue`](crate::Theme::hue), never straight from
//! the palette: a hue is held to the secondary contrast floor on whatever it
//! sits on, since a theme's yellow is tuned for its terminal and not for a
//! caption on a panel.

use omarchy_tokens::{Palette, Rgb};

/// A named hue from the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hue {
    Red,
    /// Not every theme defines it; falls back to yellow where missing.
    Orange,
    Yellow,
    Green,
    Cyan,
    Blue,
    Magenta,
}

impl Hue {
    pub const ALL: [Hue; 7] = [
        Hue::Red,
        Hue::Orange,
        Hue::Yellow,
        Hue::Green,
        Hue::Cyan,
        Hue::Blue,
        Hue::Magenta,
    ];

    /// The `colors.toml` key.
    pub fn key(self) -> &'static str {
        match self {
            Hue::Red => "red",
            Hue::Orange => "orange",
            Hue::Yellow => "yellow",
            Hue::Green => "green",
            Hue::Cyan => "cyan",
            Hue::Blue => "blue",
            Hue::Magenta => "magenta",
        }
    }

    /// The hue as the palette has it, with no contrast floor. Three stock
    /// themes omit `orange`; there it is the yellow, the nearest the theme
    /// offers.
    pub fn resolve(self, palette: &Palette) -> Rgb {
        match self {
            Hue::Orange => palette
                .try_get("orange")
                .unwrap_or_else(|| palette.yellow()),
            other => palette.get(other.key()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette(extra: &str) -> Palette {
        Palette::from_toml_str(&format!(
            "background = \"#1a1b26\"\nforeground = \"#a9b1d6\"\naccent = \"#7aa2f7\"\n\
             red = \"#f7768e\"\nyellow = \"#e0af68\"\ngreen = \"#9ece6a\"\ncyan = \"#449dab\"\n\
             blue = \"#7aa2f7\"\nmagenta = \"#ad8ee6\"\nmuted = \"#414868\"\n{extra}"
        ))
        .unwrap()
    }

    #[test]
    fn every_hue_resolves_to_its_key() {
        let p = palette("orange = \"#ff9e64\"\n");
        for hue in Hue::ALL {
            assert_eq!(hue.resolve(&p), p.get(hue.key()), "{hue:?}");
        }
    }

    #[test]
    fn a_missing_orange_is_the_yellow() {
        let p = palette("");
        assert_eq!(Hue::Orange.resolve(&p), p.yellow());
    }
}
