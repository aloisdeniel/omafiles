//! The three-column shell: a side panel, the content, a side panel — each
//! with its own bar on top — a status bar under them, and an overlay layer
//! over everything.
//!
//! The panels collapse to a strip one button wide, resize by dragging the
//! rule beside them, and in a window too narrow to dock them float over the
//! content instead. All of that state lives in [`Panels`], a gpui entity the
//! app holds, so the app persists what it wants (the widths) and the
//! [`Workbench`] does the rest.
//!
//! ```ignore
//! impl Render for Root {
//!     fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
//!         Workbench::new(&self.panels)
//!             .left(SidePanel::new(self.sidebar(cx)).bar([self.sidebar_tools(cx)]))
//!             .center(self.listing(cx))
//!             .right(SidePanel::new(self.detail(cx)).bar([self.detail_verbs(cx)]))
//!             .status(self.status_bar(cx))
//!             .overlay(self.modal(cx))
//!     }
//! }
//! ```

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement, RenderOnce, StatefulInteractiveElement as _, Styled, Window, div,
    px,
};

use crate::{ActiveTheme as _, Bar, QuietButton, Separator, Theme};

/// The two side panels, as the things that can be opened, closed and
/// resized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelSide {
    Left,
    Right,
}

impl PanelSide {
    fn other(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

/// A panel edge being dragged: which one, where the pointer started, and
/// how wide the panel was then.
#[derive(Debug, Clone, Copy)]
struct PanelResize {
    side: PanelSide,
    start_x: f32,
    start_width: f32,
}

/// The share of the window the centre column always keeps: however wide
/// the panels are dragged, the content stays at least this fraction. Below
/// it the content would be a sliver, and the panels are about the content.
const CENTER_MIN_FRACTION: f32 = 0.3;

/// The narrowest a panel goes, as a share of `dropdown-width`. Half the
/// default keeps every row's icon and a few characters of its label; below
/// that the panel says nothing, and collapsing it is the honest gesture.
const PANEL_MIN_FACTOR: f32 = 0.5;

/// What [`Panels`] tells its subscribers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelsEvent {
    /// A drag ended and the panel landed on this width. The moment to write
    /// it down, so the next session opens with it.
    Resized { side: PanelSide, width: u32 },
}

/// The side panels' state: open or collapsed, docked or floating, their
/// widths, and the edge under the pointer while one is being dragged.
///
/// A gpui entity, so the [`Workbench`]'s own controls — the collapse and
/// expand buttons, the resize grips, the scrim behind a floating panel —
/// update it directly and the app only observes:
///
/// ```ignore
/// let panels = cx.new(|_| Panels::new().with_widths(config.sidebar_width, config.detail_width));
/// cx.observe(&panels, |_, _, cx| cx.notify()).detach();
/// cx.subscribe(&panels, |this, _, event, cx| match event {
///     PanelsEvent::Resized { side, width } => this.save_width(*side, *width, cx),
/// })
/// .detach();
/// ```
///
/// Open and closed are *user intent*: a narrow window overrides them at
/// render time without forgetting what was asked for, so widening the
/// window brings the panels back rather than leaving them shut.
#[derive(Debug, Clone)]
pub struct Panels {
    left_open: bool,
    right_open: bool,
    /// The panel floating over the content while the window is too narrow
    /// to dock one — at most one, opened from its strip and dismissed by a
    /// click beside it. Separate from the docked intent on purpose: the
    /// dismissing click must not close a panel the wider window would have
    /// shown.
    floating: Option<PanelSide>,
    resizing: Option<PanelResize>,
    /// The window's width as of the last frame, so widths can be clamped
    /// against it from code that only has the app context.
    viewport_width: f32,
    left_width: Option<u32>,
    right_width: Option<u32>,
}

impl Default for Panels {
    fn default() -> Self {
        Self::new()
    }
}

impl EventEmitter<PanelsEvent> for Panels {}

impl Panels {
    /// Both panels open, at the theme's default width.
    pub fn new() -> Self {
        Self {
            left_open: true,
            right_open: true,
            floating: None,
            resizing: None,
            viewport_width: 0.0,
            left_width: None,
            right_width: None,
        }
    }

    /// The widths a previous session landed on. `None` is the theme's
    /// `dropdown-width`, so a fresh install still follows the token scale.
    pub fn with_widths(mut self, left: Option<u32>, right: Option<u32>) -> Self {
        self.left_width = left;
        self.right_width = right;
        self
    }

