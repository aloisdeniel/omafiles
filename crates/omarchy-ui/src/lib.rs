//! An Omarchy design system for gpui.
//!
//! The tokens are a faithful port of `omarchy-shell`'s `Commons/Style.qml` and
//! `Commons/Color.qml`, so an app built on this crate reads as part of the same
//! desktop as the bar and the menu rather than merely themed to match.
//!
//! M1 scope: the [`Theme`] global, colour conversion, the subscription helper,
//! and live tracking of the system theme. M2 adds `InteractiveSurface` and the
//! component kit.

use futures::StreamExt as _;
use gpui::{App, Global, Hsla, Rgba};
use omarchy_tokens::{ControlStates, Rgb, Spacing, Surfaces, Tokens, Typography};

mod components;
mod contrast;
mod interactive;
mod interop;
mod modal;
mod syntax;

pub use components::{
    ActionButton, Badge, BadgeTone, Breadcrumb, Button, ButtonKind, Hint, KeyHint, Panel,
    ROW_GROUP, Row, SectionHeader, Separator,
};
pub use contrast::{
    MIN_PRIMARY_CONTRAST, MIN_SECONDARY_CONTRAST, best_of, contrast_ratio, ensure_contrast,
};
pub use interactive::{Chrome, InteractiveSurface, SurfaceState};
pub use interop::sync_gpui_component;
pub use modal::{Modal, ModalSize};
pub use syntax::SyntaxPalette;

/// Ergonomic access to the [`Theme`] global.
///
/// ```ignore
/// div().bg(cx.theme().background()).p(px(cx.theme().space().panel_padding()))
/// ```
///
/// Reading the theme does **not** subscribe to it. A view that renders from
/// `cx.theme()` still needs [`observe_theme`] to re-render when the theme
/// changes.
pub trait ActiveTheme {
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Theme {
        self.global::<Theme>()
    }
}

impl<T> ActiveTheme for gpui::Context<'_, T> {
    fn theme(&self) -> &Theme {
        self.global::<Theme>()
    }
}

// Deliberately no impl for `Window`: it is not a context and holds no globals.
// Component code takes `&mut App` (or a `Context`) precisely so `cx.theme()`
// always has somewhere real to read from.

/// The live design tokens, as a gpui global.
///
/// # Subscribing
///
/// gpui globals do **not** invalidate views automatically — there is no
/// implicit dependency tracking. A view that only reads `cx.theme()` will keep
/// rendering the old palette after a theme switch. Every themed view must
/// subscribe and hold the returned [`gpui::Subscription`]; dropping it
/// silently unsubscribes. Use [`observe_theme`] rather than writing it by hand.
pub struct Theme {
    pub tokens: Tokens,
    /// Resolved once per theme rather than per rendered file: the table is 40
    /// contrast computations, and a preview re-renders on every cursor move.
    syntax: syntax::SyntaxPalette,
}

impl Global for Theme {}

impl Theme {
    pub fn new(tokens: Tokens) -> Self {
        Self {
            syntax: syntax::SyntaxPalette::new(&tokens.palette),
            tokens,
        }
    }

    /// Syntax colours for this theme. See [`SyntaxPalette`].
    pub fn syntax(&self) -> &syntax::SyntaxPalette {
        &self.syntax
    }

    /// Load from the live system, falling back to a built-in palette if
    /// Omarchy is not installed or the theme directory is mid-swap.
    pub fn load() -> Self {
        match omarchy_tokens::load() {
            Ok(tokens) => Self::new(tokens),
            Err(err) => {
                eprintln!("omarchy-ui: falling back to defaults: {err:#}");
                Self::new(fallback_tokens())
            }
        }
    }

    /// Install as the global and keep it current. Call once, before opening a
    /// window.
    ///
    /// Starts a watcher, so `omarchy theme set`, `omarchy font set` and
    /// `omarchy display text size` all reach the running app within a few
    /// hundred milliseconds. If the watcher cannot start, the app keeps the
    /// snapshot it loaded rather than failing — a static theme is a degraded
    /// experience, not a broken one.
    pub fn install(cx: &mut App) {
        cx.set_global(Self::load());
        Self::track_system_changes(cx);
    }

    /// Install a fixed snapshot with no watcher. For tests and screenshots.
    pub fn install_static(cx: &mut App, tokens: Tokens) {
        cx.set_global(Self::new(tokens));
    }

