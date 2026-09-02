//! The component kit.
//!
//! Every component here is a `RenderOnce` built from [`InteractiveSurface`] and
//! the spacing/typography tokens — never from hardcoded pixels or colours. That
//! is what makes a theme switch or an `omarchy display text size` change flow
//! through the whole UI with no per-component work.
//!
//! Deliberately small. `PLAN.md` §5 lists more, but a component only earns its
//! place once the explorer actually needs it; the rest arrive in M3–M6 with a
//! real call site to shape them.

use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, InteractiveElement as _, IntoElement,
    ParentElement, Pixels, Point, Render, RenderOnce, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled, Window, div, px,
};

use crate::{ActiveTheme as _, Chrome, InteractiveSurface, SurfaceState};

/// A framed container — the sidebar, the preview pane, a card.
#[derive(IntoElement)]
pub struct Panel {
    children: Vec<AnyElement>,
    padded: bool,
    filled: bool,
}

impl Default for Panel {
    fn default() -> Self {
        Self::new()
    }
}

impl Panel {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            padded: true,
            filled: true,
        }
    }

    /// Drop the internal padding — for a panel whose child manages its own, such
    /// as a scrolling list that should bleed to the edges.
    pub fn flush(mut self) -> Self {
        self.padded = false;
        self
    }

    /// Draw the border but not the surface fill.
    pub fn unfilled(mut self) -> Self {
        self.filled = false;
        self
    }
}

impl ParentElement for Panel {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Panel {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let mut panel = div()
            .flex()
            .flex_col()
            .gap(px(theme.space().panel_gap()))
            .rounded(px(theme.radius()))
            .border(px(theme.border_width().max(1.0)))
            .border_color(theme.border());

        if self.filled {
            panel = panel.bg(theme.surface());
        }
        if self.padded {
            panel = panel.p(px(theme.space().panel_padding()));
        }

        panel.children(self.children)
    }
}

/// The hover-group name every [`Row`] declares. A child styled with
/// `.group_hover(ROW_GROUP, …)` reacts to the pointer being anywhere on its
/// row, not just on itself.
pub const ROW_GROUP: &str = "row";

/// A list row — the primitive the file listing and the sidebar are built from.
///
/// Carries the full interaction vocabulary, because a file explorer needs all
/// of it: hover and the keyboard cursor read alike, the current directory stays
/// visibly selected, and the focused pane is distinguishable from the one that
/// merely has a cursor parked in it.
#[derive(IntoElement)]
pub struct Row {
    id: ElementId,
    children: Vec<AnyElement>,
    selected: bool,
    focused: bool,
    /// Rows are keyboard-driven, so the cursor is a first-class input rather
    /// than something derived from the pointer.
    cursor: bool,
    on_click: Option<ClickHandler>,
    /// Stored as an adapter — a function that applies `on_drag` to the built
    /// element — because `on_drag` is generic over the payload and the preview,
    /// and neither can be named in a struct field.
    drag: Option<ElementAdapter>,
    drop: Option<ElementAdapter>,
    right_click: Option<ElementAdapter>,
    /// Overrides the trailing inset. A row ending in a control that brings
    /// its own hit box (the tab list's close button) would otherwise show the
    /// row inset *plus* the box's, and read as a gap.
    padding_right: Option<f32>,
}

impl Row {
    /// `id` takes gpui's `ElementId`, so `("entry", index)` works directly for
    /// a list — which is the overwhelmingly common case here.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            children: Vec::new(),
            selected: false,
            focused: false,
            cursor: false,
            on_click: None,
            drag: None,
            drop: None,
            right_click: None,
            padding_right: None,
        }
    }

    /// A tighter trailing inset, for a row that ends in a control with its
    /// own hit box.
    pub fn padding_right(mut self, padding: f32) -> Self {
        self.padding_right = Some(padding);
        self
    }

    /// Make the row draggable, carrying `payload`.
    ///
    /// `preview` builds what follows the pointer. **gpui routes drops by the
    /// payload's type**, so give each draggable thing its own struct rather
    /// than reusing a tuple — otherwise two unrelated drags land in each
    /// other's drop handlers.
    pub fn draggable<T, W>(
        mut self,
        payload: T,
        preview: impl Fn(&T, Point<Pixels>, &mut Window, &mut App) -> gpui::Entity<W> + 'static,
    ) -> Self
    where
        T: 'static,
        W: Render,
    {
        self.drag = Some(Box::new(move |element| element.on_drag(payload, preview)));
        self
    }

    /// Accept a dragged `T` dropped onto this row.
    pub fn on_drop<T: 'static>(
        mut self,
        handler: impl Fn(&T, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.drop = Some(Box::new(move |element| element.on_drop(handler)));
        self
    }

    /// Activate on click. Keyboard remains the primary route — this is the
    /// discoverable one, not the only one.
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// React to a right click — the context-menu gesture.
    pub fn on_right_click(
        mut self,
        handler: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.right_click = Some(Box::new(move |element| {
            element.on_mouse_down(gpui::MouseButton::Right, handler)
        }));
        self
    }

    /// A persistent chosen state.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// The keyboard cursor. Renders as hover, by design.
    pub fn cursor(mut self, cursor: bool) -> Self {
        self.cursor = cursor;
        self
    }

    /// Whether the owning pane has focus.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }
}