    /// Start with a panel collapsed.
    pub fn with_open(mut self, side: PanelSide, open: bool) -> Self {
        self.set_open(side, open);
        self
    }

    /// The width the user asked for, if they ever dragged this panel.
    pub fn asked_width(&self, side: PanelSide) -> Option<u32> {
        match side {
            PanelSide::Left => self.left_width,
            PanelSide::Right => self.right_width,
        }
    }

    pub fn set_width(&mut self, side: PanelSide, width: Option<u32>) {
        match side {
            PanelSide::Left => self.left_width = width,
            PanelSide::Right => self.right_width = width,
        }
    }

    /// The docked intent: whether the panel is open in a wide window.
    pub fn is_open(&self, side: PanelSide) -> bool {
        match side {
            PanelSide::Left => self.left_open,
            PanelSide::Right => self.right_open,
        }
    }

    pub fn set_open(&mut self, side: PanelSide, open: bool) {
        match side {
            PanelSide::Left => self.left_open = open,
            PanelSide::Right => self.right_open = open,
        }
    }

    /// The panel floating over the content, in a narrow window.
    pub fn floating(&self) -> Option<PanelSide> {
        self.floating
    }

    /// The panel edge under the pointer, while one is being dragged.
    pub fn resizing(&self) -> Option<PanelSide> {
        self.resizing.map(|r| r.side)
    }

    /// Called by the [`Workbench`] at the top of every frame: records the
    /// window's width, and drops a float once the window is wide enough to
    /// dock again, so the next narrowing starts clean.
    pub fn begin_frame(&mut self, viewport_width: f32, theme: &Theme) {
        self.viewport_width = viewport_width;
        if !self.narrow(theme) {
            self.floating = None;
        }
    }

    /// Too narrow to dock the panels: two of them plus content at least as
    /// wide as one. Derived from the token scale rather than a magic
    /// number, so it tracks the user's text size — at a larger `base-size`
    /// the panels need more room and should give up sooner.
    pub fn narrow(&self, theme: &Theme) -> bool {
        self.viewport_width < theme.space().dropdown_width() * 3.0
    }

    /// Docked and open: taking space beside the content this frame.
    pub fn docked(&self, side: PanelSide, theme: &Theme) -> bool {
        !self.narrow(theme) && self.is_open(side)
    }

    /// On screen: floating in a narrow window, docked otherwise.
    pub fn shown(&self, side: PanelSide, theme: &Theme) -> bool {
        if self.narrow(theme) {
            self.floating == Some(side)
        } else {
            self.is_open(side)
        }
    }

    /// A strip's expand, or the key: float the panel in a narrow window,
    /// dock it in a wide one.
    pub fn open(&mut self, side: PanelSide, cx: &mut Context<Self>) {
        if self.narrow(cx.theme()) {
            self.floating = Some(side);
        } else {
            self.set_open(side, true);
        }
        cx.notify();
    }

    /// A bar's collapse, a scrim click, or the key. In a narrow window only
    /// the float goes; the docked intent is untouched.
    pub fn close(&mut self, side: PanelSide, cx: &mut Context<Self>) {
        if self.narrow(cx.theme()) {
            if self.floating == Some(side) {
                self.floating = None;
            }
        } else {
            self.set_open(side, false);
        }
        cx.notify();
    }

    pub fn toggle(&mut self, side: PanelSide, cx: &mut Context<Self>) {
        if self.shown(side, cx.theme()) {
            self.close(side, cx);
        } else {
            self.open(side, cx);
        }
    }

    /// The floor every panel keeps, and the width the two panels may share
    /// in this window once the centre column has its minimum.
    fn bounds(&self, theme: &Theme) -> (f32, f32) {
        let default = theme.space().dropdown_width();
        let floor = (default * PANEL_MIN_FACTOR).round();
        let room = (self.viewport_width * (1.0 - CENTER_MIN_FRACTION)).round();
        (floor, room)
    }

    /// A panel's width for this frame: the one the user dragged it to, or
    /// the theme's `dropdown-width` until they have, kept between the floor
    /// and whatever the centre column can spare beside the other panel.
    pub fn width(&self, side: PanelSide, theme: &Theme) -> f32 {
        self.width_beside(side, self.docked(side.other(), theme), theme)
    }

