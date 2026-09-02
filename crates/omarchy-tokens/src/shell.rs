//! The `shell.toml` structural tokens — typography, spacing, interaction
//! states and per-surface colours.
//!
//! A faithful port of `omarchy-shell`'s `Commons/Style.qml` and
//! `Commons/Color.qml`. The multipliers, defaults and — importantly — the
//! *order* of resolution are theirs, not ours. An app that computes these
//! differently drifts out of step with the bar and the menu.
//!
//! Two files feed this, in order: the theme's generated
//! `<theme>/shell.toml`, then the machine-level `~/.config/omarchy/shell.toml`
//! on top. **User keys win.**

use std::collections::HashMap;
use std::path::Path;

use crate::color::{Palette, Rgb};

/// Omarchy's anchor: 12px shell base ≡ GTK text-scaling 1.0 ≡ 9pt terminal.
pub const DEFAULT_BASE_SIZE: f32 = 12.0;

/// A flattened `shell.toml`, keyed `"section.key"` exactly as `Style.qml`
/// consumes it.
///
/// Kept as strings rather than a typed struct: the file has a dozen sections
/// with cross-references between them, and a future Omarchy release adding a
/// key must not stop the app from starting.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShellValues {
    values: HashMap<String, String>,
}

impl ShellValues {
    /// Parse one `shell.toml`. Nested tables are flattened to `section.key`.
    pub fn from_toml_str(text: &str) -> Self {
        let mut values = HashMap::new();

        if let Ok(toml::Value::Table(table)) = toml::from_str::<toml::Value>(text) {
            for (section, body) in table {
                match body {
                    toml::Value::Table(inner) => {
                        for (key, value) in inner {
                            if let Some(text) = scalar_to_string(&value) {
                                values.insert(format!("{section}.{key}"), text);
                            }
                        }
                    }
                    // Top-level scalars have no section; Style.qml ignores keys
                    // without a dot, so store them under a name that can never
                    // collide with a real section.
                    other => {
                        if let Some(text) = scalar_to_string(&other) {
                            values.insert(format!(".{section}"), text);
                        }
                    }
                }
            }
        }

        Self { values }
    }

    pub fn from_toml_file(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .map(|text| Self::from_toml_str(&text))
            .unwrap_or_default()
    }

