//! Every component in every state, driven by the live system theme.
//!
//! This is the M2 success criterion made checkable by eye:
//!
//! ```text
//! cargo run -p omarchy-ui --example gallery
//! ```
//!
//! Then, in another terminal, walk the corpus:
//!
//! ```text
//! for t in $(omarchy theme list); do omarchy theme set "$t"; sleep 1; done
//! for n in $(seq 9 20); do omarchy display text size $n; sleep 1; done
//! ```
//!
//! Nothing should be illegible, clipped, or stale. `vantablack`, `white` and
//! the five light themes are the ones that actually fail — check those first.
//!
//! The mechanical half of the criterion — contrast across all 22 themes — is
//! `tests/legibility.rs`, which does not need a display.

use gpui::{
    App, AppContext as _, Context, IntoElement, ParentElement, Render, Styled, Subscription,
    TitlebarOptions, Window, WindowDecorations, WindowOptions, div, px,
};
use omarchy_ui::{
    ActionButton, ActiveTheme as _, Badge, Breadcrumb, Button, ButtonKind, Chrome,
    InteractiveSurface, KeyHint,
    Panel, Row, SectionHeader, Separator, SurfaceState, Theme,
};

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        Theme::install(cx);

        let options = WindowOptions {
            app_id: Some("dev.omarchy.omafiles.gallery".to_string()),
            titlebar: Some(TitlebarOptions {
                title: Some("omarchy-ui gallery".into()),
                ..Default::default()
            }),
            window_decorations: Some(WindowDecorations::Server),
            ..Default::default()
        };

        cx.open_window(options, |_window, cx| cx.new(Gallery::new))
            .expect("failed to open window");
        cx.activate(true);
    });
}

struct Gallery {
    /// Held, not ignored — dropping it silently stops tracking the theme.
    _theme: Subscription,
}

impl Gallery {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            _theme: omarchy_ui::observe_theme(cx),
        }
    }
}

impl Render for Gallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Everything the outer layout needs, taken by value up front: the
        // section helpers below want `&mut App`, which cannot coexist with a
        // live `&Theme` borrow.
        let (background, foreground, dim, family, body, caption, pad, gap, summary) = {
            let theme = cx.theme();
            let t = theme.type_scale();
            (
                theme.background(),
                theme.foreground(),
                theme.dim_foreground(),
                t.family.clone(),
                t.body(),
                t.caption(),
                theme.gap().max(theme.space().md()),
                theme.space().panel_gap(),
                format!(
                    "· {} · {:?} · {}px · ×{:.2}",
                    theme.tokens.theme_name,
                    theme.tokens.palette.mode(),
                    t.base_size,
                    theme.space().scale(),
                ),
            )
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(background)
            .text_color(foreground)
            .font_family(family)
            .text_size(px(body))
            .p(px(pad))
            .gap(px(gap))
            .child(header(cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(gap))
                    .child(states_panel(cx))
                    .child(controls_panel(cx)),
            )
            .child(rows_panel(cx))
            .child(scale_panel(cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(gap))
                    .child(KeyHint::new("j/k", "move"))
                    .child(KeyHint::new("\u{23ce}", "open"))
                    .child(KeyHint::new("/", "search"))
                    .child(KeyHint::new("?", "keys"))
                    .child(div().text_size(px(caption)).text_color(dim).child(summary)),
            )
    }
}

fn header(cx: &mut App) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .child(Breadcrumb::new(["~", "Documents", "Github", "omafiles"]))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(theme.space().control_gap()))
                .child(Badge::new("12 items"))
                .child(Badge::new("serving :8080").accent())
                .child(Badge::new("2 errors").urgent()),
        )
}

/// The five-state vocabulary, side by side. If two of these are
/// indistinguishable on some theme, the state system is wrong.
fn states_panel(cx: &mut App) -> impl IntoElement {
    let theme = cx.theme();
    let controls = theme.controls();

    let states = [
        ("normal", SurfaceState::Normal, controls.normal.fill_alpha),
        (
            "hover/cursor",
            SurfaceState::Hover,
            controls.hover_cursor.fill_alpha,
        ),
        ("focus", SurfaceState::Focus, controls.focus.fill_alpha),
        (
            "selected",
            SurfaceState::Selected,
            controls.selected.fill_alpha,
        ),
        (
            "pressed",
            SurfaceState::Pressed,
            controls.pressed_fill_alpha,
        ),
    ];

    Panel::new()
        .child(SectionHeader::new("interaction states"))
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(theme.space().control_gap()))
                .children(states.into_iter().map(|(label, state, alpha)| {
                    let surface = InteractiveSurface::new(state).chrome(Chrome::Always);
                    surface
                        .build(theme)
                        .px(px(theme.space().control_padding_x()))
                        .py(px(theme.space().control_padding_y()))
                        .text_size(px(theme.type_scale().caption()))
                        .text_color(surface.text_color(theme))
                        .child(format!("{label} {alpha:.2}"))
                })),
        )
}

