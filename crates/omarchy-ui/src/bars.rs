//! The bars: one over each panel, one under the window.
//!
//! Every top bar is the same height ([`crate::Theme::bar_height`]) so the rule
//! under each meets the others' across the vertical dividers, and every bar
//! keeps one rhythm — the small inset on every side and between items — so
//! the chrome is one number rather than a set of them.
//!
//! A top bar does not sit *above* its panel's body but *over* it: the body
//! scrolls under the bar and shows through its veil. [`Headed`] is that
//! arrangement.

use gpui::{
    AnyElement, App, Div, InteractiveElement as _, IntoElement, ParentElement, RenderOnce, Styled,
    Window, div, linear_color_stop, linear_gradient, px,
};

use crate::{ActiveTheme as _, ColumnHeader, Frosted, Separator, Theme};

/// A flexible gap between a bar's leading and trailing items.
pub fn spacer() -> Div {
    div().flex_1()
}

/// The air between a bar's edge and its first or last item. The items are
/// [`crate::ActionButton`]s, shorter than the bar's slot by `md` and
/// centred in it, so the air they show above and below is the inset plus
/// half of that; the sides match it, so the button sits as far from the
/// panel's edge as from the bar's.
fn edge_inset(theme: &Theme) -> f32 {
    let space = theme.space();
    space.sm() + space.md() / 2.0
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
            .py(px(inset))
            .px(px(edge_inset(theme)))
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
        // One value everywhere: the same small inset above and below and
        // between items. The sides differ: the verbs on the right get the
        // edge inset every bar's items get, and the facts on the left the
        // panel inset, so they do not hug the window edge.
        let inset = space.sm();
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(inset))
            .py(px(inset))
            .pr(px(edge_inset(theme)))
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

/// One of the things a [`Headed`] stacks over its body, by kind, because
/// the kind fixes the height and the height fixes where the body starts.
enum Head {
    /// A [`Bar`]: [`Theme::bar_height`] tall.
    Bar(AnyElement),
    /// A [`ColumnHeader`]: a row tall.
    Columns(AnyElement),
}

impl Head {
    fn height(&self, theme: &Theme) -> f32 {
        match self {
            Self::Bar(_) => theme.bar_height(),
            Self::Columns(_) => ColumnHeader::height(theme),
        }
    }

    fn into_element(self) -> AnyElement {
        match self {
            Self::Bar(element) | Self::Columns(element) => element,
        }
    }
}

/// A bar over the body it heads, not above it. The body fills the whole
/// region, so what it scrolls passes under the bar, and the bar is a veil
/// ([`Theme::bar_veil`]) rather than solid ground, so the rows beneath show
/// through it slightly.
///
/// gpui cannot blur what is behind an element — nothing in its scene can
/// sample what was painted under it — so this is a veil, not frosted glass.
/// What sits behind the *window* is the compositor's to blur.
///
/// Under the last head is a hairline, as under any bar, or with
/// [`Headed::faded`] a short gradient from the veil to nothing, so the
/// content surfaces progressively rather than at a line. A rule reads as
/// an edge; the fade suits a head that should not look like one.
///
/// The body has to start clear of the heads, and only it can arrange that:
/// the padding must be *inside* the scrolling element, or the content would
/// stop at the bar instead of passing under it. So the scrolling element
/// pads its top by [`Headed::inset`], and [`Headed::body`] takes it as is.
/// A body that does not scroll — an empty state — goes in with
/// [`Headed::body_below`], which pads it here.
///
/// ```ignore
/// let headed = Headed::new()
///     .bar(self.nav_bar(cx))
///     .columns(self.listing_header(cx));
/// let inset = headed.inset(cx.theme());
/// headed.body(ScrollArea::new(&self.scroll).child(list.pt(px(inset))))
/// ```
#[derive(IntoElement, Default)]
pub struct Headed {
    heads: Vec<Head>,
    faded: bool,
    body: Option<AnyElement>,
    /// The body was given by [`Headed::body_below`]: pad it clear of the
    /// heads rather than trust it to.
    below: bool,
}

impl Headed {
    pub fn new() -> Self {
        Self::default()
    }

    /// A [`Bar`], stacked under the heads so far.
    pub fn bar(mut self, bar: impl IntoElement) -> Self {
        self.heads.push(Head::Bar(bar.into_any_element()));
        self
    }

    /// A [`ColumnHeader`], stacked under the heads so far.
    pub fn columns(mut self, header: ColumnHeader) -> Self {
        self.heads.push(Head::Columns(header.into_any_element()));
        self
    }

    /// No rule under the last head: a fade instead.
    pub fn faded(mut self) -> Self {
        self.faded = true;
        self
    }

    /// Where the body's content starts: the heads, the rules between them,
    /// and the rule under the last one when there is one. A fade is not
    /// counted — it lies over the content, which is the point of it.
    pub fn inset(&self, theme: &Theme) -> f32 {
        let hairline = theme.space().hairline();
        let last = self.heads.len().saturating_sub(1);
        self.heads
            .iter()
            .enumerate()
            .map(|(index, head)| {
                let ruled = index < last || !self.faded;
                head.height(theme) + if ruled { hairline } else { 0.0 }
            })
            .sum()
    }

    /// The body, filling the region under the heads; its scrolling element
    /// has already padded its top by [`Headed::inset`].
    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body = Some(body.into_any_element());
        self.below = false;
        self
    }

    /// A body that does not scroll, placed below the heads rather than
    /// under them.
    pub fn body_below(mut self, body: impl IntoElement) -> Self {
        self.body = Some(body.into_any_element());
        self.below = true;
        self
    }
}

impl RenderOnce for Headed {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let veil = theme.bar_veil();
        // Deep enough to read as a gradient rather than a soft rule, and
        // still within a row's top padding, so the first row at rest is
        // dimmed only where it has nothing to show.
        let fade = theme.space().xxl();
        let inset = self.inset(theme);
        let last = self.heads.len().saturating_sub(1);

        // The veiled stack takes the pointer, so a row half under a bar
        // cannot light up or be clicked through it — but the wheel still
        // reaches the body, since scrolling from the bar is what scrolling
        // under it invites.
        let mut stack = div().flex().flex_col().block_mouse_except_scroll();
        for (index, head) in self.heads.into_iter().enumerate() {
            stack = stack.child(head.into_element());
            if index < last || !self.faded {
                stack = stack.child(Separator::horizontal());
            }
        }
        // The veil is frosted: the fill, then a grain over it, then the
        // heads. gpui cannot blur the rows underneath, and the grain is
        // what stops them reading sharp through the veil.
        let mut overlay = div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .flex()
            .flex_col()
            .child(Frosted::new(stack).fill(veil));
        if self.faded {
            overlay = overlay.child(div().h(px(fade)).w_full().bg(linear_gradient(
                180.,
                linear_color_stop(veil, 0.),
                linear_color_stop(veil.opacity(0.), 1.),
            )));
        }

        let mut region = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.));
        if let Some(body) = self.body {
            region = if self.below {
                region.child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.))
                        .pt(px(inset))
                        .child(body),
                )
            } else {
                region.child(body)
            };
        }
        region.child(overlay)
    }
}