    /// [`width`](Self::width), told explicitly whether the other panel is
    /// docked beside this one — for the frame where the content has taken
    /// the other panel's place.
    pub fn width_beside(&self, side: PanelSide, neighbour: bool, theme: &Theme) -> f32 {
        let default = theme.space().dropdown_width();
        let (floor, room) = self.bounds(theme);
        let simple = |asked: Option<u32>| {
            asked
                .map_or(default, |w| w as f32)
                .clamp(floor, room.max(floor))
        };
        // The other panel, at its own simple clamp, is what this one has to
        // fit beside. One-sided on purpose: two panels each clamped against
        // the other's clamped width would chase in circles.
        let other = if neighbour {
            simple(self.asked_width(side.other()))
        } else {
            0.0
        };
        self.asked_width(side)
            .map_or(default, |w| w as f32)
            .clamp(floor, (room - other).max(floor))
    }

    /// Mouse-down on a panel's grip.
    pub fn start_resize(&mut self, side: PanelSide, x: f32, cx: &mut Context<Self>) {
        let start_width = self.width(side, cx.theme());
        self.resizing = Some(PanelResize {
            side,
            start_x: x,
            start_width,
        });
        cx.notify();
    }

    /// Follow the pointer: the panel is as wide as it was at mouse-down
    /// plus how far the pointer has travelled toward the window's centre.
    /// Stored already clamped, so a drag past the limit does not bank an
    /// invisible surplus that the next drag has to burn through; the clamp
    /// against the *other* panel happens at render, so a wider window gives
    /// the asked-for width back.
    pub fn drag_to(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some(resize) = self.resizing else {
            return;
        };
        let travel = x - resize.start_x;
        let width = match resize.side {
            PanelSide::Left => resize.start_width + travel,
            PanelSide::Right => resize.start_width - travel,
        };
        let (floor, room) = self.bounds(cx.theme());
        let width = width.clamp(floor, room.max(floor)).round() as u32;
        if self.asked_width(resize.side) != Some(width) {
            self.set_width(resize.side, Some(width));
            cx.notify();
        }
    }

    /// Let go: the width the panel landed on is announced as a
    /// [`PanelsEvent::Resized`], for the app to write down.
    pub fn end_resize(&mut self, cx: &mut Context<Self>) {
        let Some(resize) = self.resizing.take() else {
            return;
        };
        if let Some(width) = self.asked_width(resize.side) {
            cx.emit(PanelsEvent::Resized {
                side: resize.side,
                width,
            });
        }
        cx.notify();
    }
}

/// What a side panel holds: the items of its bar, and its body.
///
/// The bar gets the panel's collapse button appended by the [`Workbench`];
/// put a [`crate::spacer`] last to push it to the far edge, or let the
/// items flow if the panel's verbs already fill the bar.
pub struct SidePanel {
    bar: Vec<AnyElement>,
    body: AnyElement,
}

impl SidePanel {
    pub fn new(body: impl IntoElement) -> Self {
        Self {
            bar: Vec::new(),
            body: body.into_any_element(),
        }
    }

    /// The bar's items, left to right.
    pub fn bar(mut self, items: impl IntoIterator<Item = AnyElement>) -> Self {
        self.bar.extend(items);
        self
    }
}

/// The shell. See the module docs.
///
/// Renders the outermost container, and that is load-bearing: the overlay
/// is a *sibling* of the content column rather than inside it, so the
/// typography lives on the container for both to inherit, and a resize
/// drag is tracked on the container so it keeps following the pointer once
/// it has left the hairline it started on.
///
/// Action handlers belong on a `div` *around* the workbench: gpui
/// dispatches an action up the focus path, and a handler on the content
/// column is unreachable the moment a modal's text field takes focus.
#[derive(IntoElement)]
pub struct Workbench {
    panels: Entity<Panels>,
    left: Option<SidePanel>,
    right: Option<SidePanel>,
    center: Option<AnyElement>,
    status: Option<AnyElement>,
    overlay: Option<AnyElement>,
    key_context: Option<&'static str>,
    focus: Option<FocusHandle>,
}

impl Workbench {
    pub fn new(panels: &Entity<Panels>) -> Self {
        Self {
            panels: panels.clone(),
            left: None,
            right: None,
            center: None,
            status: None,
            overlay: None,
            key_context: None,
            focus: None,
        }
    }

