//! Syntax colours, derived from the Omarchy palette.
//!
//! §6.5 of `PLAN.md` calls this "the one place where taste is encoded": every
//! tree-sitter capture name maps onto a palette *role*, once, and 22 themes come
//! out looking deliberate with no per-theme work. A theme that redefines `green`
//! moves every string literal with it.
//!
//! Two consumers read the same table, which is the point of building it here
//! rather than in the app:
//!
//! - [`SyntaxPalette`] implements gpui-component's `HighlightStyleResolver`, so
//!   `SyntaxHighlighter::styles` colours a whole file with it.
//! - [`SyntaxPalette::highlight_theme`] renders the same table into the
//!   Zed-format JSON that gpui-component's `HighlightTheme` deserialises, which
//!   is what colours fenced code blocks inside a rendered markdown preview.
//!
//! One table, so a code file and the same code inside a README cannot drift.

use std::sync::Arc;

use gpui::{FontStyle, FontWeight, HighlightStyle};
use gpui_component::highlighter::HighlightTheme;
use gpui_component::input::HighlightStyleResolver;
use omarchy_tokens::{Palette, Rgb};

use crate::{color, contrast};

/// How a capture is emphasised beyond its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Emphasis {
    Plain,
    Italic,
    Bold,
}

/// Capture name → palette key → emphasis.
///
/// Palette keys are the ones in `colors.toml`, resolved through
/// [`Palette::get`], so this table reads as "a keyword is magenta" rather than
/// naming a Rust accessor. Ordering does not matter; lookup is by name with a
/// dotted fallback, so `string.special.symbol` finds `string.special`, then
/// `string`, before giving up.
///
/// The choices, briefly, because "why is a type yellow" is a fair question:
///
/// - **Structure is magenta.** Keywords are the skeleton of a file and want the
///   most saturated role. Omarchy's `magenta` is that on every stock theme.
/// - **Data is warm.** Strings green, numbers and constants orange — the
///   convention nearly every terminal palette already trains people on.
/// - **Names are cool.** Functions blue, types yellow, properties cyan. Callable
///   and nameable things stay distinct from the literals they act on.
/// - **Scaffolding recedes.** Punctuation and comments take `muted`, so the
///   shape of the code survives at a glance and the noise does not.
const CAPTURES: &[(&str, &str, Emphasis)] = &[
    // Structure.
    ("keyword", "magenta", Emphasis::Plain),
    ("operator", "cyan", Emphasis::Plain),
    ("label", "magenta", Emphasis::Plain),
    ("punctuation", "muted", Emphasis::Plain),
    ("punctuation.bracket", "muted", Emphasis::Plain),
    ("punctuation.delimiter", "muted", Emphasis::Plain),
    ("punctuation.special", "cyan", Emphasis::Plain),
    // A list marker is the only punctuation worth seeing in prose.
    ("punctuation.list_marker", "blue", Emphasis::Plain),
    // Data.
    ("string", "green", Emphasis::Plain),
    ("string.escape", "orange", Emphasis::Plain),
    ("string.regex", "orange", Emphasis::Plain),
    ("string.special", "orange", Emphasis::Plain),
    ("string.special.symbol", "cyan", Emphasis::Plain),
    ("number", "orange", Emphasis::Plain),
    ("boolean", "orange", Emphasis::Plain),
    ("constant", "orange", Emphasis::Plain),
    // Names.
    ("function", "blue", Emphasis::Plain),
    ("constructor", "blue", Emphasis::Plain),
    ("type", "yellow", Emphasis::Plain),
    ("enum", "yellow", Emphasis::Plain),
    ("variant", "yellow", Emphasis::Plain),
    ("property", "cyan", Emphasis::Plain),
    ("attribute", "yellow", Emphasis::Plain),
    ("variable", "foreground", Emphasis::Plain),
    // `self`, `this`, `super` — a variable, but not one you declared.
    ("variable.special", "red", Emphasis::Italic),
    ("primary", "foreground", Emphasis::Plain),
    // Markup and templating.
    ("tag", "red", Emphasis::Plain),
    ("tag.doctype", "muted", Emphasis::Plain),
    ("preproc", "brown", Emphasis::Plain),
    ("embedded", "brown", Emphasis::Plain),
    ("title", "bright_foreground", Emphasis::Bold),
    ("emphasis", "foreground", Emphasis::Italic),
    ("emphasis.strong", "bright_foreground", Emphasis::Bold),
    ("link_text", "cyan", Emphasis::Plain),
    ("link_uri", "blue", Emphasis::Plain),
    ("text.literal", "green", Emphasis::Plain),
    ("text.code.span", "green", Emphasis::Plain),
    // Scaffolding.
    ("comment", "muted", Emphasis::Italic),
    ("comment.doc", "muted", Emphasis::Italic),
    ("hint", "muted", Emphasis::Italic),
    ("predictive", "muted", Emphasis::Italic),
];

