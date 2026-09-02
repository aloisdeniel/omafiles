//! Driving `gpui-component`'s theme from Omarchy tokens.
//!
//! # Why this exists
//!
//! Bare gpui has 13 elements and no text input, no visible scrollbar and no
//! context menu — a file explorer needs all three, and hand-writing them is
//! weeks of work before any file-manager logic gets written.
//! [`gpui-component`](https://github.com/longbridge/gpui-component) has them,
//! is Apache-2.0, and its own committed lockfile pins the same zed rev we do.
//!
//! But it ships its own theme system, and a widget kit painting its own colours
//! next to `omarchy-ui` painting Omarchy's would be a Frankenstein. So this
//! module makes **Omarchy the single source of truth**: it pushes our tokens
//! into gpui-component's `Theme` global, and its widgets then render in the
//! system palette with no per-widget work.
//!
//! # Layering
//!
//! - `omarchy-ui`'s own components — the ones carrying Omarchy's identity —
//!   read [`crate::Theme`] directly.
//! - gpui-component's components read *their* `Theme`, which this keeps in sync.
//!
//! Call [`sync_gpui_component`] once at startup and again on every theme change.

use gpui::{App, px};
use gpui_component::{Theme as KitTheme, ThemeMode};
use omarchy_tokens::{Mode, Rgb, Tokens};

use crate::{color, contrast};

/// Push Omarchy's tokens into `gpui-component`'s theme global.
///
/// Requires `gpui_component::init(cx)` to have run first — it is what creates
/// the global this mutates.
///
/// ⚠ **`sync_base` is not optional.** gpui-component mirrors the radius, the
/// colours and the fonts into a second "base" global, and the scrollbar and
/// resize handles read *that* one. Mutating the theme without syncing leaves
/// them on the previous palette — a partial retint that is easy to miss because
/// most widgets look correct.
pub fn sync_gpui_component(tokens: &Tokens, cx: &mut App) {
    {
        let kit = KitTheme::global_mut(cx);
        apply(tokens, kit);
    }
    KitTheme::sync_base(cx);
}

fn apply(tokens: &Tokens, kit: &mut KitTheme) {
    let p = &tokens.palette;
    let controls = &tokens.controls;

    kit.mode = match p.mode() {
        Mode::Dark => ThemeMode::Dark,
        Mode::Light => ThemeMode::Light,
    };

    // Syntax colours, so fenced code blocks inside a rendered markdown preview
    // are tinted by the same table that colours a source file. `sync_base` reads
    // this when it re-installs the rich-text defaults, so it must be set before
    // that call — which is why it lives here and not beside it.
    match crate::SyntaxPalette::new(p).highlight_theme() {
        Ok(theme) => kit.highlight_theme = theme,
        // Keeping the kit's own theme means code blocks are off-palette, not
        // invisible. Worth a line on stderr and not worth failing a theme switch.
        Err(err) => eprintln!("omarchy-ui: syntax theme unavailable: {err:#}"),
    }

    // Typography and geometry. Omarchy is a monospace-first desktop, so both
    // the proportional and mono slots get the same family on purpose.
    kit.font_family = tokens.typography.family.clone().into();
    kit.mono_font_family = tokens.typography.family.clone().into();
    kit.font_size = px(tokens.typography.body());
    kit.mono_font_size = px(tokens.typography.body());
    kit.radius = px(tokens.geometry.corner_radius);
    kit.radius_lg = px(tokens.geometry.corner_radius * 2.0);

    let bg = p.background();
    let surface = p.lighter_background();
    let fg = p.foreground();
    let c = &mut kit.colors;

    // --- foundations
    c.background = color(bg);
    c.foreground = color(fg);
    c.border = fill(controls.normal.border, controls.normal.border_alpha, bg);
    c.ring = color(p.accent());
    c.selection = fill(p.selection(), controls.selection_fill_alpha, bg);
    c.caret = color(p.bright_foreground());
    c.drop_target = fill(p.accent(), 0.24, bg);

    // --- muted / secondary text.
    // `muted_foreground` is what most secondary labels use, so it goes through
    // the contrast floor: on `white`, Omarchy's dark_foreground is identical to
    // the surface behind it.
    c.muted = color(surface);
    c.muted_foreground = color(contrast::ensure_contrast(
        p.dark_foreground(),
        surface,
        contrast::MIN_SECONDARY_CONTRAST,
    ));

    // --- accent, primary, secondary, danger
    c.accent = fill(p.accent(), controls.selected.fill_alpha, bg);
    c.accent_foreground = color(p.accent());
    c.primary = color(p.accent());
    c.primary_hover = color(p.accent().mix(fg, 0.12));
    c.primary_active = color(p.accent().mix(Rgb::BLACK, 0.12));
    c.primary_foreground = color(readable_on(p.accent(), p));
    c.secondary = fill(fg, controls.normal.fill_alpha, bg);
    c.secondary_hover = fill(fg, controls.hover_cursor.fill_alpha, bg);
    c.secondary_active = fill(fg, controls.pressed_fill_alpha, bg);
    c.secondary_foreground = color(fg);
    c.danger = color(p.urgent());
    c.danger_hover = color(p.urgent().mix(fg, 0.12));
    c.danger_active = color(p.urgent().mix(Rgb::BLACK, 0.12));
    c.danger_foreground = color(readable_on(p.urgent(), p));

    // --- surfaces
    c.popover = color(tokens.surfaces.popups.background);
    c.popover_foreground = color(tokens.surfaces.popups.text);
    c.input = color(surface);
    c.sidebar = color(surface);
    c.sidebar_foreground = color(fg);
    c.sidebar_border = c.border;
    c.sidebar_accent = fill(fg, controls.selected.fill_alpha, surface);
    c.sidebar_accent_foreground = color(p.bright_foreground());

    // --- lists. The five-state vocabulary, so gpui-component's rows read the
    // same as `omarchy_ui::Row`.
    c.list = color(bg);
    c.list_even = color(bg);
    c.list_head = color(surface);
    c.list_hover = fill(fg, controls.hover_cursor.fill_alpha, bg);
    c.list_active = fill(fg, controls.selected.fill_alpha, bg);
    c.list_active_border = color(p.accent());

    // --- scrollbar. The thing bare gpui does not have at all, and the main
    // reason this dependency earns its place.
    c.scrollbar = fill(fg, 0.02, bg);
    c.scrollbar_thumb = fill(fg, 0.24, bg);
    c.scrollbar_thumb_hover = fill(fg, 0.40, bg);
}