    pub fn left(mut self, panel: SidePanel) -> Self {
        self.left = Some(panel);
        self
    }

    pub fn right(mut self, panel: SidePanel) -> Self {
        self.right = Some(panel);
        self
    }

    /// A panel, or not — for a frame where the content has taken its place.
    pub fn right_if(mut self, panel: Option<SidePanel>) -> Self {
        self.right = panel;
        self
    }

    /// The centre column. Brings its own bar, if it wants one.
    pub fn center(mut self, content: impl IntoElement) -> Self {
        self.center = Some(content.into_any_element());
        self
    }

    /// The bar under the window — a [`crate::StatusBar`], typically.
    pub fn status(mut self, status: impl IntoElement) -> Self {
        self.status = Some(status.into_any_element());
        self
    }

    /// The modal layer, when something is open.
    pub fn overlay(mut self, overlay: Option<AnyElement>) -> Self {
        self.overlay = overlay;
        self
    }

    /// The key context and focus handle of the content column, so the
    /// app's bindings resolve while nothing more specific has focus.
    pub fn focus(mut self, key_context: &'static str, focus: &FocusHandle) -> Self {
        self.key_context = Some(key_context);
        self.focus = Some(focus.clone());
        self
    }
}

impl RenderOnce for Workbench {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let viewport_width = f32::from(window.viewport_size().width);
        self.panels.update(cx, |panels, cx| {
            panels.begin_frame(viewport_width, cx.theme())
        });

        let (background, foreground, family, body, scrim) = {
            let theme = cx.theme();
            (
                theme.window_background(),
                theme.foreground(),
                theme.type_scale().family.clone(),
                theme.type_scale().body(),
                theme.scrim(0.6),
            )
        };
        let (narrow, left_docked, right_docked, floating, left_width, right_width) = {
            let panels = self.panels.read(cx);
            let theme = cx.theme();
            let left_docked = self.left.is_some() && panels.docked(PanelSide::Left, theme);
            let right_docked = self.right.is_some() && panels.docked(PanelSide::Right, theme);
            (
                panels.narrow(theme),
                left_docked,
                right_docked,
                panels.floating(),
                panels.width_beside(PanelSide::Left, right_docked, theme),
                panels.width_beside(PanelSide::Right, left_docked, theme),
            )
        };

        // The row of columns. The rule beside a docked panel is also its
        // grip; a strip's rule is only a rule, since a collapsed panel has
        // no width to drag. Each panel carries its own bar, so the top of
        // the window splits where the panes do.
        let mut row = div().flex().flex_row().flex_1().min_h(px(0.));
        let mut floats = Vec::new();
        if let Some(left) = self.left {
            if left_docked {
                row = row
                    .child(column(PanelSide::Left, left, left_width, &self.panels, cx))
                    .child(resize_handle(PanelSide::Left, &self.panels, cx));
            } else {
                if narrow && floating == Some(PanelSide::Left) {
                    floats.push(float(
                        PanelSide::Left,
                        left,
                        left_width,
                        scrim,
                        &self.panels,
                        cx,
                    ));
                }
                row = row
                    .child(strip(PanelSide::Left, &self.panels, cx))
                    .child(Separator::vertical());
            }
        }
        let mut content = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .children(self.center);
        if let Some(right) = self.right {
            if right_docked {
                content = content
                    .child(resize_handle(PanelSide::Right, &self.panels, cx))
                    .child(column(
                        PanelSide::Right,
                        right,
                        right_width,
                        &self.panels,
                        cx,
                    ));
            } else {
                if narrow && floating == Some(PanelSide::Right) {
                    floats.push(float(
                        PanelSide::Right,
                        right,
                        right_width,
                        scrim,
                        &self.panels,
                        cx,
                    ));
                }
                content = content.child(Separator::vertical()).child(strip(
                    PanelSide::Right,
                    &self.panels,
                    cx,
                ));
            }
        }
        row = row.child(content);

        let mut column = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(background)
            .text_color(foreground)
            .font_family(family.clone())
            .text_size(px(body))
            // No padding or gap on the shell: the rules must reach the
            // window edges and sit flush against the bars they divide.
            // Every region supplies its own inner spacing.
            .child(row);
        if let Some(key_context) = self.key_context {
            column = column.key_context(key_context);
        }
        if let Some(focus) = &self.focus {
            column = column.track_focus(focus);
        }
        if let Some(status) = self.status {
            column = column.child(Separator::horizontal()).child(status);
        }