impl ParentElement for Row {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Row {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let space = theme.space();

        // The cursor is folded into `hovered` on purpose: Omarchy treats mouse
        // hover and the panel keyboard cursor as one state.
        //
        // Borderless: a row is an item in a list, not a control. The state is
        // carried by the fill and by the step from secondary to primary text —
        // an outline around one line of a directory listing draws a box in the
        // middle of a column of names.
        // Square as well as borderless: a row is a slice of a list, and a
        // rounded fill draws a lozenge in the middle of a column of names.
        let surface = InteractiveSurface::from_flags(
            self.cursor,
            self.focused && self.cursor,
            self.selected,
            false,
        )
        .borderless()
        .square();

        let mut base = div()
            .id(self.id)
            // Every row is a hover group, so a child can reveal itself only
            // while the pointer is on its row (`.group_hover(ROW_GROUP, …)`) —
            // the tab list's close button is the first taker.
            .group(ROW_GROUP)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(space.control_gap()))
            .w_full()
            .h(px(space.control_height()))
            .px(px(space.row_padding_x()))
            .text_color(surface.text_color(theme));
        if let Some(padding) = self.padding_right {
            base = base.pr(px(padding));
        }

        // Children are attached *before* the interaction adapters. With them
        // attached afterwards, a row carrying an `on_click` collapsed to its
        // first child inside a `uniform_list` — the sidebar's plain flex column
        // was unaffected, which is what made it look like a click bug rather
        // than a layout one.
        // Pointer hover takes the text up with it as well as the fill, because
        // hover and the keyboard cursor are one state and the cursor row now
        // announces itself mostly through its text colour.
        let mut row = surface
            .paint(base, theme)
            .hover(|s| s.bg(theme.hover_fill()).text_color(theme.foreground()))
            .children(self.children);

        if let Some(apply) = self.drag {
            row = apply(row);
        }
        if let Some(apply) = self.drop {
            row = apply(row);
        }
        if let Some(apply) = self.right_click {
            row = apply(row);
        }
        if let Some(handler) = self.on_click {
            row = row.cursor_pointer().on_click(handler);
        }
        row
    }
}

/// A click handler, in gpui's own shape so `cx.listener(…)` composes directly.
///
/// Named because the boxed type is unreadable inline and every future
/// interactive component wants the same signature.
type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// A small action button: a glyph, a label, or both.
///
/// The one primitive behind every small clickable verb in the chrome — the
/// navigation bar's arrows, the workspace headers' `+` and `…`, the detail
/// panel's entry actions, the status bar's directory actions and badges, the
/// preview's expand and collapse. One primitive means one geometry, one hover,
/// one pressed state and one disabled treatment, so the chrome is uniform by
/// construction rather than by diligence.
///
/// The grammar is [`Row`]'s, not [`Button`]'s: quiet — no idle chrome — with
/// the state carried by the fill and by the step from secondary to primary
/// text. A bar full of bordered buttons reads as a form; a bar of quiet verbs
/// reads as chrome. `Button` remains for the emphatic case (a dialog's
/// confirm), where the outline is what says "control".
///
/// Geometry: `control-height` tall always; a glyph alone gets a square, a
/// label gets `control-padding-x`. Labels sit at caption size — these are
/// annotations on the chrome, not content.
#[derive(IntoElement)]
pub struct ActionButton {
    id: ElementId,
    glyph: Option<SharedString>,
    label: Option<SharedString>,
    /// Extra content after the label — the git badge's counts, for instance.
    children: Vec<AnyElement>,
    enabled: bool,
    /// Carries the accent — a state badge that is currently "on".
    accent: bool,
    on_click: Option<ClickHandler>,
}

