//! The mechanical half of M2's success criterion.
//!
//! `PLAN.md` says: run the gallery through all 22 themes and every text size,
//! and nothing should be illegible, clipped or stale. Eyeballing 22 themes ×
//! 12 sizes is 264 screenshots and is not a thing anyone will redo on every
//! change — so the part that *can* be checked mechanically is checked here, and
//! the gallery is left for the part that genuinely needs eyes (layout, clipping,
//! rhythm).
//!
//! No display, no gpui window: these assert on the token values a component
//! would resolve, which is exactly where a legibility bug originates.

use omarchy_tokens::{Palette, Rgb, ShellValues, Spacing, Typography};
use omarchy_ui::{MIN_SECONDARY_CONTRAST, contrast_ratio, ensure_contrast};

const THEMES_DIR: &str = "/usr/share/omarchy/themes";

fn themes() -> Vec<(String, Palette)> {
    let Ok(entries) = std::fs::read_dir(THEMES_DIR) else {
        return Vec::new();
    };
    let mut themes: Vec<(String, Palette)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let colors = entry.path().join("colors.toml");
            let palette = Palette::from_toml_file(&colors).ok()?;
            Some((entry.file_name().to_string_lossy().into_owned(), palette))
        })
        .collect();
    themes.sort_by(|a, b| a.0.cmp(&b.0));
    themes
}

fn skip_if_absent(themes: &[(String, Palette)]) -> bool {
    if themes.is_empty() {
        eprintln!("skipping: no themes under {THEMES_DIR}");
        return true;
    }
    false
}

/// Primary text must be readable on both the window background and a panel.
#[test]
fn primary_text_is_readable_on_every_theme() {
    let themes = themes();
    if skip_if_absent(&themes) {
        return;
    }

    let mut failures = Vec::new();
    for (name, palette) in &themes {
        for (surface_name, surface) in [
            ("background", palette.background()),
            ("lighter_background", palette.lighter_background()),
        ] {
            let ratio = contrast_ratio(palette.foreground(), surface);
            if ratio < MIN_SECONDARY_CONTRAST {
                failures.push(format!(
                    "{name}: foreground {} on {surface_name} {} = {ratio:.2}:1",
                    palette.foreground().to_hex(),
                    surface.to_hex()
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "primary text below {MIN_SECONDARY_CONTRAST}:1 on {} theme/surface pairs:\n{}",
        failures.len(),
        failures.join("\n")
    );
    eprintln!("{} themes: primary text legible", themes.len());
}

/// **The §2.2d regression.** `dark_foreground` is what secondary text uses, and
/// on `white` it is byte-identical to the panel it sits on. The raw value fails;
/// the value the crate actually hands to a component must not.
#[test]
fn secondary_text_is_rescued_where_the_palette_collides() {
    let themes = themes();
    if skip_if_absent(&themes) {
        return;
    }

    let mut rescued = Vec::new();
    let mut failures = Vec::new();

    for (name, palette) in &themes {
        for (surface_name, surface) in [
            ("background", palette.background()),
            ("lighter_background", palette.lighter_background()),
        ] {
            let raw = palette.dark_foreground();
            let fixed = ensure_contrast(raw, surface, MIN_SECONDARY_CONTRAST);
            let ratio = contrast_ratio(fixed, surface);

            if fixed != raw {
                rescued.push(format!(
                    "{name}/{surface_name}: {} → {} ({:.2}:1 → {ratio:.2}:1)",
                    raw.to_hex(),
                    fixed.to_hex(),
                    contrast_ratio(raw, surface),
                ));
            }
            if ratio < MIN_SECONDARY_CONTRAST {
                failures.push(format!(
                    "{name}/{surface_name}: {ratio:.2}:1 after adjustment"
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "secondary text still illegible after adjustment:\n{}",
        failures.join("\n")
    );

    // The rescue must actually fire somewhere, or the test is vacuous and would
    // keep passing if `ensure_contrast` became the identity function.
    assert!(
        !rescued.is_empty(),
        "no theme needed a contrast rescue — either the corpus changed or \
         ensure_contrast stopped doing anything"
    );
    eprintln!(
        "rescued {} theme/surface pairs:\n{}",
        rescued.len(),
        rescued.join("\n")
    );
}

/// The five interaction states are washes over the background. If two of them
/// land on the same colour, the user cannot tell hover from selected.
#[test]
fn interaction_states_stay_distinguishable() {
    let themes = themes();
    if skip_if_absent(&themes) {
        return;
    }

    // Style.qml's defaults.
    const ALPHAS: [(&str, f32); 4] = [
        ("normal", 0.04),
        ("hover", 0.08),
        ("selected", 0.18),
        ("pressed", 0.22),
    ];

    let mut failures = Vec::new();
    for (name, palette) in &themes {
        let bg = palette.background();
        let fg = palette.foreground();

        // Composite each wash over the background, the way the compositor will.
        let composited: Vec<(&str, Rgb)> = ALPHAS
            .iter()
            .map(|(label, alpha)| (*label, bg.mix(fg, *alpha)))
            .collect();

        for pair in composited.windows(2) {
            let (a_name, a) = pair[0];
            let (b_name, b) = pair[1];
            if a == b {
                failures.push(format!(
                    "{name}: {a_name} and {b_name} both composite to {}",
                    a.to_hex()
                ));
            }
        }

        // And the quietest wash must actually differ from bare background,
        // or "normal" chrome is invisible.
        if composited[0].1 == bg {
            failures.push(format!(
                "{name}: normal fill is indistinguishable from background"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "interaction states collapse on {} themes:\n{}",
        failures.len(),
        failures.join("\n")
    );
    eprintln!(
        "{} themes: interaction states distinguishable",
        themes.len()
    );
}

/// The other half of "nothing clipped": every text size from 9 to 20 must leave
/// body text fitting inside a control, and the scale must stay monotonic.
#[test]
fn the_scale_stays_sane_across_every_text_size() {
    // omarchy-display-text-size accepts 9..=20.
    for base in 9..=20 {
        let values = ShellValues::from_toml_str(&format!("[font]\nbase-size = {base}\n"));
        let type_scale = Typography::new("test".into(), &values);
        let spacing = Spacing::new(&values, &type_scale);

        assert_eq!(type_scale.body(), base as f32, "body must equal base-size");

        // Monotonic, so "caption" never renders larger than "heading".
        let steps = [
            type_scale.caption(),
            type_scale.body_small(),
            type_scale.body(),
            type_scale.subtitle(),
            type_scale.title(),
            type_scale.heading(),
        ];
        for pair in steps.windows(2) {
            assert!(
                pair[1] >= pair[0],
                "scale is not monotonic at base-size {base}: {steps:?}"
            );
        }

        // A row must be able to contain its text. This is the clipping check
        // the gallery would otherwise be the only way to catch.
        assert!(
            spacing.control_height() >= type_scale.body(),
            "at base-size {base}, control-height {} cannot contain body text {}",
            spacing.control_height(),
            type_scale.body(),
        );
        assert!(
            spacing.control_height() >= type_scale.title(),
            "at base-size {base}, control-height {} clips title text {}",
            spacing.control_height(),
            type_scale.title(),
        );

        // Spacing must never collapse to zero, or the layout loses its rhythm.
        assert!(
            spacing.hairline() >= 1.0,
            "hairline vanished at base-size {base}"
        );
        assert!(
            spacing.row_padding_x() > 0.0,
            "row padding vanished at {base}"
        );
    }
}
