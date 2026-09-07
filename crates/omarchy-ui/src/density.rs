//! How much room the UI takes.
//!
//! The shell's tokens describe one size: the bar's, the menu's. An app with
//! a window of its own can afford more — a listing read for an hour is
//! easier at a step up from the bar's density — so the [`Theme`](crate::Theme)
//! carries a [`Density`] and sizes its tokens through it. Every component
//! reads the sized tokens, so a density is one switch rather than a second
//! set of numbers in each of them.

use omarchy_tokens::Tokens;

/// The two sizes the UI comes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Density {
    /// The shell's own scale: what the bar and the menu use, and what the
    /// tokens describe untouched. The default.
    #[default]
    Compact,
    /// A step up: larger type, roomier spacing, taller bars and rows, and
    /// icons a size above the text rather than at it.
    Normal,
}

impl Density {
    pub const ALL: [Density; 2] = [Density::Compact, Density::Normal];

    /// The other one.
    pub fn toggled(self) -> Self {
        match self {
            Self::Compact => Self::Normal,
            Self::Normal => Self::Compact,
        }
    }

    /// The name a settings file or a notice uses.
    pub fn name(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Normal => "normal",
        }
    }

    /// How much larger the type is than the shell's. A step, not a jump:
    /// the text should read as a little bigger, and the space around it as
    /// a lot.
    pub fn font_factor(self) -> f32 {
        match self {
            Self::Compact => 1.0,
            Self::Normal => 1.15,
        }
    }

    /// How much wider the spacing is than the shell's. Rows, bars, insets
    /// and gaps all follow, so `control-height` goes from 28 to 38 at the
    /// default text size and a bar from 36 to 48.
    pub fn space_factor(self) -> f32 {
        match self {
            Self::Compact => 1.0,
            Self::Normal => 1.35,
        }
    }

    /// The shell's tokens, sized for this density. Compact returns them as
    /// they are.
    pub fn apply(self, tokens: &Tokens) -> Tokens {
        match self {
            Self::Compact => tokens.clone(),
            Self::Normal => tokens.scaled(self.font_factor(), self.space_factor()),
        }
    }
}

impl std::str::FromStr for Density {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.trim().to_ascii_lowercase().as_str() {
            "compact" => Ok(Self::Compact),
            "normal" => Ok(Self::Normal),
            other => Err(format!("unknown density {other:?}: compact or normal")),
        }
    }
}

impl std::fmt::Display for Density {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_is_the_default_and_leaves_the_tokens_alone() {
        let tokens = crate::fallback_tokens();
        assert_eq!(Density::default(), Density::Compact);
        assert_eq!(Density::Compact.apply(&tokens), tokens);
    }

    #[test]
    fn normal_grows_type_and_spacing_but_not_colour() {
        let tokens = crate::fallback_tokens();
        let normal = Density::Normal.apply(&tokens);
        assert!(normal.typography.body() > tokens.typography.body());
        assert!(normal.spacing.control_height() > tokens.spacing.control_height());
        assert_eq!(normal.palette, tokens.palette);
        assert_eq!(normal.controls, tokens.controls);
        // The documented numbers at the default text size.
        assert_eq!(tokens.spacing.control_height(), 28.0);
        assert_eq!(normal.spacing.control_height(), 38.0);
        assert_eq!(normal.typography.body(), 14.0);
    }

    #[test]
    fn toggling_twice_is_the_identity() {
        for density in Density::ALL {
            assert_eq!(density.toggled().toggled(), density);
        }
    }

    #[test]
    fn names_round_trip() {
        for density in Density::ALL {
            assert_eq!(density.name().parse::<Density>(), Ok(density));
        }
        assert!("roomy".parse::<Density>().is_err());
        assert_eq!(" Normal ".parse::<Density>(), Ok(Density::Normal));
    }
}