        let panels = self.panels;
        let end = {
            let panels = panels.clone();
            move |cx: &mut App| panels.update(cx, |p, cx| p.end_resize(cx))
        };
        let end_up = end.clone();
        let end_out = end.clone();

        // Outermost is a plain positioned container, not the flex column:
        // an absolutely-positioned overlay that is also a flex *item* still
        // gets sized by flex, which left the scrim covering only a band.
        div()
            .relative()
            .size_full()
            .font_family(family)
            .text_size(px(body))
            .text_color(foreground)
            // A panel resize is tracked here, on the container that spans
            // the window, so the drag keeps following the pointer once it
            // has left the hairline it started on.
            .on_mouse_move(move |event: &gpui::MouseMoveEvent, _window, cx| {
                if panels.read(cx).resizing.is_none() {
                    return;
                }
                // The button came up somewhere the window did not see it.
                if event.pressed_button != Some(gpui::MouseButton::Left) {
                    end(cx);
                    return;
                }
                let x = f32::from(event.position.x);
                panels.update(cx, |p, cx| p.drag_to(x, cx));
            })
            .on_mouse_up(gpui::MouseButton::Left, move |_e, _window, cx| end_up(cx))
            .on_mouse_up_out(gpui::MouseButton::Left, move |_e, _window, cx| end_out(cx))
            .child(column)
            .children(floats)
            .children(self.overlay)
    }
}

/// A docked or floating panel: its bar, a rule, its body.
fn column(
    side: PanelSide,
    panel: SidePanel,
    width: f32,
    panels: &Entity<Panels>,
    _cx: &mut App,
) -> AnyElement {
    let (id, glyph) = match side {
        PanelSide::Left => ("panel-collapse-left", "\u{f100}"), // nf-fa-angle_double_left
        PanelSide::Right => ("panel-collapse-right", "\u{f101}"), // nf-fa-angle_double_right
    };
    // Borderless, at the far right: a control about the panel itself, not
    // one of its tools, so it must not read as one more of them.
    let collapse = QuietButton::new(id, glyph).on_click({
        let panels = panels.clone();
        move |_e, _w, cx| panels.update(cx, |p, cx| p.close(side, cx))
    });
    div()
        .flex()
        .flex_col()
        .w(px(width))
        .flex_shrink_0()
        .h_full()
        .child(Bar::new().children(panel.bar).child(collapse))
        .child(Separator::horizontal())
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.))
                .child(panel.body),
        )
        .into_any_element()
}

/// What stands in for a collapsed panel: a strip one button wide, holding
/// only the way to expand it again, under a bar-height header so its rule
/// meets the neighbours'.
fn strip(side: PanelSide, panels: &Entity<Panels>, cx: &mut App) -> AnyElement {
    let theme = cx.theme();
    let inset = theme.space().sm();
    let width = theme.icon_column() + inset * 2.0;
    let (id, glyph) = match side {
        PanelSide::Left => ("panel-expand-left", "\u{f101}"),
        PanelSide::Right => ("panel-expand-right", "\u{f100}"),
    };
    let expand = QuietButton::new(id, glyph).on_click({
        let panels = panels.clone();
        move |_e, _w, cx| panels.update(cx, |p, cx| p.open(side, cx))
    });
    div()
        .flex()
        .flex_col()
        .w(px(width))
        .flex_shrink_0()
        .h_full()
        .child(Bar::new().centered().child(expand))
        .child(Separator::horizontal())
        .into_any_element()
}