/// Syntax colours for one palette.
///
/// Cheap to clone and safe to share: it is a resolved table of colours, not a
/// parser or a cache.
#[derive(Debug, Clone)]
pub struct SyntaxPalette {
    /// Resolved in table order, so the rendered theme is deterministic.
    entries: Vec<(&'static str, Rgb, Emphasis)>,
    /// What code is drawn on, which is what every colour was floored against.
    background: Rgb,
    foreground: Rgb,
    dark: bool,
}

impl SyntaxPalette {
    /// Resolve [`CAPTURES`] against a palette.
    ///
    /// Every colour is held to [`contrast::MIN_SECONDARY_CONTRAST`] against the
    /// background it will be drawn on. That floor exists for the same reason it
    /// does elsewhere in the crate: it rescues only the pathological cases — a
    /// theme whose `muted` is byte-identical to its background renders comments
    /// invisible, which is worse than mildly off-palette — and leaves everything
    /// a theme author actually chose untouched.
    pub fn new(palette: &Palette) -> Self {
        let background = palette.background();
        let entries = CAPTURES
            .iter()
            .map(|(capture, key, emphasis)| {
                let ink = contrast::ensure_contrast(
                    palette.get(key),
                    background,
                    contrast::MIN_SECONDARY_CONTRAST,
                );
                (*capture, ink, *emphasis)
            })
            .collect();

        Self {
            entries,
            background,
            foreground: palette.foreground(),
            dark: palette.mode().is_dark(),
        }
    }

    /// The style for a capture, falling back along dotted segments.
    ///
    /// A grammar emits names this table has never heard of — `keyword.import`,
    /// `function.method.builtin`. Trimming after the last dot means those inherit
    /// their family's colour instead of coming out unstyled, which is why the
    /// table can stay short.
    pub fn style(&self, capture: &str) -> Option<HighlightStyle> {
        let mut name = capture;
        loop {
            if let Some((_, ink, emphasis)) = self.entries.iter().find(|(n, _, _)| *n == name) {
                return Some(HighlightStyle {
                    color: Some(color(*ink)),
                    font_weight: (*emphasis == Emphasis::Bold).then_some(FontWeight::BOLD),
                    font_style: (*emphasis == Emphasis::Italic).then_some(FontStyle::Italic),
                    ..Default::default()
                });
            }
            name = &name[..name.rfind('.')?];
        }
    }

