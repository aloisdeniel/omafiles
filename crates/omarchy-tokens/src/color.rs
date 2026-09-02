use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Result, anyhow, bail};

/// An opaque sRGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Rgb = Rgb::new(0, 0, 0);
    pub const WHITE: Rgb = Rgb::new(255, 255, 255);

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// `0xRRGGBB`, the form gpui's `rgb()` helper takes.
    pub const fn to_u32(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Linear blend, matching `mix_color` in `omarchy-theme-set-templates`:
    /// `int(start * (1 - amount) + end * amount + 0.5)` per channel, i.e.
    /// round-half-up.
    pub fn mix(self, other: Rgb, amount: f32) -> Rgb {
        let amount = amount.clamp(0.0, 1.0);
        let channel =
            |a: u8, b: u8| -> u8 { (a as f32 * (1.0 - amount) + b as f32 * amount + 0.5) as u8 };
        Rgb::new(
            channel(self.r, other.r),
            channel(self.g, other.g),
            channel(self.b, other.b),
        )
    }

    /// Relative luminance (WCAG).
    pub fn luminance(self) -> f32 {
        fn channel(v: u8) -> f32 {
            let v = v as f32 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b)
    }
}

impl FromStr for Rgb {
    type Err = anyhow::Error;

    /// Accepts `#rgb` and `#rrggbb`, case-insensitively. Theme files are
    /// inconsistent about casing (`#060B1E` in `ethereal`, lowercase
    /// elsewhere), so this must not care.
    fn from_str(s: &str) -> Result<Self> {
        let hex = s.trim().strip_prefix('#').unwrap_or(s.trim());

        let nibble = |c: char| -> Result<u8> {
            c.to_digit(16)
                .map(|d| d as u8)
                .ok_or_else(|| anyhow!("not a hex digit: {c:?}"))
        };

        let chars: Vec<char> = hex.chars().collect();
        match chars.len() {
            3 => Ok(Rgb::new(
                nibble(chars[0])? * 17,
                nibble(chars[1])? * 17,
                nibble(chars[2])? * 17,
            )),
            6 => {
                let byte =
                    |hi: char, lo: char| -> Result<u8> { Ok(nibble(hi)? * 16 + nibble(lo)?) };
                Ok(Rgb::new(
                    byte(chars[0], chars[1])?,
                    byte(chars[2], chars[3])?,
                    byte(chars[4], chars[5])?,
                ))
            }
            n => bail!("expected 3 or 6 hex digits, got {n} in {s:?}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

impl Mode {
    pub fn is_dark(self) -> bool {
        matches!(self, Mode::Dark)
    }
}

/// A resolved Omarchy palette.
///
/// Themes do not all define the same keys — three of the 22 stock themes omit
/// `orange` and `brown` entirely — so this mirrors `omarchy-theme-color`'s
/// derivation chain rather than requiring a fixed shape. Unknown keys are kept
/// rather than rejected: a future Omarchy release adding a colour must not stop
/// the app from starting.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    mode: Mode,
    colors: HashMap<String, Rgb>,
}

/// Accessor for a key the derivation chain guarantees exists.
macro_rules! role {
    ($($name:ident),* $(,)?) => {
        $(
            pub fn $name(&self) -> Rgb {
                self.get(stringify!($name))
            }
        )*
    };
}

impl Palette {
    pub fn from_toml_file(path: &Path) -> Result<Self> {
        Self::from_toml_str(&std::fs::read_to_string(path)?)
    }

    pub fn from_toml_str(text: &str) -> Result<Self> {
        let raw: HashMap<String, toml::Value> = toml::from_str(text)?;

        let mode = match raw.get("mode").and_then(|v| v.as_str()) {
            Some(s) if s.trim().eq_ignore_ascii_case("light") => Mode::Light,
            _ => Mode::Dark,
        };

        // Anything that is not a parseable colour is dropped rather than
        // failing the load — `mode` is a string, and a future release may add
        // non-colour keys.
        let mut colors: HashMap<String, Rgb> = HashMap::new();
        for (key, value) in &raw {
            if let Some(Ok(rgb)) = value.as_str().map(str::parse::<Rgb>) {
                colors.insert(key.clone(), rgb);
            }
        }

        if !colors.contains_key("background") || !colors.contains_key("foreground") {
            bail!("palette must define at least `background` and `foreground`");
        }

        let mut palette = Self { mode, colors };
        palette.derive();
        Ok(palette)
    }

    /// Fill in every key a theme may omit.
    ///
    /// Ported from `omarchy-theme-color`, **in its order** — several rules feed
    /// each other (`brown` reads the `orange` that the previous line may have
    /// just derived), so reordering silently changes results.
    fn derive(&mut self) {
        // Legacy ANSI names, when a theme supplies those instead of semantic ones.
        self.alias_from("light_foreground", &["color7", "foreground"]);
        self.alias_from("bright_foreground", &["color15", "foreground"]);
        self.set_from("cursor", "bright_foreground");
        self.alias_from("lighter_background", &["color0", "background"]);
        self.alias_from("dark_foreground", &["color8", "foreground"]);
        self.alias_from("muted", &["color8", "dark_foreground"]);
        self.alias_from(
            "selection",
            &["selection_background", "color8", "color0", "background"],
        );
        self.alias_from("selection_background", &["selection"]);
        self.alias_from("selection_foreground", &["bright_foreground"]);

        self.alias_from("orange", &["yellow"]);
        self.mix_from("brown", "orange", Rgb::BLACK, 0.50);

        self.mix_from("dark_background", "background", Rgb::BLACK, 0.25);
        self.mix_from("darker_background", "background", Rgb::BLACK, 0.50);

        for base in ["red", "yellow", "green", "cyan", "blue", "magenta"] {
            self.mix_from(&format!("bright_{base}"), base, Rgb::WHITE, 0.20);
        }

        // Short aliases the rest of Omarchy uses interchangeably.
        for (alias, source) in [
            ("bg", "background"),
            ("fg", "foreground"),
            ("dark_bg", "dark_background"),
            ("darker_bg", "darker_background"),
            ("lighter_bg", "lighter_background"),
            ("dark_fg", "dark_foreground"),
            ("light_fg", "light_foreground"),
            ("bright_fg", "bright_foreground"),
            ("purple", "magenta"),
            ("bright_purple", "bright_magenta"),
        ] {
            self.set_from(alias, source);
        }
    }

    /// Set `key` to the first of `sources` that exists, if `key` is absent.
    fn alias_from(&mut self, key: &str, sources: &[&str]) {
        if self.colors.contains_key(key) {
            return;
        }
        for source in sources {
            if let Some(&value) = self.colors.get(*source) {
                self.colors.insert(key.to_string(), value);
                return;
            }
        }
    }

    /// Unconditionally mirror `source` onto `key`.
    fn set_from(&mut self, key: &str, source: &str) {
        if let Some(&value) = self.colors.get(source) {
            self.colors.insert(key.to_string(), value);
        }
    }

    /// Set `key` to `mix(source, toward, amount)`, if `key` is absent.
    fn mix_from(&mut self, key: &str, source: &str, toward: Rgb, amount: f32) {
        if self.colors.contains_key(key) {
            return;
        }
        if let Some(&base) = self.colors.get(source) {
            self.colors
                .insert(key.to_string(), base.mix(toward, amount));
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Look up any key, including ones without a named accessor.
    pub fn try_get(&self, key: &str) -> Option<Rgb> {
        self.colors.get(key).copied()
    }

    /// Look up a key the derivation chain guarantees. Falls back to `foreground`
    /// rather than panicking — a missing colour must never take down a running
    /// desktop app.
    pub fn get(&self, key: &str) -> Rgb {
        self.try_get(key).unwrap_or_else(|| {
            self.colors
                .get("foreground")
                .copied()
                .unwrap_or(Rgb::new(0xff, 0x00, 0xff))
        })
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.colors.keys().map(String::as_str)
    }

    role!(
        accent,
        selection,
        muted,
        background,
        dark_background,
        darker_background,
        lighter_background,
        foreground,
        dark_foreground,
        light_foreground,
        bright_foreground,
        red,
        yellow,
        orange,
        green,
        cyan,
        blue,
        magenta,
        brown,
    );

    /// `urgent` is a first-class role in `Style.qml` but has no key of its own;
    /// `Color.qml` sources it from `red`. Not guessable, so it is spelled out.
    pub fn urgent(&self) -> Rgb {
        self.red()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_six_digit_hex_in_either_case() {
        assert_eq!(
            "#1a1b26".parse::<Rgb>().unwrap(),
            Rgb::new(0x1a, 0x1b, 0x26)
        );
        assert_eq!(
            "#060B1E".parse::<Rgb>().unwrap(),
            Rgb::new(0x06, 0x0b, 0x1e)
        );
        assert_eq!(
            "#FFFCF0".parse::<Rgb>().unwrap(),
            Rgb::new(0xff, 0xfc, 0xf0)
        );
    }

    #[test]
    fn parses_three_digit_hex() {
        assert_eq!("#fff".parse::<Rgb>().unwrap(), Rgb::new(255, 255, 255));
        assert_eq!("#08f".parse::<Rgb>().unwrap(), Rgb::new(0x00, 0x88, 0xff));
    }

    #[test]
    fn rejects_malformed_hex() {
        assert!("#12345".parse::<Rgb>().is_err());
        assert!("#gggggg".parse::<Rgb>().is_err());
        assert!("".parse::<Rgb>().is_err());
    }

    #[test]
    fn converts_to_the_u32_form_gpui_wants() {
        assert_eq!("#1a1b26".parse::<Rgb>().unwrap().to_u32(), 0x1a1b26);
    }

    #[test]
    fn mix_matches_omarchys_rounding() {
        // `white`'s brown is mix(orange #4a4a4a, black, 50%) = #252525.
        let orange: Rgb = "#4a4a4a".parse().unwrap();
        assert_eq!(orange.mix(Rgb::BLACK, 0.5).to_hex(), "#252525");

        // Endpoints are exact.
        let c: Rgb = "#7aa2f7".parse().unwrap();
        assert_eq!(c.mix(Rgb::WHITE, 0.0), c);
        assert_eq!(c.mix(Rgb::BLACK, 1.0), Rgb::BLACK);
    }

    // Note the `##` delimiter: the palette values contain `"#`, which would
    // close a plain `r#"…"#` string on the first colour.
    const TOKYO_NIGHT: &str = r##"
mode = "dark"
accent = "#7aa2f7"
selection = "#292e42"
muted = "#414868"
background = "#1a1b26"
dark_background = "#13141c"
darker_background = "#0e0e14"
lighter_background = "#24283b"
foreground = "#a9b1d6"
dark_foreground = "#565f89"
light_foreground = "#b4bee6"
bright_foreground = "#c0caf5"
red = "#f7768e"
yellow = "#e0af68"
orange = "#eb927b"
green = "#9ece6a"
cyan = "#449dab"
blue = "#7aa2f7"
magenta = "#ad8ee6"
brown = "#75493d"
bright_red = "#ff7a93"
bright_yellow = "#ff9e64"
bright_green = "#b9f27c"
bright_cyan = "#0db9d7"
bright_blue = "#7da6ff"
bright_magenta = "#bb9af7"
"##;

    #[test]
    fn parses_a_real_theme() {
        let palette = Palette::from_toml_str(TOKYO_NIGHT).unwrap();
        assert_eq!(palette.mode(), Mode::Dark);
        assert_eq!(palette.background(), Rgb::new(0x1a, 0x1b, 0x26));
        assert_eq!(palette.accent(), Rgb::new(0x7a, 0xa2, 0xf7));
    }

    #[test]
    fn explicit_values_are_never_overwritten_by_derivation() {
        let palette = Palette::from_toml_str(TOKYO_NIGHT).unwrap();
        // orange is defined and differs from yellow; brown is defined and is
        // not mix(orange, black, 50%).
        assert_eq!(palette.orange().to_hex(), "#eb927b");
        assert_ne!(palette.orange(), palette.yellow());
        assert_eq!(palette.brown().to_hex(), "#75493d");
    }

    #[test]
    fn urgent_resolves_to_red() {
        let palette = Palette::from_toml_str(TOKYO_NIGHT).unwrap();
        assert_eq!(palette.urgent(), palette.red());
    }

    #[test]
    fn light_mode_is_detected() {
        let light = TOKYO_NIGHT.replace(r#"mode = "dark""#, r#"mode = "light""#);
        assert_eq!(Palette::from_toml_str(&light).unwrap().mode(), Mode::Light);
    }

    /// Regression: `white`, `solitude` and `last-horizon` ship no `orange` and
    /// no `brown`. Requiring them made the whole palette fail to load, and the
    /// app silently fell back to a built-in theme.
    #[test]
    fn derives_orange_and_brown_when_a_theme_omits_them() {
        let text = r##"
mode = "light"
accent = "#6e6e6e"
selection = "#e5e5e5"
muted = "#8a8a8a"
background = "#ffffff"
dark_background = "#f5f5f5"
darker_background = "#ebebeb"
lighter_background = "#fafafa"
foreground = "#2a2a2a"
dark_foreground = "#6e6e6e"
light_foreground = "#4a4a4a"
bright_foreground = "#000000"
red = "#2a2a2a"
yellow = "#4a4a4a"
green = "#3a3a3a"
cyan = "#5a5a5a"
blue = "#3a3a3a"
magenta = "#4a4a4a"
"##;
        let palette = Palette::from_toml_str(text).unwrap();
        assert_eq!(
            palette.orange(),
            palette.yellow(),
            "orange falls back to yellow"
        );
        assert_eq!(
            palette.brown().to_hex(),
            "#252525",
            "brown = mix(orange, black, 50%)"
        );
    }

    #[test]
    fn derives_bright_variants_and_background_shades() {
        let minimal = r##"
mode = "dark"
background = "#202020"
foreground = "#c0c0c0"
accent = "#7aa2f7"
red = "#f00000"
yellow = "#e0af68"
green = "#00f000"
cyan = "#00f0f0"
blue = "#0000f0"
magenta = "#f000f0"
"##;
        let p = Palette::from_toml_str(minimal).unwrap();
        assert_eq!(p.dark_background().to_hex(), "#181818"); // mix(bg, black, 25%)
        assert_eq!(p.darker_background().to_hex(), "#101010"); // mix(bg, black, 50%)
        assert_eq!(
            p.try_get("bright_red").unwrap(),
            p.red().mix(Rgb::WHITE, 0.20)
        );
        // Absent optional keys still resolve rather than blowing up.
        assert_eq!(p.lighter_background(), p.background());
        assert_eq!(p.bright_foreground(), p.foreground());
    }

    #[test]
    fn keeps_unknown_keys_instead_of_rejecting_them() {
        let text = format!("{TOKYO_NIGHT}\nsome_future_color = \"#123456\"\n");
        let palette = Palette::from_toml_str(&text).unwrap();
        assert_eq!(
            palette.try_get("some_future_color").unwrap().to_hex(),
            "#123456"
        );
    }

    #[test]
    fn requires_only_background_and_foreground() {
        assert!(Palette::from_toml_str("mode = \"dark\"\nbackground = \"#000\"\n").is_err());
        assert!(
            Palette::from_toml_str("background = \"#000000\"\nforeground = \"#ffffff\"\n").is_ok()
        );
    }

    #[test]
    fn luminance_orders_the_corpus_extremes() {
        let black: Rgb = "#000000".parse().unwrap(); // vantablack
        let white: Rgb = "#ffffff".parse().unwrap(); // white
        assert!(black.luminance() < 0.01);
        assert!(white.luminance() > 0.99);
    }
}
