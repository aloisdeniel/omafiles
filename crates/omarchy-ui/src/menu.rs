//! Menus and the pieces every contextual list is built from.
//!
//! A [`ContextMenu`] is a card at the pointer — or a [`Modal`] when it was
//! summoned from the keyboard and has no pointer to sit at. A
//! [`GroupHeader`] names a collapsible group of rows and takes drops for it.
//! [`separated`] and [`modal_inset`] are what the modals in an app share:
//! the rule between rows, and the inset a non-list body child sits in.

use gpui::{
    AnyElement, App, ClickEvent, DragMoveEvent, ElementId, InteractiveElement as _, IntoElement,
    ParentElement, Pixels, Point, RenderOnce, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled, Window, div, px,
};

use crate::components::ElementAdapter;
use crate::{ActiveTheme as _, Modal, Separator};

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// Interleave the subtle rule between a menu's rows, so every contextual
/// list divides the same way.
pub fn separated(rows: Vec<AnyElement>) -> Vec<AnyElement> {
    let mut out = Vec::with_capacity(rows.len() * 2);
    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 {
            out.push(Separator::horizontal().subtle().into_any_element());
        }
        out.push(row);
    }
    out
}

/// A modal body child's horizontal inset. The card itself is flush so rules
/// can run edge to edge; anything that is not a flush list — an input,
/// prose, a status line — sits in one of these.
pub fn modal_inset(cx: &App) -> gpui::Div {
    div().px(px(cx.theme().space().popup_padding()))
}

/// A contextual menu: a titled card of rows.
///
/// With a [`position`](ContextMenu::position) it is a card at that point —
/// clamped so it stays on screen — over a transparent scrim that dismisses
/// on click. Without one it is an ordinary [`Modal`], for the keyboard
/// route: the same rows, hung from the top like every other overlay.
///
/// ```ignore
/// ContextMenu::new("entry-menu", name)
///     .position(click_position)
///     .rows(rows)
///     .on_dismiss(cx.listener(|this, _, window, cx| this.close_menu(window, cx)))
/// ```
///
/// Escape is the caller's, as it is for [`Modal`].
#[derive(IntoElement)]
pub struct ContextMenu {
    id: SharedString,
    title: SharedString,
    rows: Vec<AnyElement>,
    position: Option<Point<Pixels>>,
    on_dismiss: Option<ClickHandler>,
}

impl ContextMenu {
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            rows: Vec::new(),
            position: None,
            on_dismiss: None,
        }
    }

    /// Where the pointer was. `None` — the keyboard summoned it — renders
    /// as a modal.
    pub fn position(mut self, position: Option<Point<Pixels>>) -> Self {
        self.position = position;
        self
    }

    /// The rows, top to bottom. [`crate::Row`]s by convention; a rule is
    /// drawn between them.
    pub fn rows(mut self, rows: Vec<AnyElement>) -> Self {
        self.rows = rows;
        self
    }

    pub fn on_dismiss(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss = Some(Box::new(handler));
        self
    }
}

impl ParentElement for ContextMenu {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.rows.extend(elements);
    }
}

impl RenderOnce for ContextMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(point) = self.position else {
            let mut modal = Modal::new(self.id, self.title)
                .child(div().flex().flex_col().children(separated(self.rows)))
                .hint("esc", "close");
            if let Some(handler) = self.on_dismiss {
                modal = modal.on_dismiss(handler);
            }
            return modal.into_any_element();
        };

        let viewport = window.viewport_size();
        let theme = cx.theme();
        let space = theme.space();
        let width = space.dropdown_width();
        let estimated = self.rows.len() as f32 * space.control_height()
            + space.popup_padding() * 2.0
            + space.control_height();
        let x = f32::from(point.x).min(f32::from(viewport.width) - width - space.md());
        let y = f32::from(point.y).min(f32::from(viewport.height) - estimated - space.md());

        // Flush like the modals: sections carry the padding, rules run
        // edge to edge.
        let card = div()
            .flex()
            .flex_col()
            .w(px(width))
            .rounded(px(theme.radius()))
            .bg(theme.menu_background())
            .border(px(theme.border_width().max(1.0)))
            .border_color(theme.menu_border())
            .occlude()
            .child(
                div()
                    .px(px(space.row_padding_x()))
                    .pt(px(space.sm()))
                    .pb(px(space.xs()))
                    .text_size(px(theme.type_scale().caption()))
                    .text_color(theme.dim_foreground())
                    .overflow_hidden()
                    .child(self.title),
            )
            // The same header/content rule the modals draw.
            .child(Separator::horizontal())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .py(px(space.xs()))
                    .children(separated(self.rows)),
            );

        let mut scrim = div().id(self.id).absolute().inset_0();
        if let Some(handler) = self.on_dismiss {
            scrim = scrim.on_click(handler);
        }
        scrim
            .child(
                div()
                    .absolute()
                    .left(px(x.max(0.0)))
                    .top(px(y.max(0.0)))
                    .child(card),
            )
            .into_any_element()
    }
}