impl ActionButton {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            glyph: None,
            label: None,
            children: Vec::new(),
            enabled: true,
            accent: false,
            on_click: None,
        }
    }

    pub fn glyph(mut self, glyph: impl Into<SharedString>) -> Self {
        self.glyph = Some(glyph.into());
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// A disabled action looks unavailable rather than merely doing nothing
    /// when pressed.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Draw in the accent — for a badge whose state is "on" (`accent` is
    /// scarce; one such badge per bar at most).
    pub fn accent(mut self, accent: bool) -> Self {
        self.accent = accent;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl ParentElement for ActionButton {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for ActionButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let space = theme.space();
        // A step below control-height: these are chrome verbs, not form
        // controls, and matching the full control size made them compete with
        // the content around them. Derived from the scale so it tracks the
        // user's text size.
        let size = space.control_height() - space.md();
        let square = self.label.is_none() && self.children.is_empty();

        // Two roles inside one button: the glyph is always the *secondary*
        // colour — it decorates the verb rather than being it — and the label
        // carries the readable colour (foreground, or the accent for a badge
        // that is "on"). Disabled fades both.
        let (glyph_color, label_color) = if !self.enabled {
            let faded = theme.dim_foreground().opacity(0.5);
            (faded, faded)
        } else if self.accent {
            (theme.dim_foreground(), theme.accent())
        } else {
            (theme.dim_foreground(), theme.foreground())
        };
        let caption = theme.type_scale().caption();

        let button = div()
            .id(self.id)
            .flex()
            .flex_row()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .h(px(size))
            .gap(px(space.control_gap()))
            // Everything inside is caption-sized — a chrome verb is an
            // annotation on the window, not content.
            .text_size(px(caption))
            // Tighter than the window radius: a small element with the full
            // corner radius reads as a pill. Capped rather than fixed so a
            // square-cornered theme (rounding 0) stays square.
            .rounded(px(theme.radius().min(2.0)))
            // A really subtle idle border — the separator's exact colour, so
            // every hairline in the chrome sits at one strength: enough of an
            // edge for the verb to sit in, quiet enough that a bar of them
            // still reads as chrome rather than a form.
            .border(px(theme.border_width().max(1.0)))
            .border_color(theme.border().opacity(0.2))
            .text_color(label_color);
        let button = if square {
            button.w(px(size))
        } else {
            // md rather than control-padding-x: these live in dense bars and
            // toolbars, and the full control padding makes three of them
            // overflow the detail panel.
            button.px(px(space.md()))
        };
        let mut button = button
            .children(self.glyph.map(|glyph| {
                div()
                    .text_color(glyph_color)
                    .child(glyph)
                    .into_any_element()
            }))
            .children(self.label.map(|label| label.into_any_element()))
            .children(self.children);

        if self.enabled {
            let (hover, pressed) = (theme.hover_fill(), theme.pressed_fill());
            // The fill carries the hover; the colours stay in their roles.
            button = button
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .active(move |s| s.bg(pressed));
            if let Some(handler) = self.on_click {
                button = button.on_click(handler);
            }
        }
        button
    }
}

/// Applies drag or drop wiring to a built row.
///
/// `on_drag`/`on_drop` are generic over the payload, which cannot be named in a
/// struct field — so the builder stores the *application* of the call instead.
type ElementAdapter = Box<dyn FnOnce(Stateful<Div>) -> Stateful<Div>>;

/// What a button is for, which decides how loudly it draws itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonKind {
    /// Bordered chrome. The default, and what most buttons should be.
    #[default]
    Normal,
    /// Carries the accent — one per view at most.
    Primary,
    /// No chrome until interacted with, for dense toolbars.
    Ghost,
    /// Destructive. Uses `urgent`, which Omarchy sources from `red`.
    Danger,
}

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    kind: ButtonKind,
    active: bool,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: ButtonKind::default(),
            active: false,
            on_click: None,
        }
    }

    pub fn kind(mut self, kind: ButtonKind) -> Self {
        self.kind = kind;
        self
    }

    /// A toggle that is currently on — renders as selected.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let space = theme.space();

        let chrome = match self.kind {
            ButtonKind::Ghost => Chrome::Quiet,
            _ => Chrome::Always,
        };
        let state = if self.active {
            SurfaceState::Selected
        } else {
            SurfaceState::Normal
        };

        let text = match self.kind {
            ButtonKind::Primary => theme.accent(),
            ButtonKind::Danger => theme.urgent(),
            _ if self.active => theme.bright_foreground(),
            _ => theme.foreground(),
        };

        let base = div()
            .id(self.id)
            .flex()
            .items_center()
            .justify_center()
            .h(px(space.control_height()))
            .px(px(space.control_padding_x()))
            .text_size(px(theme.type_scale().body()))
            .text_color(text)
            .cursor_pointer();

        let mut button = InteractiveSurface::new(state)
            .chrome(chrome)
            .paint(base, theme)
            .hover(|s| s.bg(theme.hover_fill()))
            .active(|s| s.bg(theme.pressed_fill()))
            .child(self.label);

        if let Some(handler) = self.on_click {
            button = button.on_click(handler);
        }
        button
    }
}