    /// Layer `other` on top of `self`. Used to put the user's
    /// `~/.config/omarchy/shell.toml` over the theme's generated one.
    pub fn overlay(mut self, other: ShellValues) -> Self {
        self.values.extend(other.values);
        self
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    fn number(&self, key: &str) -> Option<f32> {
        self.get(key)?.trim().parse().ok()
    }

    /// `Style.qml`'s `boolToken`.
    fn boolean(&self, key: &str, fallback: bool) -> bool {
        match self.get(key).map(|s| s.trim().to_ascii_lowercase()) {
            Some(s) => match s.as_str() {
                "true" | "1" | "yes" | "on" => true,
                "false" | "0" | "no" | "off" => false,
                _ => fallback,
            },
            None => fallback,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.values.keys().map(String::as_str)
    }
}

fn scalar_to_string(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Integer(i) => Some(i.to_string()),
        toml::Value::Float(f) => Some(f.to_string()),
        toml::Value::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

// ---------------------------------------------------------------- typography

/// The type scale. `base-size` is the rem root; every token derives from it
/// unless the theme pins that specific one.
#[derive(Debug, Clone, PartialEq)]
pub struct Typography {
    pub family: String,
    pub base_size: f32,
    overrides: HashMap<String, f32>,
}

impl Typography {
    pub fn new(family: String, values: &ShellValues) -> Self {
        let base_size = values
            .number("font.base-size")
            .unwrap_or(DEFAULT_BASE_SIZE)
            .max(1.0);

        // Any [font] key other than base-size pins a single token.
        let overrides = [
            "caption",
            "body-small",
            "body",
            "subtitle",
            "title",
            "heading",
            "display",
            "display-large",
            "icon-small",
            "icon",
            "icon-large",
        ]
        .into_iter()
        .filter_map(|key| {
            let value = values.number(&format!("font.{key}"))?;
            (value > 0.0).then(|| (key.to_string(), value.round()))
        })
        .collect();

        Self {
            family,
            base_size,
            overrides,
        }
    }

    /// `Style.qml`: `fontScale = max(1/12, base_size / 12)`.
    pub fn scale(&self) -> f32 {
        (self.base_size / DEFAULT_BASE_SIZE).max(1.0 / DEFAULT_BASE_SIZE)
    }

    /// `Style.qml`: `fontPx(m) = max(1, round(base_size * m))`.
    pub fn px(&self, multiplier: f32) -> f32 {
        (self.base_size * multiplier).round().max(1.0)
    }

    /// A pinned override wins; otherwise derive from the scale.
    fn token(&self, key: &str, fallback: f32) -> f32 {
        self.overrides.get(key).copied().unwrap_or(fallback)
    }

    pub fn caption(&self) -> f32 {
        self.token("caption", self.px(0.833))
    }
    pub fn body_small(&self) -> f32 {
        self.token("body-small", self.px(0.917))
    }
    pub fn body(&self) -> f32 {
        self.token("body", self.px(1.0))
    }
    pub fn subtitle(&self) -> f32 {
        self.token("subtitle", self.px(1.083))
    }
    pub fn title(&self) -> f32 {
        self.token("title", self.px(1.167))
    }
    pub fn heading(&self) -> f32 {
        self.token("heading", self.px(1.333))
    }
    pub fn display(&self) -> f32 {
        self.token("display", self.px(2.0))
    }
    pub fn display_large(&self) -> f32 {
        self.token("display-large", self.px(2.333))
    }

    /// Icon sizes default to *other tokens*, not to multipliers.
    pub fn icon_small(&self) -> f32 {
        self.token("icon-small", self.body_small())
    }
    pub fn icon(&self) -> f32 {
        self.token("icon", self.title())
    }
    pub fn icon_large(&self) -> f32 {
        self.token("icon-large", self.px(1.5))
    }
}

// ------------------------------------------------------------------- spacing

/// The spacing scale.
///
/// Note the asymmetry ported from `Style.qml`'s `spacingToken`: an explicit
/// override is used **raw**, while a default is multiplied by the scale.
#[derive(Debug, Clone, PartialEq)]
pub struct Spacing {
    scale: f32,
    overrides: HashMap<String, f32>,
}

/// `(token name, Style.qml default)`.
const SPACING_DEFAULTS: &[(&str, f32)] = &[
    ("xxs", 2.0),
    ("xs", 3.0),
    ("sm", 4.0),
    ("md", 6.0),
    ("lg", 8.0),
    ("xl", 10.0),
    ("xxl", 12.0),
    ("xxxl", 14.0),
    ("huge", 18.0),
    ("control-gap", 8.0),
    ("control-padding-x", 10.0),
    ("control-padding-y", 6.0),
    ("input-padding-y", 7.0),
    ("control-height", 28.0),
    ("popup-row-height", 28.0),
    ("row-gap", 8.0),
    ("row-padding-x", 12.0),
    ("label-gap", 4.0),
    ("panel-gap", 14.0),
    ("panel-padding", 18.0),
    ("popup-padding", 14.0),
    ("dropdown-width", 240.0),
    ("searchable-dropdown-width", 260.0),
    ("number-field-width", 120.0),
    ("searchable-popup-min-height", 220.0),
];

impl Spacing {
    pub fn new(values: &ShellValues, typography: &Typography) -> Self {
        let base = values.number("spacing.scale").unwrap_or(1.0);
        let with_font = values.boolean("spacing.scale-with-font", true);

        let scale = base * if with_font { typography.scale() } else { 1.0 };

        let overrides = SPACING_DEFAULTS
            .iter()
            .filter_map(|(key, _)| {
                let value = values.number(&format!("spacing.{key}"))?;
                (value >= 0.0).then(|| (key.to_string(), value.round()))
            })
            .collect();

        Self { scale, overrides }
    }

    /// The effective multiplier: `spacing.scale × (scale-with-font ? fontScale : 1)`.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// `Style.qml`'s `space(px)`.
    pub fn space(&self, px: f32) -> f32 {
        if px <= 0.0 {
            return 0.0;
        }
        (px * self.scale).round().max(1.0)
    }

    /// `Style.qml`'s `spacingToken`. An override is raw; a default is scaled.
    pub fn get(&self, key: &str) -> f32 {
        if let Some(&value) = self.overrides.get(key) {
            return value;
        }
        let default = SPACING_DEFAULTS
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| *value)
            .unwrap_or(0.0);
        self.space(default)
    }

    /// 1px at the current scale.
    pub fn hairline(&self) -> f32 {
        self.space(1.0)
    }

    pub fn xxs(&self) -> f32 {
        self.get("xxs")
    }
    pub fn xs(&self) -> f32 {
        self.get("xs")
    }
    pub fn sm(&self) -> f32 {
        self.get("sm")
    }
    pub fn md(&self) -> f32 {
        self.get("md")
    }
    pub fn lg(&self) -> f32 {
        self.get("lg")
    }
    pub fn xl(&self) -> f32 {
        self.get("xl")
    }
    pub fn xxl(&self) -> f32 {
        self.get("xxl")
    }
    pub fn xxxl(&self) -> f32 {
        self.get("xxxl")
    }
    pub fn huge(&self) -> f32 {
        self.get("huge")
    }
    pub fn control_gap(&self) -> f32 {
        self.get("control-gap")
    }
    pub fn control_padding_x(&self) -> f32 {
        self.get("control-padding-x")
    }
    pub fn control_padding_y(&self) -> f32 {
        self.get("control-padding-y")
    }
    pub fn input_padding_y(&self) -> f32 {
        self.get("input-padding-y")
    }
    pub fn control_height(&self) -> f32 {
        self.get("control-height")
    }
    pub fn popup_row_height(&self) -> f32 {
        self.get("popup-row-height")
    }
    pub fn row_gap(&self) -> f32 {
        self.get("row-gap")
    }
    pub fn row_padding_x(&self) -> f32 {
        self.get("row-padding-x")
    }
    pub fn label_gap(&self) -> f32 {
        self.get("label-gap")
    }
    pub fn panel_gap(&self) -> f32 {
        self.get("panel-gap")
    }
    pub fn panel_padding(&self) -> f32 {
        self.get("panel-padding")
    }
    pub fn popup_padding(&self) -> f32 {
        self.get("popup-padding")
    }

    // Widths for narrow floating panels. Present in Style.qml's token set;
    // accessors added when the detail pane needed one.
    pub fn dropdown_width(&self) -> f32 {
        self.get("dropdown-width")
    }
    pub fn searchable_dropdown_width(&self) -> f32 {
        self.get("searchable-dropdown-width")
    }
    pub fn number_field_width(&self) -> f32 {
        self.get("number-field-width")
    }
    pub fn searchable_popup_min_height(&self) -> f32 {
        self.get("searchable-popup-min-height")
    }
}

// ------------------------------------------------------------ role resolution

/// The five role names a `shell.toml` colour token may resolve to.
///
/// `urgent ← red` is the one that is not guessable: `Style.qml` treats `urgent`
/// as a first-class role but `colors.toml` has no such key.
pub fn resolve_role(name: &str, palette: &Palette) -> Option<Rgb> {
    match name.trim().to_ascii_lowercase().as_str() {
        "foreground" | "text" => Some(palette.foreground()),
        "background" => Some(palette.background()),
        "accent" => Some(palette.accent()),
        "urgent" => Some(palette.urgent()),
        "muted" => Some(palette.muted()),
        _ => None,
    }
}

/// `Color.qml`'s `flatColor`: resolve a token to one opaque colour.
///
/// Takes the first colour stop (so a Hyprland-style gradient collapses to its
/// start, which is what the shell itself does wherever it needs a flat
/// colour), then resolves roles, dotted cross-references, and literals.
pub fn flat_color(token: &str, values: &ShellValues, palette: &Palette, fallback: Rgb) -> Rgb {
    flat_color_inner(token, values, palette, fallback, 0)
}

fn flat_color_inner(
    token: &str,
    values: &ShellValues,
    palette: &Palette,
    fallback: Rgb,
    depth: u8,
) -> Rgb {
    // A malformed theme could otherwise point two keys at each other.
    const MAX_INDIRECTION: u8 = 8;
    if depth > MAX_INDIRECTION {
        return fallback;
    }

    let Some(first) = first_color_token(token) else {
        return fallback;
    };
    let lowered = first.to_ascii_lowercase();

    if lowered == "transparent" {
        return fallback;
    }

    // A dotted cross-reference, e.g. `border = "hyprland.active-border"`.
    if let Some(referenced) = values.get(&lowered).filter(|r| **r != first) {
        return flat_color_inner(referenced, values, palette, fallback, depth + 1);
    }

    if let Some(role) = resolve_role(&lowered, palette) {
        return role;
    }

    parse_color(&first).unwrap_or(fallback)
}

/// The first colour-ish word in a value. Gradients look like
/// `rgba(...) rgba(...) 45deg`; an angle is not a colour.
fn first_color_token(spec: &str) -> Option<String> {
    spec.split_whitespace()
        .find(|part| {
            !part
                .trim_end_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-')
                .is_empty()
                || !part.ends_with("deg")
        })
        .filter(|part| !part.ends_with("deg"))
        .map(|part| part.trim().to_string())
}

/// `#rrggbb`, `#rgb`, `rgb(...)`, `rgba(...)` in either hex or decimal form,
/// and `0xAARRGGBB`.
fn parse_color(text: &str) -> Option<Rgb> {
    let text = text.trim();

    if let Ok(rgb) = text.parse::<Rgb>() {
        return Some(rgb);
    }

    if let Some(hex) = text.strip_prefix("0x").filter(|h| h.len() == 8) {
        // Hyprland's 0xAARRGGBB — drop the alpha.
        return hex[2..].parse().ok();
    }

    let lowered = text.to_ascii_lowercase();
    let inner = lowered
        .strip_prefix("rgba(")
        .or_else(|| lowered.strip_prefix("rgb("))?
        .strip_suffix(')')?;

    if !inner.contains(',') {
        // rgba(1a1b26ff) — Hyprland's packed hex form.
        return inner.get(..6)?.parse().ok();
    }

    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    let channel = |i: usize| -> Option<u8> { parts.get(i)?.parse::<f32>().ok().map(clamp_channel) };
    Some(Rgb::new(channel(0)?, channel(1)?, channel(2)?))
}

fn clamp_channel(v: f32) -> u8 {
    v.clamp(0.0, 255.0) as u8
}

// ------------------------------------------------------------ control states

/// One interactive state: a colour, a fill alpha, a border colour, a border
/// width and a border alpha.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StateStyle {
    pub color: Rgb,
    pub fill_alpha: f32,
    pub border: Rgb,
    pub border_width: f32,
    pub border_alpha: f32,
}

/// The `[controls]` vocabulary.
///
/// `normal` is idle chrome, `hover_cursor` covers both mouse hover and the
/// keyboard cursor, `focus` is real focus (defaulting to `hover_cursor`),
/// `selected` is a persistent chosen state, and `pressed`/`selection` are
/// momentary fills.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlStates {
    pub normal: StateStyle,
    pub hover_cursor: StateStyle,
    pub focus: StateStyle,
    pub selected: StateStyle,
    /// `Style.qml` gives pressed only a colour and a fill alpha, and its colour
    /// defaults to `hover-cursor-color`.
    pub pressed_color: Rgb,
    pub pressed_fill_alpha: f32,
    pub selection_fill_alpha: f32,
}