/// The header of a collapsible group of rows — a workspace's tabs, a
/// project's files. The name is the collapse toggle, with a chevron that
/// says which way it will go; controls about the group sit at the right
/// edge; and the header is a drop target, so a group stays droppable when
/// it is empty — which is exactly when you most want to drag something
/// into it.
#[derive(IntoElement)]
pub struct GroupHeader {
    id: ElementId,
    label: SharedString,
    collapsed: bool,
    active: bool,
    on_toggle: Option<ClickHandler>,
    trailing: Vec<AnyElement>,
    drop: Vec<ElementAdapter>,
    drag_over: Vec<ElementAdapter>,
    drag_move: Vec<ElementAdapter>,
}

impl GroupHeader {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            collapsed: false,
            active: false,
            on_toggle: None,
            trailing: Vec::new(),
            drop: Vec::new(),
            drag_over: Vec::new(),
            drag_move: Vec::new(),
        }
    }

    pub fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }

    /// The group holding where you are: drawn in the plain foreground, not
    /// the dim one, the way a place row says so.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Clicking the name.
    pub fn on_toggle(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle = Some(Box::new(handler));
        self
    }

    /// A control at the right edge — a [`crate::QuietButton`] by convention.
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing.push(element.into_any_element());
        self
    }

    /// Accept a dragged `T` dropped on the header.
    pub fn on_drop<T: 'static>(
        mut self,
        handler: impl Fn(&T, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.drop
            .push(Box::new(move |element| element.on_drop(handler)));
        self
    }

    /// Restyle while a dragged `T` is over the header. Pair with
    /// [`GroupHeader::on_drop`] for the same `T`.
    pub fn drag_over<T: 'static>(
        mut self,
        style: impl Fn(StyleRefinement, &T, &mut Window, &mut App) -> StyleRefinement + 'static,
    ) -> Self {
        self.drag_over
            .push(Box::new(move |element| element.drag_over::<T>(style)));
        self
    }

    pub fn on_drag_move<T: 'static>(
        mut self,
        handler: impl Fn(&DragMoveEvent<T>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.drag_move
            .push(Box::new(move |element| element.on_drag_move(handler)));
        self
    }
}

impl RenderOnce for GroupHeader {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let space = theme.space();
        let (pad_x, gap, chevron_gap) = (space.row_padding_x(), space.xs(), space.md());
        let (pad_top, pad_bottom, caption) =
            (space.sm(), space.xxs(), theme.type_scale().caption());
        let dim = theme.dim_foreground_on(theme.tokens.palette.lighter_background());
        let bright = theme.bright_foreground();
        let text = if self.active { theme.foreground() } else { dim };

        let mut name = div()
            .id("toggle")
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(chevron_gap))
            .px(px(pad_x))
            .pt(px(pad_top))
            .pb(px(pad_bottom))
            .text_size(px(caption))
            .text_color(text)
            .cursor_pointer()
            .hover(move |style| style.text_color(bright))
            .child(div().flex_shrink_0().child(if self.collapsed {
                "\u{f054}" // nf-fa-chevron_right
            } else {
                "\u{f078}" // nf-fa-chevron_down
            }))
            // Plain case: a group is named by the user, unlike the fixed
            // section headers above it.
            .child(div().min_w(px(0.)).overflow_hidden().child(self.label));
        if let Some(handler) = self.on_toggle {
            name = name.on_click(handler);
        }

        let mut header = div()
            .id(self.id)
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .pr(px(pad_x))
            .child(name)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(gap))
                    .children(self.trailing),
            );
        for apply in self.drag_over {
            header = apply(header);
        }
        for apply in self.drop {
            header = apply(header);
        }
        for apply in self.drag_move {
            header = apply(header);
        }
        header
    }
}