/// The rule beside a docked panel, widened into a grip: the hairline stays
/// where it was and a few invisible pixels either side of it take the
/// pointer.
fn resize_handle(side: PanelSide, panels: &Entity<Panels>, cx: &mut App) -> AnyElement {
    let theme = cx.theme();
    let thickness = theme.space().hairline();
    // The grip's reach, either side of the rule. Negative margins keep it
    // from moving the panes: the layout still sees one hairline.
    let reach = theme.space().sm().max(4.0);
    let rule = theme.border().opacity(0.2);
    let accent = theme.accent();
    let dragging = panels.read(cx).resizing() == Some(side);
    let id = match side {
        PanelSide::Left => "panel-resize-left",
        PanelSide::Right => "panel-resize-right",
    };
    let panels = panels.clone();
    div()
        .id(id)
        .flex()
        .flex_row()
        .justify_center()
        .flex_shrink_0()
        .h_full()
        .w(px(thickness + reach * 2.0))
        .mx(px(-reach))
        .cursor_col_resize()
        .on_mouse_down(
            gpui::MouseButton::Left,
            move |event: &gpui::MouseDownEvent, _w, cx| {
                let x = f32::from(event.position.x);
                panels.update(cx, |p, cx| p.start_resize(side, x, cx));
            },
        )
        .child(
            div()
                .h_full()
                .w(px(thickness))
                // Lit while dragged, so the edge being moved is the one
                // thing on screen that says so.
                .bg(if dragging { accent } else { rule }),
        )
        .into_any_element()
}

/// A panel floating over the content, in a window too narrow to dock it:
/// the panel at its edge of the window, over a scrim that closes it.
///
/// Docked panels are chromeless, but a floating one covers the content and
/// would be unreadable without its own ground and an edge to separate it
/// from what it hides.
fn float(
    side: PanelSide,
    panel: SidePanel,
    width: f32,
    scrim: gpui::Hsla,
    panels: &Entity<Panels>,
    cx: &mut App,
) -> AnyElement {
    let background = cx.theme().background();
    let inner = column(side, panel, width, panels, cx);
    let mut card = div().flex().flex_row().h_full().bg(background);
    card = match side {
        PanelSide::Left => card.child(inner).child(Separator::vertical()),
        PanelSide::Right => card.child(Separator::vertical()).child(inner),
    };
    let (id, layer) = match side {
        PanelSide::Left => ("panel-float-left", div().justify_start()),
        PanelSide::Right => ("panel-float-right", div().justify_end()),
    };
    let panels = panels.clone();
    layer
        .id(id)
        .absolute()
        .inset_0()
        .bg(scrim)
        .flex()
        .flex_row()
        .on_click(move |_e, _w, cx| panels.update(cx, |p, cx| p.close(side, cx)))
        .child(card)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::new(crate::fallback_tokens())
    }

    fn wide(theme: &Theme) -> Panels {
        let mut panels = Panels::new();
        panels.begin_frame(theme.space().dropdown_width() * 6.0, theme);
        panels
    }

    #[test]
    fn a_fresh_panel_is_the_default_width() {
        let theme = theme();
        let panels = wide(&theme);
        assert_eq!(
            panels.width(PanelSide::Left, &theme),
            theme.space().dropdown_width()
        );
    }

    #[test]
    fn the_floor_and_the_centre_minimum_hold() {
        let theme = theme();
        let default = theme.space().dropdown_width();
        let mut panels = wide(&theme).with_widths(Some(1), Some(100_000));
        assert_eq!(
            panels.width(PanelSide::Left, &theme),
            (default * PANEL_MIN_FACTOR).round()
        );
        // The right panel may take the room minus the left's clamped width.
        let room = (default * 6.0 * (1.0 - CENTER_MIN_FRACTION)).round();
        let left = panels.width(PanelSide::Left, &theme);
        assert_eq!(panels.width(PanelSide::Right, &theme), room - left);
        // Without a neighbour it takes all of the room.
        panels.set_open(PanelSide::Left, false);
        assert_eq!(panels.width(PanelSide::Right, &theme), room);
    }

    #[test]
    fn a_narrow_window_floats_instead_of_docking() {
        let theme = theme();
        let mut panels = Panels::new();
        panels.begin_frame(theme.space().dropdown_width() * 2.0, &theme);
        assert!(panels.narrow(&theme));
        assert!(!panels.docked(PanelSide::Left, &theme));
        assert!(panels.is_open(PanelSide::Left), "the intent survives");
        assert!(!panels.shown(PanelSide::Left, &theme));
    }

    #[test]
    fn widening_drops_the_float() {
        let theme = theme();
        let mut panels = Panels::new();
        panels.begin_frame(theme.space().dropdown_width() * 2.0, &theme);
        panels.floating = Some(PanelSide::Right);
        assert!(panels.shown(PanelSide::Right, &theme));
        panels.begin_frame(theme.space().dropdown_width() * 6.0, &theme);
        assert_eq!(panels.floating(), None);
        assert!(panels.docked(PanelSide::Right, &theme));
    }
}
