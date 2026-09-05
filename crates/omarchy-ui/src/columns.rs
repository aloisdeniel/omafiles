//! A list's column header: labels that sort, and dividers that resize.
//!
//! One flexible column takes whatever is left; the fixed ones after it have
//! widths, and the boundary at each fixed column's left edge is a grip.
//! Dragging a grip moves the boundary under the pointer: the column on its
//! right gives up what the one on its left gains, so the columns beyond stay
//! put. The first grip trades with the flexible column, which absorbs it.
//!
//! The arithmetic lives in [`ColumnResize`], which is plain data, so an app
//! stores it beside the widths it persists and tests it without a display.

use gpui::{
    AnyElement, App, ElementId, InteractiveElement as _, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled, Window, div, px,
};

use crate::ActiveTheme as _;

/// A column boundary being dragged: which grip, where the pointer started,
/// and the fixed widths then. Build one on mouse-down, ask it for the
/// widths on every move, drop it on mouse-up.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnResize {
    divider: usize,
    start_x: f32,
    start: Vec<f32>,
}

impl ColumnResize {
    /// `divider` indexes the fixed columns: the grip at the left edge of
    /// fixed column `divider`. `widths` are the fixed widths at mouse-down.
    pub fn begin(divider: usize, x: f32, widths: Vec<f32>) -> Self {
        Self {
            divider,
            start_x: x,
            start: widths,
        }
    }

    pub fn divider(&self) -> usize {
        self.divider
    }

    /// The fixed widths with the pointer at `x`, every column held between
    /// `min` and `max`. Clamped as a pair, so that when one column hits its
    /// floor the other stops too and the boundary stays under the pointer
    /// rather than the columns sliding as a block.
    pub fn widths_at(&self, x: f32, min: f32, max: f32) -> Vec<f32> {
        let travel = x - self.start_x;
        let mut widths = self.start.clone();
        let Some(&right_start) = self.start.get(self.divider) else {
            return widths;
        };
        let right = (right_start - travel).clamp(min, max);
        match self.divider.checked_sub(1) {
            None => widths[self.divider] = right,
            Some(left_index) => {
                let pair = self.start[left_index] + right_start;
                let left = (pair - right).clamp(min, max);
                widths[left_index] = left;
                widths[self.divider] = (pair - left).clamp(min, max);
            }
        }
        widths
    }
}

/// A click on a column's label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortEvent {
    /// The column's index, in the order given to the header.
    pub column: usize,
}

/// Mouse-down on a grip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GripEvent {
    /// The grip's index among the fixed columns — what
    /// [`ColumnResize::begin`] takes.
    pub divider: usize,
    /// The pointer's x, in window coordinates.
    pub x: f32,
}

/// One column of a [`ColumnHeader`].
#[derive(Debug, Clone)]
pub struct Column {
    label: SharedString,
    width: Option<f32>,
}

impl Column {
    /// The flexible column: takes whatever the fixed ones leave.
    pub fn flex(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            width: None,
        }
    }

    /// A fixed column, `width` wide, with a grip at its left edge.
    pub fn fixed(label: impl Into<SharedString>, width: f32) -> Self {
        Self {
            label: label.into(),
            width: Some(width),
        }
    }
}

type SortHandler = Box<dyn Fn(&SortEvent, &mut Window, &mut App) + 'static>;
type GripHandler = std::rc::Rc<dyn Fn(&GripEvent, &mut Window, &mut App) + 'static>;
/// The sort handler, shared by every label it is cloned into.
type SharedSortHandler = std::rc::Rc<dyn Fn(&SortEvent, &mut Window, &mut App) + 'static>;