impl ControlStates {
    pub fn new(values: &ShellValues, palette: &Palette) -> Self {
        let fg = palette.foreground();

        let color = |key: &str, fallback: Rgb| -> Rgb {
            match values.get(key) {
                Some(token) => flat_color(token, values, palette, fallback),
                None => fallback,
            }
        };
        let alpha = |key: &str, fallback: f32| -> f32 {
            values.number(key).unwrap_or(fallback).clamp(0.0, 1.0)
        };
        let width = |key: &str, fallback: f32| -> f32 {
            values.number(key).unwrap_or(fallback).round().max(0.0)
        };

        let normal = StateStyle {
            color: color("controls.normal-color", fg),
            fill_alpha: alpha("controls.normal-fill-alpha", 0.04),
            border: color("controls.normal-border", fg),
            border_width: width("controls.normal-border-width", 1.0),
            border_alpha: alpha("controls.normal-border-alpha", 0.40),
        };

        let hover_cursor = StateStyle {
            color: color("controls.hover-cursor-color", fg),
            fill_alpha: alpha("controls.hover-cursor-fill-alpha", 0.08),
            border: color("controls.hover-cursor-border", fg),
            border_width: width("controls.hover-cursor-border-width", normal.border_width),
            border_alpha: alpha("controls.hover-cursor-border-alpha", 0.25),
        };

        // Focus mirrors hover-cursor by default so mouse hover, keyboard cursor
        // and tab focus all read as the same state.
        let focus = StateStyle {
            color: color("controls.focus-color", hover_cursor.color),
            fill_alpha: alpha("controls.focus-fill-alpha", hover_cursor.fill_alpha),
            border: color("controls.focus-border", hover_cursor.border),
            border_width: width("controls.focus-border-width", hover_cursor.border_width),
            border_alpha: alpha("controls.focus-border-alpha", hover_cursor.border_alpha),
        };

        let selected = StateStyle {
            color: color("controls.selected-color", fg),
            fill_alpha: alpha("controls.selected-fill-alpha", 0.18),
            border: color("controls.selected-border", fg),
            border_width: width("controls.selected-border-width", 0.0),
            border_alpha: alpha("controls.selected-border-alpha", 1.0),
        };

        Self {
            pressed_color: color("controls.pressed-color", hover_cursor.color),
            normal,
            hover_cursor,
            focus,
            selected,
            pressed_fill_alpha: alpha("controls.pressed-fill-alpha", 0.22),
            selection_fill_alpha: alpha("controls.selection-fill-alpha", 0.35),
        }
    }
}