    /// Bridge the watcher's blocking thread onto gpui's executor.
    ///
    /// The watcher does blocking filesystem IO, so it lives on its own OS
    /// thread rather than occupying one from the background pool for the life
    /// of the process. It pushes through an async channel, so there is no
    /// polling timer.
    fn track_system_changes(cx: &mut App) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Tokens>();

        std::thread::Builder::new()
            .name("omarchy-ui-theme".into())
            .spawn(move || {
                let mut watcher = match omarchy_tokens::watch() {
                    Ok(watcher) => watcher,
                    Err(err) => {
                        eprintln!("omarchy-ui: theme watcher unavailable, staying static: {err:#}");
                        return;
                    }
                };

                // A long timeout rather than an infinite one so the thread
                // notices a closed channel and exits with the app.
                const POLL: std::time::Duration = std::time::Duration::from_secs(60);
                loop {
                    if watcher.wait(POLL) && tx.unbounded_send(watcher.current().clone()).is_err() {
                        return; // receiver dropped: the app is shutting down
                    }
                    if tx.is_closed() {
                        return;
                    }
                }
            })
            .expect("spawn theme watcher thread");

        // Detached rather than held: the task's lifetime is the app's. On quit
        // the task is dropped, which drops `rx`, which closes the channel, which
        // is what tells the watcher thread to exit.
        cx.spawn(async move |cx| {
            while let Some(tokens) = rx.next().await {
                // `set_global` notifies observers; every themed view re-renders
                // through its `observe_theme` subscription.
                cx.update(|cx| cx.set_global(Theme::new(tokens)));
            }
        })
        .detach();
    }

    // ------------------------------------------------------ structural tokens

    pub fn type_scale(&self) -> &Typography {
        &self.tokens.typography
    }

    pub fn space(&self) -> &Spacing {
        &self.tokens.spacing
    }

    pub fn controls(&self) -> &ControlStates {
        &self.tokens.controls
    }

    pub fn surfaces(&self) -> &Surfaces {
        &self.tokens.surfaces
    }

    /// Hyprland's `decoration:rounding`.
    pub fn radius(&self) -> f32 {
        self.tokens.geometry.corner_radius
    }

    /// Half of Hyprland's `general:gaps_out`, matching the shell.
    pub fn gap(&self) -> f32 {
        self.tokens.geometry.gaps_out
    }

    // ---------------------------------------------------------- palette roles

    /// The five role names a `shell.toml` colour token may resolve to, plus the
    /// surfaces built from them.
    pub fn background(&self) -> Hsla {
        color(self.tokens.palette.background())
    }
    pub fn foreground(&self) -> Hsla {
        color(self.tokens.palette.foreground())
    }
    pub fn accent(&self) -> Hsla {
        color(self.tokens.palette.accent())
    }
    /// `urgent` has no key of its own; Omarchy sources it from `red`.
    pub fn urgent(&self) -> Hsla {
        color(self.tokens.palette.urgent())
    }
    pub fn muted(&self) -> Hsla {
        color(self.tokens.palette.muted())
    }

    /// One step up from the base background — panels, cards, the sidebar.
    pub fn surface(&self) -> Hsla {
        color(self.tokens.palette.lighter_background())
    }
    /// One step down — the window chrome behind everything else.
    pub fn sunken(&self) -> Hsla {
        color(self.tokens.palette.dark_background())
    }
    pub fn bright_foreground(&self) -> Hsla {
        color(self.tokens.palette.bright_foreground())
    }
    /// Secondary text on the window background.
    ///
    /// Guaranteed legible: on themes where `dark_foreground` collides with the
    /// surface it sits on — `white` has both at `#c0c0c0` — this steps it far
    /// enough to be read. On every other theme it is the palette value
    /// untouched. See [`contrast`].
    pub fn dim_foreground(&self) -> Hsla {
        self.dim_foreground_on(self.tokens.palette.background())
    }

    /// Secondary text on an arbitrary background — a panel, a filled row.
    pub fn dim_foreground_on(&self, background: Rgb) -> Hsla {
        color(contrast::ensure_contrast(
            self.tokens.palette.dark_foreground(),
            background,
            contrast::MIN_SECONDARY_CONTRAST,
        ))
    }

    /// Primary text on an arbitrary background, held to the stricter floor.
    pub fn foreground_on(&self, background: Rgb) -> Hsla {
        color(contrast::ensure_contrast(
            self.tokens.palette.foreground(),
            background,
            contrast::MIN_PRIMARY_CONTRAST,
        ))
    }

    /// The raw palette value, with no contrast floor. For cases that must match
    /// the shell byte-for-byte even where that is hard to read.
    pub fn dim_foreground_raw(&self) -> Hsla {
        color(self.tokens.palette.dark_foreground())
    }

    // ------------------------------------------------------- interaction fills
    //
    // Omarchy states are alpha washes over the background rather than solid
    // fills. That is what keeps 22 very different themes — pure black through
    // pure white — legible without per-theme overrides. Colours and alphas come
    // from `[controls]`, so a theme that overrides them is honoured.

    fn state_fill(&self, state: &omarchy_tokens::StateStyle) -> Hsla {
        color(state.color).opacity(state.fill_alpha)
    }

    pub fn normal_fill(&self) -> Hsla {
        self.state_fill(&self.tokens.controls.normal)
    }
    pub fn hover_fill(&self) -> Hsla {
        self.state_fill(&self.tokens.controls.hover_cursor)
    }
    /// Real keyboard focus. Defaults to the hover treatment, so mouse hover,
    /// panel cursor and tab focus read as one state unless a theme separates
    /// them.
    pub fn focus_fill(&self) -> Hsla {
        self.state_fill(&self.tokens.controls.focus)
    }
    pub fn selected_fill(&self) -> Hsla {
        self.state_fill(&self.tokens.controls.selected)
    }
    pub fn pressed_fill(&self) -> Hsla {
        color(self.tokens.controls.pressed_color).opacity(self.tokens.controls.pressed_fill_alpha)
    }
    /// Text-selection highlight.
    pub fn selection_fill(&self) -> Hsla {
        color(self.tokens.palette.selection()).opacity(self.tokens.controls.selection_fill_alpha)
    }

    pub fn border(&self) -> Hsla {
        let normal = &self.tokens.controls.normal;
        color(normal.border).opacity(normal.border_alpha)
    }
    pub fn border_width(&self) -> f32 {
        self.tokens.controls.normal.border_width
    }
}