/// Composite `color` at `alpha` over `over`, producing an opaque result.
///
/// gpui-component's fields are opaque `Hsla`, and Omarchy's states are alpha
/// washes, so the compositing has to happen here rather than at paint time.
fn fill(color_in: Rgb, alpha: f32, over: Rgb) -> gpui::Hsla {
    color(over.mix(color_in, alpha.clamp(0.0, 1.0)))
}

/// Text drawn *on* a filled accent or danger swatch — pick whichever of the
/// theme's extremes reads better against it.
fn readable_on(surface: Rgb, palette: &omarchy_tokens::Palette) -> Rgb {
    contrast::best_of(surface, palette.bright_foreground(), palette.background())
}

#[cfg(test)]
mod tests {
    use super::*;
    use omarchy_tokens::Palette;

    fn tokens_with(colors: &str) -> Tokens {
        use omarchy_tokens::{ControlStates, Geometry, ShellValues, Spacing, Typography};
        let palette = Palette::from_toml_str(colors).unwrap();
        let shell = ShellValues::default();
        let typography = Typography::new("mono".into(), &shell);
        let spacing = Spacing::new(&shell, &typography);
        Tokens {
            theme_name: "test".into(),
            controls: ControlStates::new(&shell, &palette),
            surfaces: omarchy_tokens::Surfaces::new(&shell, &palette),
            typography,
            spacing,
            palette,
            geometry: Geometry::default(),
            shell,
        }
    }

    const WHITE: &str = r##"
mode = "light"
background = "#ffffff"
lighter_background = "#c0c0c0"
foreground = "#2a2a2a"
dark_foreground = "#c0c0c0"
bright_foreground = "#000000"
accent = "#6e6e6e"
selection = "#e5e5e5"
muted = "#8a8a8a"
red = "#2a2a2a"
yellow = "#4a4a4a"
green = "#3a3a3a"
cyan = "#5a5a5a"
blue = "#3a3a3a"
magenta = "#4a4a4a"
"##;

    /// The whole point: gpui-component's widgets must not paint their own
    /// palette next to ours.
    #[test]
    fn maps_the_omarchy_background_and_foreground() {
        let tokens = tokens_with(WHITE);
        let mut kit = KitTheme::default();
        apply(&tokens, &mut kit);

        assert_eq!(kit.colors.background, color(tokens.palette.background()));
        assert_eq!(kit.colors.foreground, color(tokens.palette.foreground()));
        assert_eq!(
            kit.mode,
            ThemeMode::Light,
            "light themes must not read as dark"
        );
    }

    /// The §2.2d collision reaches gpui-component too — its secondary labels
    /// would be invisible on `white` if we passed the raw palette value.
    #[test]
    fn muted_foreground_goes_through_the_contrast_floor() {
        let tokens = tokens_with(WHITE);
        let mut kit = KitTheme::default();
        apply(&tokens, &mut kit);

        let raw = color(tokens.palette.dark_foreground());
        assert_ne!(
            kit.colors.muted_foreground, raw,
            "must not hand gpui-component a colour identical to its own surface"
        );
    }

    #[test]
    fn state_fills_are_composited_opaque_and_ordered() {
        let tokens = tokens_with(WHITE);
        let mut kit = KitTheme::default();
        apply(&tokens, &mut kit);

        // Fully opaque: gpui-component's fields are not alpha-composited later.
        assert_eq!(kit.colors.list_hover.a, 1.0);
        assert_eq!(kit.colors.list_active.a, 1.0);
        // And active must be a stronger wash than hover, as in Style.qml.
        assert_ne!(kit.colors.list_hover, kit.colors.list_active);
    }

    #[test]
    fn geometry_and_font_come_from_omarchy() {
        let mut tokens = tokens_with(WHITE);
        tokens.geometry.corner_radius = 7.0;
        let mut kit = KitTheme::default();
        apply(&tokens, &mut kit);

        assert_eq!(kit.radius, px(7.0));
        assert_eq!(kit.font_family.as_ref(), "mono");
        assert_eq!(kit.font_size, px(tokens.typography.body()));
    }
}
