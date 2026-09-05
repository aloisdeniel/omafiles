//! The bars: one over each panel, one under the window.
//!
//! Every top bar is the same height ([`crate::Theme::bar_height`]) so the rule
//! under each meets the others' across the vertical dividers, and every bar
//! keeps one rhythm — the small inset on every side and between items — so
//! the chrome is one number rather than a set of them.

use gpui::{AnyElement, App, Div, IntoElement, ParentElement, RenderOnce, Styled, Window, div, px};

use crate::ActiveTheme as _;

/// A flexible gap between a bar's leading and trailing items.
pub fn spacer() -> Div {
    div().flex_1()
}

/// A panel's top bar: [`crate::Theme::bar_height`] tall, items laid out
/// left to right with the small inset between them. Clips: a bar that
/// overflows its panel would paint over the neighbouring one.
///
/// ```ignore
/// Bar::new()
///     .child(ActionButton::new("back").glyph("\u{f060}"))
///     .child(Breadcrumb::new(["~", "Documents"]))
///     .child(spacer())
///     .child(QuietButton::new("collapse", "\u{f100}"))
/// ```
#[derive(IntoElement, Default)]
pub struct Bar {
    children: Vec<AnyElement>,
    centered: bool,
}

impl Bar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Centre the items rather than leading them — for a bar holding one
    /// control, like a collapsed panel's strip.
    pub fn centered(mut self) -> Self {
        self.centered = true;
        self
    }
}

impl ParentElement for Bar {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Bar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let inset = theme.space().sm();
        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .flex_shrink_0()
            .h(px(theme.bar_height()))
            .gap(px(inset))
            .p(px(inset))
            .overflow_hidden();
        if self.centered {
            bar = bar.justify_center();
        }
        bar.children(self.children)
    }
}

/// The bar under the window: facts on the left, the verbs that act on the
/// whole view on the right. Caption-sized secondary text, because it
/// annotates the window rather than joining its content.
#[derive(IntoElement, Default)]
pub struct StatusBar {
    leading: Vec<AnyElement>,
    trailing: Vec<AnyElement>,
}

impl StatusBar {
    pub fn new() -> Self {
        Self::default()
    }

    /// An item on the left — a count, a branch, a notice.
    pub fn leading(mut self, element: impl IntoElement) -> Self {
        self.leading.push(element.into_any_element());
        self
    }

    /// Items on the left, in order.
    pub fn leading_all(mut self, elements: impl IntoIterator<Item = AnyElement>) -> Self {
        self.leading.extend(elements);
        self
    }

    /// An item on the right — an [`crate::ActionButton`], typically.
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing.push(element.into_any_element());
        self
    }

    pub fn trailing_all(mut self, elements: impl IntoIterator<Item = AnyElement>) -> Self {
        self.trailing.extend(elements);
        self
    }
}

impl RenderOnce for StatusBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let space = theme.space();
        // One value everywhere: the same small inset on every side and
        // between items — except on the left, where the facts get the panel
        // inset so they do not hug the window edge.
        let inset = space.sm();
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(inset))
            .p(px(inset))
            .pl(px(space.panel_padding()))
            .text_size(px(theme.type_scale().caption()))
            .text_color(theme.dim_foreground())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .min_w(px(0.))
                    .gap(px(inset))
                    .children(self.leading),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .flex_shrink_0()
                    .gap(px(inset))
                    .children(self.trailing),
            )
    }
}
