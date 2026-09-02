//! Keeping text readable across a corpus we do not control.
//!
//! # Why this exists
//!
//! Omarchy's palette is authored per theme, and some themes give two roles the
//! same value. On `white`, `dark_foreground` and `lighter_background` are both
//! `#c0c0c0` — so secondary text drawn on a panel is *invisible*, not merely
//! low-contrast.
//!
//! # The rule
//!
//! Match the shell, and rescue only the degenerate cases.
//!
//! The whole premise of `omarchy-ui` is that it looks like the rest of the
//! desktop, so forcing every pair to WCAG AA would be the wrong fix — it would
//! systematically diverge from the bar and the menu on every theme. Instead
//! [`ensure_contrast`] is a *floor*: it leaves a colour alone when it is
//! already legible, and only steps in when it is not.

use omarchy_tokens::Rgb;

/// Minimum contrast for secondary text — labels, captions, dim rows.
///
/// **Well below WCAG, and deliberately so.** Omarchy's `dark_foreground` sits
/// between 1.0:1 and 2.9:1 against its own surfaces across the stock corpus;
/// dim text is dim by design. Holding it to a real accessibility threshold
/// would mean overruling the theme author nearly everywhere, which defeats the
/// point of a crate whose job is to look like the rest of the desktop.
///
/// Measured against the 22 stock themes (44 theme/surface pairs) by
/// `tests/legibility.rs`:
///
/// | Floor | Pairs adjusted | Themes touched |
/// | --- | --- | --- |
/// | 2.0 | 6 | 4 |
/// | 2.5 | 20 | 13 |
/// | 3.0 (WCAG large text) | 28 | 16 |
/// | 4.5 (WCAG body text) | nearly all | nearly all |
///
/// 2.0 rescues only what is genuinely unreadable — `white`'s secondary text is
/// *byte-identical* to the panel behind it at 1.00:1 — and leaves the other 18
/// themes exactly as written. Raising it is a one-line change if the balance
/// should ever tip toward accessibility over fidelity.
pub const MIN_SECONDARY_CONTRAST: f32 = 2.0;

/// Minimum contrast for anything load-bearing — primary text, focused rows.
pub const MIN_PRIMARY_CONTRAST: f32 = 4.5;

/// WCAG relative-contrast ratio, from 1.0 (identical) to 21.0 (black on white).
pub fn contrast_ratio(a: Rgb, b: Rgb) -> f32 {
    let (l1, l2) = (a.luminance(), b.luminance());
    let (lighter, darker) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
    (lighter + 0.05) / (darker + 0.05)
}

/// Return `foreground`, or the nearest adjustment to it that reaches
/// `min_ratio` against `background`.
///
/// Blends toward black or white — whichever direction actually increases
/// contrast with this background — in small steps, so a colour that is only
/// slightly too close keeps most of its hue. Returns the pure extreme if even
/// that cannot reach the target, which happens only for mid-grey backgrounds
/// where no colour can.
pub fn ensure_contrast(foreground: Rgb, background: Rgb, min_ratio: f32) -> Rgb {
    if contrast_ratio(foreground, background) >= min_ratio {
        return foreground;
    }

    // Push away from the background: darken against a light one, lighten
    // against a dark one.
    let target = if background.luminance() > 0.5 {
        Rgb::BLACK
    } else {
        Rgb::WHITE
    };

    const STEPS: u8 = 20;
    for step in 1..=STEPS {
        let amount = step as f32 / STEPS as f32;
        let candidate = foreground.mix(target, amount);
        if contrast_ratio(candidate, background) >= min_ratio {
            return candidate;
        }
    }

    target
}

/// Pick whichever of `light` and `dark` reads better on `background`.
///
/// For text drawn *on top of* a filled surface — a selected row, a badge —
/// where the palette offers a natural pair.
pub fn best_of(background: Rgb, light: Rgb, dark: Rgb) -> Rgb {
    if contrast_ratio(light, background) >= contrast_ratio(dark, background) {
        light
    } else {
        dark
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(hex: &str) -> Rgb {
        hex.parse().unwrap()
    }

    #[test]
    fn contrast_spans_the_full_wcag_range() {
        assert!((contrast_ratio(Rgb::BLACK, Rgb::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(Rgb::WHITE, Rgb::WHITE) - 1.0).abs() < 0.01);
        // Order must not matter.
        assert_eq!(
            contrast_ratio(Rgb::BLACK, Rgb::WHITE),
            contrast_ratio(Rgb::WHITE, Rgb::BLACK)
        );
    }

    #[test]
    fn leaves_an_already_legible_colour_untouched() {
        let fg = rgb("#a9b1d6");
        let bg = rgb("#1a1b26"); // tokyo-night: 8.5:1, comfortably legible
        assert!(contrast_ratio(fg, bg) >= MIN_SECONDARY_CONTRAST);
        assert_eq!(
            ensure_contrast(fg, bg, MIN_SECONDARY_CONTRAST),
            fg,
            "must not repaint text that is already fine — that would diverge \
             from the shell on every theme"
        );
    }

    /// The exact case from `PLAN.md` §2.2d: on the `white` theme,
    /// `dark_foreground` and `lighter_background` are both `#c0c0c0`.
    #[test]
    fn rescues_invisible_text_on_the_white_theme() {
        let invisible = rgb("#c0c0c0");
        let surface = rgb("#c0c0c0");
        assert!(
            (contrast_ratio(invisible, surface) - 1.0).abs() < 0.01,
            "identical colours, i.e. invisible"
        );

        let fixed = ensure_contrast(invisible, surface, MIN_SECONDARY_CONTRAST);
        assert_ne!(fixed, invisible);
        assert!(contrast_ratio(fixed, surface) >= MIN_SECONDARY_CONTRAST);
        assert!(
            fixed.luminance() < surface.luminance(),
            "a light surface should darken its text, not lighten it"
        );
    }

    #[test]
    fn lightens_against_a_dark_background() {
        let fg = rgb("#1c1c1c");
        let bg = rgb("#121212"); // matte-black
        let fixed = ensure_contrast(fg, bg, MIN_SECONDARY_CONTRAST);
        assert!(fixed.luminance() > bg.luminance());
        assert!(contrast_ratio(fixed, bg) >= MIN_SECONDARY_CONTRAST);
    }

    #[test]
    fn handles_the_corpus_extremes() {
        for bg in [rgb("#000000"), rgb("#ffffff")] {
            let fixed = ensure_contrast(bg, bg, MIN_PRIMARY_CONTRAST);
            assert!(
                contrast_ratio(fixed, bg) >= MIN_PRIMARY_CONTRAST,
                "failed on {}",
                bg.to_hex()
            );
        }
    }

    /// Mid-grey is the one background where no colour reaches 4.5:1. The
    /// function must still terminate and return its best effort.
    #[test]
    fn degrades_gracefully_when_the_target_is_unreachable() {
        let bg = rgb("#808080");
        let fixed = ensure_contrast(rgb("#7f7f7f"), bg, 21.0);
        assert!(fixed == Rgb::BLACK || fixed == Rgb::WHITE);
    }

    #[test]
    fn best_of_picks_the_higher_contrast_option() {
        let light = rgb("#ffffff");
        let dark = rgb("#000000");
        assert_eq!(best_of(rgb("#1a1b26"), light, dark), light);
        assert_eq!(best_of(rgb("#fafafa"), light, dark), dark);
    }
}