    /// The same table as a gpui-component [`HighlightTheme`].
    ///
    /// Built by rendering Zed-format JSON and deserialising it, because
    /// `ThemeStyle`'s fields are private and that JSON is the type's documented
    /// input — `registry.rs` says so in as many words. Deserialisation cannot
    /// realistically fail (the shape is ours and every field is optional), but
    /// it returns a `Result`, and a preview that silently loses its colours is
    /// worse than one that says why.
    pub fn highlight_theme(&self) -> anyhow::Result<Arc<HighlightTheme>> {
        let syntax: Vec<String> = self
            .entries
            .iter()
            .map(|(capture, ink, emphasis)| {
                let style = match emphasis {
                    Emphasis::Plain => String::new(),
                    Emphasis::Italic => r#","font_style":"italic""#.to_string(),
                    Emphasis::Bold => r#","font_weight":700"#.to_string(),
                };
                format!(
                    r#""{capture}":{{"color":"{}"{style}}}"#,
                    ink.to_hex(),
                    style = style
                )
            })
            .collect();

        let appearance = if self.dark { "dark" } else { "light" };
        let json = format!(
            r#"{{"name":"omarchy","appearance":"{appearance}","style":{{"editor.background":"{bg}","editor.foreground":"{fg}","syntax":{{{syntax}}}}}}}"#,
            bg = self.background.to_hex(),
            fg = self.foreground.to_hex(),
            syntax = syntax.join(","),
        );

        Ok(Arc::new(serde_json::from_str::<HighlightTheme>(&json)?))
    }
}

/// So `SyntaxHighlighter::styles(&range, &palette)` takes it directly.
impl HighlightStyleResolver for SyntaxPalette {
    fn style(&self, name: &str) -> Option<HighlightStyle> {
        SyntaxPalette::style(self, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> Palette {
        crate::fallback_tokens().palette
    }

    #[test]
    fn every_capture_in_the_table_resolves() {
        let syntax = SyntaxPalette::new(&palette());
        for (capture, _, _) in CAPTURES {
            assert!(
                syntax.style(capture).is_some(),
                "{capture} is in the table but does not resolve"
            );
        }
    }

    #[test]
    fn unknown_captures_inherit_their_family() {
        let syntax = SyntaxPalette::new(&palette());
        // Grammars emit names this table has never heard of. They must take
        // their family's colour rather than coming out unstyled.
        let keyword = syntax.style("keyword").unwrap().color;
        assert_eq!(syntax.style("keyword.import").unwrap().color, keyword);
        assert_eq!(
            syntax.style("keyword.control.repeat").unwrap().color,
            keyword
        );

        let string = syntax.style("string").unwrap().color;
        assert_eq!(syntax.style("string.quoted.double").unwrap().color, string);

        // A name with no family in the table is genuinely unknown, and the
        // caller should fall back to the ambient text colour rather than get a
        // wrong one.
        assert!(syntax.style("nonsense").is_none());
        assert!(syntax.style("").is_none());
    }

    #[test]
    fn emphasis_survives_the_lookup() {
        let syntax = SyntaxPalette::new(&palette());
        assert_eq!(
            syntax.style("comment").unwrap().font_style,
            Some(FontStyle::Italic)
        );
        assert_eq!(
            syntax.style("emphasis.strong").unwrap().font_weight,
            Some(FontWeight::BOLD)
        );
        assert_eq!(syntax.style("keyword").unwrap().font_weight, None);
    }

    #[test]
    fn the_rendered_theme_round_trips_into_gpui_component() {
        // The JSON is hand-rendered, so a typo in the format string would
        // silently cost every markdown code block its colours. Deserialising it
        // through the real type is the only check that means anything.
        let syntax = SyntaxPalette::new(&palette());
        let theme = syntax.highlight_theme().expect("renders valid Zed JSON");

        let ours = syntax.style("keyword").unwrap().color;
        assert_eq!(theme.style("keyword").and_then(|s| s.color), ours);
        assert_eq!(
            theme.style("string").and_then(|s| s.color),
            syntax.style("string").unwrap().color
        );
        assert_eq!(
            theme.style("comment").and_then(|s| s.font_style),
            Some(FontStyle::Italic)
        );
    }

    #[test]
    fn colours_clear_the_contrast_floor_on_every_stock_theme() {
        // The floor is what stops a theme whose `muted` equals its background
        // from rendering comments invisible.
        const THEMES_DIR: &str = "/usr/share/omarchy/themes";
        let Ok(entries) = std::fs::read_dir(THEMES_DIR) else {
            eprintln!("no theme corpus at {THEMES_DIR}; skipping");
            return;
        };

        let mut checked = 0;
        for entry in entries.flatten() {
            let colors = entry.path().join("colors.toml");
            let Ok(palette) = Palette::from_toml_file(&colors) else {
                continue;
            };
            let syntax = SyntaxPalette::new(&palette);
            for (capture, ink, _) in &syntax.entries {
                let ratio = contrast::contrast_ratio(*ink, syntax.background);
                assert!(
                    ratio >= contrast::MIN_SECONDARY_CONTRAST - 0.01,
                    "{capture} on {:?} is {ratio:.2}:1",
                    entry.file_name()
                );
            }
            checked += 1;
        }
        assert!(checked > 0, "corpus found but no theme parsed");
    }
}