// ----------------------------------------------------------------- surfaces

/// A themed surface: background, text and border, each with its own alpha.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface {
    pub background: Rgb,
    pub background_alpha: f32,
    pub text: Rgb,
    pub border: Rgb,
    pub border_alpha: f32,
}

/// The per-surface sections we care about. `Color.qml` has more (`bar`,
/// `polkit`, `lock`, `image-picker`); these are the ones a file explorer maps
/// onto.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surfaces {
    pub popups: Surface,
    pub tooltip: Surface,
    pub menu: Surface,
    pub notifications: Surface,
}

impl Surfaces {
    pub fn new(values: &ShellValues, palette: &Palette) -> Self {
        Self {
            // Fallbacks ported verbatim from Color.qml rather than invented.
            popups: surface(values, palette, "popups", palette.accent(), 1.0),
            tooltip: surface(values, palette, "tooltip", palette.foreground(), 0.97),
            menu: surface(values, palette, "menu", palette.foreground(), 1.0),
            notifications: surface(values, palette, "notifications", palette.accent(), 1.0),
        }
    }
}

fn surface(
    values: &ShellValues,
    palette: &Palette,
    section: &str,
    border_fallback: Rgb,
    background_alpha_fallback: f32,
) -> Surface {
    let pick = |key: &str, fallback: Rgb| -> Rgb {
        match values.get(&format!("{section}.{key}")) {
            Some(token) => flat_color(token, values, palette, fallback),
            None => fallback,
        }
    };
    let alpha = |key: &str, fallback: f32| -> f32 {
        values
            .number(&format!("{section}.{key}"))
            .unwrap_or(fallback)
            .clamp(0.0, 1.0)
    };

    Surface {
        background: pick("background", palette.background()),
        background_alpha: alpha("background-alpha", background_alpha_fallback),
        text: pick("text", palette.foreground()),
        border: pick("border", border_fallback),
        border_alpha: alpha("border-alpha", 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> Palette {
        Palette::from_toml_str(
            "background = \"#1a1b26\"\n\
             foreground = \"#a9b1d6\"\n\
             accent = \"#7aa2f7\"\n\
             red = \"#f7768e\"\n\
             muted = \"#414868\"\n\
             yellow = \"#e0af68\"\n\
             green = \"#9ece6a\"\n\
             cyan = \"#449dab\"\n\
             blue = \"#7aa2f7\"\n\
             magenta = \"#ad8ee6\"\n",
        )
        .unwrap()
    }

    #[test]
    fn flattens_sections_to_dotted_keys() {
        let v = ShellValues::from_toml_str("[font]\nbase-size = 14\n\n[spacing]\nscale = 1.5\n");
        assert_eq!(v.get("font.base-size"), Some("14"));
        assert_eq!(v.get("spacing.scale"), Some("1.5"));
        assert_eq!(v.get("font.missing"), None);
    }

    #[test]
    fn user_values_overlay_theme_values() {
        let theme = ShellValues::from_toml_str("[font]\nbase-size = 12\ncaption = 9\n");
        let user = ShellValues::from_toml_str("[font]\nbase-size = 16\n");
        let merged = theme.overlay(user);

        assert_eq!(merged.get("font.base-size"), Some("16"), "user wins");
        assert_eq!(merged.get("font.caption"), Some("9"), "theme key survives");
    }

    #[test]
    fn type_scale_matches_style_qml_defaults() {
        let t = Typography::new("x".into(), &ShellValues::default());
        // The values Style.qml documents in its own trailing comments.
        assert_eq!(t.caption(), 10.0);
        assert_eq!(t.body_small(), 11.0);
        assert_eq!(t.body(), 12.0);
        assert_eq!(t.subtitle(), 13.0);
        assert_eq!(t.title(), 14.0);
        assert_eq!(t.heading(), 16.0);
        assert_eq!(t.display(), 24.0);
        assert_eq!(t.display_large(), 28.0);
        assert_eq!(t.icon_large(), 18.0);
    }

    #[test]
    fn icon_tokens_default_to_other_tokens_not_multipliers() {
        let t = Typography::new(
            "x".into(),
            &ShellValues::from_toml_str("[font]\nbase-size = 14\n"),
        );
        assert_eq!(t.icon_small(), t.body_small());
        assert_eq!(t.icon(), t.title());
    }

    #[test]
    fn a_pinned_font_token_bypasses_the_scale() {
        let v = ShellValues::from_toml_str("[font]\nbase-size = 20\nheading = 11\n");
        let t = Typography::new("x".into(), &v);
        assert_eq!(t.heading(), 11.0, "pinned value is used raw");
        assert_eq!(t.body(), 20.0, "other tokens still scale");
    }

    #[test]
    fn spacing_defaults_scale_with_the_font() {
        let v = ShellValues::from_toml_str("[font]\nbase-size = 14\n");
        let t = Typography::new("x".into(), &v);
        let s = Spacing::new(&v, &t);
        // round(8 * 14/12) = 9
        assert_eq!(s.lg(), 9.0);
        assert_eq!(s.control_height(), (28.0 * 14.0 / 12.0f32).round());
    }

    #[test]
    fn a_spacing_override_is_raw_but_a_default_is_scaled() {
        // The asymmetry in Style.qml's spacingToken — easy to get wrong.
        let v = ShellValues::from_toml_str("[font]\nbase-size = 14\n\n[spacing]\nlg = 8\n");
        let t = Typography::new("x".into(), &v);
        let s = Spacing::new(&v, &t);
        assert_eq!(s.lg(), 8.0, "explicit override is NOT scaled");
        assert_eq!(s.xl(), (10.0 * 14.0 / 12.0f32).round(), "default IS scaled");
    }

    #[test]
    fn spacing_can_opt_out_of_font_scaling() {
        let v = ShellValues::from_toml_str(
            "[font]\nbase-size = 24\n\n[spacing]\nscale = 1.0\nscale-with-font = false\n",
        );
        let t = Typography::new("x".into(), &v);
        let s = Spacing::new(&v, &t);
        assert_eq!(s.scale(), 1.0);
        assert_eq!(s.lg(), 8.0);
    }

    #[test]
    fn resolves_the_five_roles_including_urgent() {
        let p = palette();
        assert_eq!(resolve_role("foreground", &p), Some(p.foreground()));
        assert_eq!(resolve_role("text", &p), Some(p.foreground()));
        assert_eq!(resolve_role("accent", &p), Some(p.accent()));
        assert_eq!(resolve_role("urgent", &p), Some(p.red()), "urgent <- red");
        assert_eq!(resolve_role("muted", &p), Some(p.muted()));
        assert_eq!(resolve_role("nonsense", &p), None);
    }

    #[test]
    fn flat_color_follows_dotted_cross_references() {
        let v = ShellValues::from_toml_str(
            "[hyprland]\nactive-border = \"#7aa2f7\"\n\n[popups]\nborder = \"hyprland.active-border\"\n",
        );
        let p = palette();
        let token = v.get("popups.border").unwrap();
        assert_eq!(flat_color(token, &v, &p, Rgb::BLACK).to_hex(), "#7aa2f7");
    }

    #[test]
    fn flat_color_takes_the_first_stop_of_a_gradient() {
        let v = ShellValues::default();
        let p = palette();
        assert_eq!(
            flat_color("rgba(7aa2f7ff) rgba(bb9af7ff) 45deg", &v, &p, Rgb::BLACK).to_hex(),
            "#7aa2f7"
        );
    }

    #[test]
    fn flat_color_parses_hyprland_colour_forms() {
        let v = ShellValues::default();
        let p = palette();
        for (input, want) in [
            ("#7aa2f7", "#7aa2f7"),
            ("rgb(122,162,247)", "#7aa2f7"),
            ("rgba(122,162,247,0.5)", "#7aa2f7"),
            ("rgba(7aa2f7ff)", "#7aa2f7"),
            ("0xff7aa2f7", "#7aa2f7"),
        ] {
            assert_eq!(
                flat_color(input, &v, &p, Rgb::BLACK).to_hex(),
                want,
                "{input}"
            );
        }
    }

    #[test]
    fn flat_color_survives_a_reference_cycle() {
        let v = ShellValues::from_toml_str("[a]\nb = \"a.c\"\n\n[a]\nc = \"a.b\"\n");
        let p = palette();
        // Must terminate and fall back rather than recursing forever.
        assert_eq!(flat_color("a.b", &v, &p, Rgb::WHITE), Rgb::WHITE);
    }

    #[test]
    fn control_states_use_style_qml_defaults() {
        let c = ControlStates::new(&ShellValues::default(), &palette());
        assert_eq!(c.normal.fill_alpha, 0.04);
        assert_eq!(c.normal.border_alpha, 0.40);
        assert_eq!(c.normal.border_width, 1.0);
        assert_eq!(c.hover_cursor.fill_alpha, 0.08);
        assert_eq!(c.selected.fill_alpha, 0.18);
        assert_eq!(c.selected.border_width, 0.0);
        assert_eq!(c.pressed_fill_alpha, 0.22);
        assert_eq!(
            c.pressed_color, c.hover_cursor.color,
            "pressed colour <- hover"
        );
        assert_eq!(c.selection_fill_alpha, 0.35);
    }

    #[test]
    fn focus_mirrors_hover_unless_overridden() {
        let default = ControlStates::new(&ShellValues::default(), &palette());
        assert_eq!(default.focus.fill_alpha, default.hover_cursor.fill_alpha);

        let v = ShellValues::from_toml_str(
            "[controls]\nfocus-fill-alpha = 0.5\nfocus-color = \"accent\"\n",
        );
        let overridden = ControlStates::new(&v, &palette());
        assert_eq!(overridden.focus.fill_alpha, 0.5);
        assert_eq!(overridden.focus.color, palette().accent());
        assert_eq!(
            overridden.hover_cursor.fill_alpha, 0.08,
            "hover is untouched"
        );
    }

    #[test]
    fn surfaces_fall_back_to_palette_roles() {
        let s = Surfaces::new(&ShellValues::default(), &palette());
        assert_eq!(s.popups.background, palette().background());
        assert_eq!(s.popups.text, palette().foreground());
        assert_eq!(
            s.popups.border,
            palette().accent(),
            "popups border <- accent"
        );
        assert_eq!(
            s.tooltip.border,
            palette().foreground(),
            "tooltip <- foreground"
        );
        assert_eq!(s.tooltip.background_alpha, 0.97, "tooltip's legacy opacity");
    }

    #[test]
    fn unknown_sections_are_kept_not_rejected() {
        let v = ShellValues::from_toml_str("[future]\nsomething = \"#123456\"\n");
        assert_eq!(v.get("future.something"), Some("#123456"));
    }

    #[test]
    fn malformed_toml_yields_empty_rather_than_panicking() {
        assert!(ShellValues::from_toml_str("this is not [ toml").is_empty());
    }
}