/// Convert an Omarchy colour into gpui's.
pub fn color(rgb: Rgb) -> Hsla {
    Rgba {
        r: rgb.r as f32 / 255.0,
        g: rgb.g as f32 / 255.0,
        b: rgb.b as f32 / 255.0,
        a: 1.0,
    }
    .into()
}

/// Subscribe a view to theme changes.
///
/// **Hold the returned [`gpui::Subscription`] in the view's own state.**
/// Dropping it unsubscribes, which produces a view that is correct until the
/// first theme switch and stale forever after — invisible in development,
/// obvious to a user.
///
/// ```ignore
/// struct FileList { _theme: Subscription }
///
/// impl FileList {
///     fn new(cx: &mut Context<Self>) -> Self {
///         Self { _theme: omarchy_ui::observe_theme(cx) }
///     }
/// }
/// ```
pub fn observe_theme<T: 'static>(cx: &mut gpui::Context<T>) -> gpui::Subscription {
    cx.observe_global::<Theme>(|_this, cx| cx.notify())
}

/// A minimal dark palette for when Omarchy is not present — CI, a non-Omarchy
/// machine, or a theme directory caught mid-swap.
fn fallback_tokens() -> Tokens {
    use omarchy_tokens::{Geometry, Palette, ShellValues};

    const DEFAULTS: &str = include_str!("fallback-theme.toml");

    let palette = Palette::from_toml_str(DEFAULTS).expect("built-in fallback palette must parse");
    let shell = ShellValues::default();
    let typography = Typography::new("monospace".to_string(), &shell);
    let spacing = Spacing::new(&shell, &typography);

    Tokens {
        theme_name: "fallback".to_string(),
        controls: ControlStates::new(&shell, &palette),
        surfaces: Surfaces::new(&shell, &palette),
        typography,
        spacing,
        palette,
        geometry: Geometry::default(),
        shell,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_fallback_parses() {
        let tokens = fallback_tokens();
        assert_eq!(tokens.theme_name, "fallback");
        assert_eq!(tokens.typography.base_size, 12.0);
        assert_eq!(tokens.typography.body(), 12.0);
        assert_eq!(tokens.spacing.lg(), 8.0);
    }

    #[test]
    fn converts_colour_channels_in_the_right_order() {
        let hsla = color(Rgb::new(0xff, 0x00, 0x00));
        let rgba: Rgba = hsla.into();
        assert!(rgba.r > 0.99 && rgba.g < 0.01 && rgba.b < 0.01);
    }
}