/// The labels above a list, on a row's metrics — height, padding, gap — so
/// they sit over the values they name.
///
/// Built from a plain `div` rather than a [`crate::Row`]: it is never the
/// cursor and never marked, and a row's interaction vocabulary would only
/// invite it to light up as one. Uppercase caption in secondary text — the
/// treatment [`crate::SectionHeader`] gives a group label, because this is
/// the same kind of thing.
///
/// Columns are indexed in the order given; `on_sort` receives that index.
/// Grips are indexed among the *fixed* columns, in order, which is what
/// [`ColumnResize::begin`] takes.
///
/// ```ignore
/// ColumnHeader::new()
///     .leading(theme.icon_column())
///     .column(Column::flex("name"))
///     .column(Column::fixed("size", size_width))
///     .column(Column::fixed("age", age_width))
///     .sorted(sort.column, sort.descending)
///     .on_sort(cx.listener(|this, event: &SortEvent, _, cx| this.sort_by(event.column, cx)))
///     .resizing(self.column_resize.as_ref().map(ColumnResize::divider))
///     .on_grip(cx.listener(|this, event: &GripEvent, _, cx| {
///         this.start_column_resize(event.divider, event.x, cx)
///     }))
/// ```
#[derive(IntoElement)]
pub struct ColumnHeader {
    /// Reserved width before the first label — the icon column, which has
    /// no label but still has to be there, or every label sits one column
    /// to the left.
    leading: Option<f32>,
    columns: Vec<Column>,
    sorted: Option<(usize, bool)>,
    resizing: Option<usize>,
    on_sort: Option<SortHandler>,
    on_grip: Option<GripHandler>,
}

impl Default for ColumnHeader {
    fn default() -> Self {
        Self::new()
    }
}

impl ColumnHeader {
    pub fn new() -> Self {
        Self {
            leading: None,
            columns: Vec::new(),
            sorted: None,
            resizing: None,
            on_sort: None,
            on_grip: None,
        }
    }

    pub fn leading(mut self, width: f32) -> Self {
        self.leading = Some(width);
        self
    }

    pub fn column(mut self, column: Column) -> Self {
        self.columns.push(column);
        self
    }

    pub fn columns(mut self, columns: impl IntoIterator<Item = Column>) -> Self {
        self.columns.extend(columns);
        self
    }

    /// Which column the list is sorted by, and whether the values run down
    /// the list in descending order. The label carries a caret pointing
    /// the way they run.
    pub fn sorted(mut self, column: usize, descending: bool) -> Self {
        self.sorted = Some((column, descending));
        self
    }

    /// A click on a label. Takes the shape `cx.listener(…)` produces.
    pub fn on_sort(
        mut self,
        handler: impl Fn(&SortEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_sort = Some(Box::new(handler));
        self
    }

    /// The grip being dragged, if any: it lights in the accent.
    pub fn resizing(mut self, divider: Option<usize>) -> Self {
        self.resizing = divider;
        self
    }

    /// Mouse-down on a grip: the fixed-column index and the pointer's x.
    /// The drag itself is tracked by the app on a container that spans the
    /// window (see [`crate::Workbench`]), so the pointer can leave the
    /// hairline it started on.
    pub fn on_grip(
        mut self,
        handler: impl Fn(&GripEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_grip = Some(std::rc::Rc::new(handler));
        self
    }
}

impl RenderOnce for ColumnHeader {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let space = theme.space();
        let caption = theme.type_scale().caption();
        let dim = theme.dim_foreground();
        let fg = theme.foreground();
        let gap = space.control_gap();
        let (height, padding) = (space.control_height(), space.row_padding_x());
        let on_sort: Option<SharedSortHandler> = self.on_sort.map(std::rc::Rc::from);
        let sorted = self.sorted;

        let label = |index: usize, text: SharedString| {
            let caret = sorted
                .filter(|(column, _)| *column == index)
                .map(|(_, descending)| {
                    if descending {
                        "\u{f0d7}" // nf-fa-caret_down
                    } else {
                        "\u{f0d8}" // nf-fa-caret_up
                    }
                });
            let is_sorted = sorted.is_some_and(|(column, _)| column == index);
            let mut label = div()
                .id(("sort-by", index))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(gap * 0.5))
                .min_w(px(0.))
                .text_color(if is_sorted { fg } else { dim })
                .child(div().truncate().child(text.to_uppercase()))
                .children(caret);
            if let Some(on_sort) = on_sort.clone() {
                label = label
                    .cursor_pointer()
                    .hover(move |style| style.text_color(fg))
                    .on_click(move |_event, window, cx| {
                        on_sort(&SortEvent { column: index }, window, cx)
                    });
            }
            label
        };

        let mut header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(gap))
            .w_full()
            .h(px(height))
            .px(px(padding))
            .text_size(px(caption))
            .text_color(dim)
            .children(self.leading.map(|width| div().w(px(width)).flex_shrink_0()));

