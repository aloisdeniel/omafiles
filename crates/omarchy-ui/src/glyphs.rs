//! Ink-centred glyphs.
//!
//! Omarchy's font is the plain Nerd Font variant, whose icons keep the
//! 0.6em monospace advance but draw out to 0.75–0.92em (measured on
//! JetBrainsMono Nerd Font: the folder is 0.924em wide). A layout that
//! centres the advance therefore leaves the ink a pixel or two right of
//! centre — obvious on a square glyph button. Vertically the same icons sit
//! exactly on the text box's centre, so only the x axis needs help.
//!
//! gpui cannot say where a glyph's ink is: on Linux `typographic_bounds`
//! returns the advance, and the raster bounds it computes for its atlas are
//! crate-private. So the font file is read directly, found through
//! fontconfig like the family itself is, and each glyph's shift is measured
//! once and cached per family.

use std::collections::HashMap;

use gpui::{App, Global};

use crate::ActiveTheme as _;

/// How far right of its advance's centre `glyph`'s ink sits, in pixels at
/// `size` — what to move a centred glyph *left* by so the ink is centred.
///
/// Zero for text, for anything longer than one character, and whenever the
/// font cannot be read: the fallback is the plain centring, never a guess.
pub fn glyph_ink_shift(glyph: &str, size: f32, cx: &mut App) -> f32 {
    let mut chars = glyph.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return 0.0;
    };
    let family = cx.theme().type_scale().family.clone();
    let metrics = cx.default_global::<GlyphMetrics>();
    if metrics.family != family {
        *metrics = GlyphMetrics::load(family);
    }
    let GlyphMetrics { face, shifts, .. } = metrics;
    let em = *shifts
        .entry(ch)
        .or_insert_with(|| face.as_ref().map_or(0.0, |face| face.ink_shift_em(ch)));
    em * size
}

/// The face for the current family, and every shift measured so far.
#[derive(Default)]
struct GlyphMetrics {
    family: String,
    face: Option<FontFile>,
    /// In em, so one measurement serves every size.
    shifts: HashMap<char, f32>,
}

impl Global for GlyphMetrics {}

impl GlyphMetrics {
    fn load(family: String) -> Self {
        let face = FontFile::for_family(&family);
        if face.is_none() {
            eprintln!("omarchy-ui: no readable font file for {family:?}; glyphs centre by advance");
        }
        Self {
            family,
            face,
            shifts: HashMap::new(),
        }
    }
}

/// A font file's bytes and the face index fontconfig chose in it.
struct FontFile {
    data: Vec<u8>,
    index: u32,
}

impl FontFile {
    /// Resolve `family` through fontconfig, as the family itself was.
    fn for_family(family: &str) -> Option<Self> {
        let output = std::process::Command::new("fc-match")
            .args([family, "-f", "%{file}\n%{index}"])
            .output()
            .ok()
            .filter(|out| out.status.success())?;
        let text = String::from_utf8_lossy(&output.stdout);
        let mut lines = text.lines();
        let path = lines.next()?.trim();
        let index = lines
            .next()
            .and_then(|i| i.trim().parse().ok())
            .unwrap_or(0);
        Self::read(path, index)
    }

    fn read(path: &str, index: u32) -> Option<Self> {
        let data = std::fs::read(path).ok()?;
        // Parse once to reject what ttf-parser cannot read, so a broken file
        // costs one message at load rather than a failure per glyph.
        ttf_parser::Face::parse(&data, index).ok()?;
        Some(Self { data, index })
    }

    fn ink_shift_em(&self, ch: char) -> f32 {
        let Ok(face) = ttf_parser::Face::parse(&self.data, self.index) else {
            return 0.0;
        };
        ink_shift_em(&face, ch)
    }
}

/// The ink centre minus the advance centre, in em. Zero when the face has
/// no glyph for `ch` — the shaper will fall back to another font, whose
/// geometry this face cannot speak for.
fn ink_shift_em(face: &ttf_parser::Face, ch: char) -> f32 {
    let Some(id) = face.glyph_index(ch) else {
        return 0.0;
    };
    let (Some(advance), Some(ink)) = (face.glyph_hor_advance(id), face.glyph_bounding_box(id))
    else {
        return 0.0;
    };
    let upm = face.units_per_em() as f32;
    let ink_centre = (ink.x_min as f32 + ink.x_max as f32) / 2.0;
    (ink_centre - advance as f32 / 2.0) / upm
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against the real font when it is installed — the numbers in the
    /// module docs — and a no-op elsewhere, so CI without the font passes
    /// rather than pretending.
    #[test]
    fn nerd_font_icons_are_right_of_centre_and_text_is_not() {
        let Some(file) = FontFile::for_family("JetBrainsMono Nerd Font") else {
            return;
        };
        let face = ttf_parser::Face::parse(&file.data, file.index).unwrap();
        if face.glyph_index('\u{f114}').is_none() {
            return; // fontconfig substituted a font with no icons
        }
        let folder = ink_shift_em(&face, '\u{f114}');
        let letter = ink_shift_em(&face, 'a');
        assert!(folder > 0.1, "folder icon shift {folder} em");
        assert!(letter.abs() < 0.02, "letter shift {letter} em");
    }

    #[test]
    fn a_missing_glyph_shifts_nothing() {
        let Some(file) = FontFile::for_family("monospace") else {
            return;
        };
        let face = ttf_parser::Face::parse(&file.data, file.index).unwrap();
        // A noncharacter no font maps.
        assert_eq!(ink_shift_em(&face, '\u{fffe}'), 0.0);
    }
}