/// A hairline rule. Uses the scaled hairline so it stays 1 device pixel at
/// small text sizes and thickens with the rest of the UI at large ones.
#[derive(IntoElement, Default)]
pub struct Separator {
    vertical: bool,
    subtle: bool,
}

impl Separator {
    pub fn horizontal() -> Self {
        Self {
            vertical: false,
            subtle: false,
        }
    }

    pub fn vertical() -> Self {
        Self {
            vertical: true,
            subtle: false,
        }
    }

    /// The secondary strength — for rules *inside* a group, like the ones
    /// between a menu's rows, which should whisper under the primary rule
    /// that frames the group.
    pub fn subtle(mut self) -> Self {
        self.subtle = true;
        self
    }
}

impl RenderOnce for Separator {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let thickness = px(theme.space().hairline());
        // Far fainter than a control border: a rule only has to *divide*, and
        // at full border strength the pane edges competed with the content.
        // The subtle variant halves that again.
        let alpha = if self.subtle { 0.1 } else { 0.2 };
        let rule = div().bg(theme.border().opacity(alpha));
        if self.vertical {
            rule.w(thickness).h_full()
        } else {
            rule.h(thickness).w_full()
        }
    }
}

/// A small uppercase label above a group — "PLACES", "PINNED".
#[derive(IntoElement)]
pub struct SectionHeader {
    label: SharedString,
}

impl SectionHeader {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

impl RenderOnce for SectionHeader {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px(px(theme.space().row_padding_x()))
            .pt(px(theme.space().sm()))
            .pb(px(theme.space().xxs()))
            .text_size(px(theme.type_scale().caption()))
            // On a filled panel, secondary text needs the panel's colour to
            // measure against, not the window's.
            .text_color(theme.dim_foreground_on(theme.tokens.palette.lighter_background()))
            .child(self.label.to_uppercase())
    }
}

/// A status pill — the HTTP server's state, a file count, a warning.
#[derive(IntoElement)]
pub struct Badge {
    label: SharedString,
    tone: BadgeTone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BadgeTone {
    #[default]
    Neutral,
    Accent,
    Urgent,
}

impl Badge {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            tone: BadgeTone::default(),
        }
    }

    pub fn accent(mut self) -> Self {
        self.tone = BadgeTone::Accent;
        self
    }

    pub fn urgent(mut self) -> Self {
        self.tone = BadgeTone::Urgent;
        self
    }
}

impl RenderOnce for Badge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let color = match self.tone {
            BadgeTone::Neutral => theme.dim_foreground(),
            BadgeTone::Accent => theme.accent(),
            BadgeTone::Urgent => theme.urgent(),
        };

        div()
            .flex()
            .items_center()
            .px(px(theme.space().sm()))
            .py(px(theme.space().xxs()))
            .rounded(px(theme.radius()))
            .border(px(theme.border_width().max(1.0)))
            .border_color(color.opacity(0.4))
            .text_size(px(theme.type_scale().caption()))
            .text_color(color)
            .child(self.label)
    }
}

/// A `key  description` pair for the hint bar. The whole app is
/// shortcut-oriented, so this is load-bearing rather than decorative.
#[derive(IntoElement)]
pub struct KeyHint {
    key: SharedString,
    action: SharedString,
}

impl KeyHint {
    pub fn new(key: impl Into<SharedString>, action: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            action: action.into(),
        }
    }
}

impl RenderOnce for KeyHint {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme.space().label_gap()))
            .text_size(px(theme.type_scale().caption()))
            .child(
                div()
                    .px(px(theme.space().xs()))
                    .rounded(px(theme.radius()))
                    .bg(theme.normal_fill())
                    .text_color(theme.bright_foreground())
                    .child(self.key),
            )
            .child(div().text_color(theme.dim_foreground()).child(self.action))
    }
}

/// The current path, one segment per component.
#[derive(IntoElement)]
pub struct Breadcrumb {
    segments: Vec<SharedString>,
}

impl Breadcrumb {
    pub fn new(segments: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        Self {
            segments: segments.into_iter().map(Into::into).collect(),
        }
    }
}

impl RenderOnce for Breadcrumb {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let last = self.segments.len().saturating_sub(1);
        let separator = theme.dim_foreground();

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme.space().xs()))
            .text_size(px(theme.type_scale().body()))
            .children(
                self.segments
                    .into_iter()
                    .enumerate()
                    .flat_map(move |(index, segment)| {
                        // Only the leaf is emphasised; ancestors recede.
                        let color = if index == last {
                            theme.bright_foreground()
                        } else {
                            theme.dim_foreground()
                        };
                        let mut parts = Vec::new();
                        if index > 0 {
                            parts.push(div().text_color(separator).child("/").into_any_element());
                        }
                        parts.push(div().text_color(color).child(segment).into_any_element());
                        parts
                    }),
            )
    }
}