        let mut grips = 0;
        for (index, column) in self.columns.into_iter().enumerate() {
            let cell = match column.width {
                None => div()
                    .flex_1()
                    .min_w(px(0.))
                    .child(label(index, column.label)),
                Some(width) => {
                    let divider = grips;
                    grips += 1;
                    div()
                        .relative()
                        .w(px(width))
                        .flex_shrink_0()
                        .child(label(index, column.label))
                        .child(grip(
                            divider,
                            gap,
                            self.resizing == Some(divider),
                            self.on_grip.clone(),
                            cx,
                        ))
                }
            };
            header = header.child(cell);
        }
        header
    }
}

/// The rule at a fixed column's left edge, widened into a grip: a faint
/// hairline in the middle of the gap before the column, and a few invisible
/// pixels either side that take the pointer. Absolutely placed, so it costs
/// the header no width and the labels stay over the values.
fn grip(
    divider: usize,
    gap: f32,
    dragging: bool,
    on_grip: Option<GripHandler>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let thickness = theme.space().hairline();
    let reach = theme.space().sm().max(4.0);
    let rule = theme.border().opacity(0.2);
    let lit = theme.border();
    let accent = theme.accent();
    let id: ElementId = ("column-grip", divider).into();
    let group = format!("column-grip-{divider}");
    let mut handle = div()
        .id(id)
        .group(group.clone())
        .absolute()
        .top_0()
        .bottom_0()
        .left(px(-(gap + thickness) / 2.0 - reach))
        .w(px(thickness + reach * 2.0))
        .flex()
        .flex_row()
        .justify_center()
        .cursor_col_resize()
        .child(
            div()
                .h_full()
                .w(px(thickness))
                // Lit while dragged, so the edge being moved is the one
                // thing on screen that says so; brighter under the
                // pointer, so the grip announces itself before the drag.
                .bg(if dragging { accent } else { rule })
                .group_hover(group, move |style| {
                    style.bg(if dragging { accent } else { lit })
                }),
        );
    if let Some(on_grip) = on_grip {
        handle = handle.on_mouse_down(
            gpui::MouseButton::Left,
            move |event: &gpui::MouseDownEvent, window, cx| {
                let grip = GripEvent {
                    divider,
                    x: f32::from(event.position.x),
                };
                on_grip(&grip, window, cx)
            },
        );
    }
    handle.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_grip_trades_with_the_flexible_column() {
        let resize = ColumnResize::begin(0, 100., vec![72., 48.]);
        // Pointer right by 10: the first fixed column shrinks, the flexible
        // one (not tracked here) absorbs it; the other fixed column holds.
        assert_eq!(resize.widths_at(110., 40., 320.), vec![62., 48.]);
        assert_eq!(resize.widths_at(90., 40., 320.), vec![82., 48.]);
    }

    #[test]
    fn a_later_grip_trades_between_its_neighbours() {
        let resize = ColumnResize::begin(1, 100., vec![72., 48.]);
        // The pair's sum is preserved: 120.
        assert_eq!(resize.widths_at(105., 40., 320.), vec![77., 43.]);
        assert_eq!(resize.widths_at(95., 40., 320.), vec![67., 53.]);
    }

    #[test]
    fn the_pair_stops_together_at_the_floor() {
        let resize = ColumnResize::begin(1, 100., vec![72., 48.]);
        // Dragging far right would take the age column below 40: it stops
        // at 40 and the size column stops at 80, not at 120 - 8.
        assert_eq!(resize.widths_at(200., 40., 320.), vec![80., 40.]);
        // And far left: the size column bottoms out at 40, age tops at 80.
        assert_eq!(resize.widths_at(0., 40., 320.), vec![40., 80.]);
    }

    #[test]
    fn a_grip_that_does_not_exist_changes_nothing() {
        let resize = ColumnResize::begin(5, 100., vec![72., 48.]);
        assert_eq!(resize.widths_at(150., 40., 320.), vec![72., 48.]);
    }
}