fn controls_panel(cx: &mut App) -> impl IntoElement {
    let theme = cx.theme();
    Panel::new().child(SectionHeader::new("controls")).child(
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(theme.space().control_gap()))
            .child(Button::new("btn-normal", "Normal"))
            .child(Button::new("btn-primary", "Primary").kind(ButtonKind::Primary))
            .child(Button::new("btn-ghost", "Ghost").kind(ButtonKind::Ghost))
            .child(Button::new("btn-danger", "Delete").kind(ButtonKind::Danger))
            .child(Button::new("btn-active", "Toggled").active(true))
            .child(
                div()
                    .h(px(theme.space().control_height()))
                    .child(Separator::vertical()),
            )
            .child(Button::new("btn-tail", "After"))
            .child(
                div()
                    .h(px(theme.space().control_height()))
                    .child(Separator::vertical()),
            )
            // The quiet chrome verbs: every small action button in the app is
            // one of these, in exactly these states.
            .child(ActionButton::new("act-icon").glyph("\u{f062}"))
            .child(ActionButton::new("act-disabled").glyph("\u{f060}").enabled(false))
            .child(
                ActionButton::new("act-labelled")
                    .glyph("\u{f120}")
                    .label("Terminal"),
            )
            .child(
                ActionButton::new("act-accent")
                    .glyph("\u{f0ac}")
                    .label(":8080")
                    .accent(true),
            ),
    )
}

/// A miniature file listing, which is what `Row` actually exists for.
fn rows_panel(cx: &mut App) -> impl IntoElement {
    let theme = cx.theme();
    let entries = [
        ("crates/", "—", true, false),
        ("plan/", "—", false, false),
        ("Cargo.toml", "412 B", false, true),
        ("README.md", "37 B", false, false),
        (".gitignore", "24 B", false, false),
    ];

    Panel::new()
        .flush()
        .child(SectionHeader::new("rows — cursor on 3, selection on 1"))
        .child(
            div()
                .flex()
                .flex_col()
                .children(entries.into_iter().enumerate().map(
                    |(index, (name, size, selected, cursor))| {
                        Row::new(("entry", index))
                            .selected(selected)
                            .cursor(cursor)
                            .focused(true)
                            .child(div().flex_1().child(name))
                            .child(
                                div()
                                    .text_size(px(theme.type_scale().caption()))
                                    .text_color(theme.dim_foreground_on(
                                        theme.tokens.palette.lighter_background(),
                                    ))
                                    .child(size),
                            )
                    },
                )),
        )
}

/// The type scale plus the palette, so a wrong `base-size` or a broken
/// derivation is visible rather than subtle.
fn scale_panel(cx: &mut App) -> impl IntoElement {
    let theme = cx.theme();
    let t = theme.type_scale();
    let p = &theme.tokens.palette;

    let steps = [
        ("caption", t.caption()),
        ("body-small", t.body_small()),
        ("body", t.body()),
        ("subtitle", t.subtitle()),
        ("title", t.title()),
        ("heading", t.heading()),
    ];
    let swatches = [
        ("bg", p.background()),
        ("surface", p.lighter_background()),
        ("fg", p.foreground()),
        ("accent", p.accent()),
        ("urgent", p.urgent()),
        ("green", p.green()),
        ("yellow", p.yellow()),
        ("blue", p.blue()),
        ("magenta", p.magenta()),
        ("muted", p.muted()),
    ];

    Panel::new()
        .child(SectionHeader::new("type scale & palette"))
        .child(
            div()
                .flex()
                .flex_row()
                .items_baseline()
                .gap(px(theme.space().xl()))
                .children(steps.into_iter().map(|(name, size)| {
                    div().text_size(px(size)).child(format!("{name} {size:.0}"))
                })),
        )
        .child(Separator::horizontal())
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(theme.space().sm()))
                .children(swatches.into_iter().map(|(name, rgb)| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(theme.space().xxs()))
                        .child(
                            div()
                                .w(px(56.))
                                .h(px(theme.space().control_height()))
                                .rounded(px(theme.radius()))
                                .bg(omarchy_ui::color(rgb))
                                .border(px(1.))
                                .border_color(theme.border()),
                        )
                        .child(
                            div()
                                .text_size(px(theme.type_scale().caption()))
                                .text_color(
                                    theme.dim_foreground_on(
                                        theme.tokens.palette.lighter_background(),
                                    ),
                                )
                                .child(name),
                        )
                })),
        )
}
