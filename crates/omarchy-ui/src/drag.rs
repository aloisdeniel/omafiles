//! Drag and drop: the label that follows the pointer, and the highlight a
//! target shows while it can take the drop.
//!
//! The payloads themselves are the app's: gpui routes drops by the payload's
//! type, so every draggable kind of thing needs its own struct. What is
//! shared is how a drag looks, which is what lives here.

use gpui::{
    App, AppContext as _, Context, Entity, Hsla, IntoElement, ParentElement, Render,
    StyleRefinement, Styled, Window, div, px,
};

use crate::ActiveTheme as _;

/// The highlight a target takes while a dragged `T` is over it: the
/// selected fill, so "you can drop here" reads the same everywhere. Pass
/// it to [`crate::Row::drag_over`] or gpui's own `drag_over`.
///
/// ```ignore
/// Row::new(id).drag_over::<DraggedEntries>(drop_highlight)
/// ```
pub fn drop_highlight<T>(
    style: StyleRefinement,
    _dragged: &T,
    _window: &mut Window,
    cx: &mut App,
) -> StyleRefinement {
    style.bg(cx.theme().selected_fill())
}

/// The floating label for a drag. Detached from the row, so it needs its
/// own ground — otherwise it reads as text floating over the UI. Build one
/// with [`drag_label`] from a row's drag preview closure.
///
/// The colours are taken when it is built rather than read each frame: a
/// preview lives for one drag, and a theme switch mid-drag is not a case
/// worth a subscription.
pub struct DragLabel {
    label: String,
    background: Hsla,
    border: Hsla,
    text: Hsla,
    radius: f32,
    padding: f32,
    height: f32,
}

/// What follows the pointer while `label` is dragged.
///
/// ```ignore
/// Row::new(id).draggable(payload, |payload, _position, _window, cx| {
///     drag_label(payload.label(), cx)
/// })
/// ```
pub fn drag_label(label: impl Into<String>, cx: &mut App) -> Entity<DragLabel> {
    let theme = cx.theme();
    let preview = DragLabel {
        label: label.into(),
        background: theme.surface(),
        border: theme.border(),
        text: theme.bright_foreground(),
        radius: theme.radius(),
        padding: theme.space().row_padding_x(),
        height: theme.space().control_height(),
    };
    cx.new(|_| preview)
}

impl Render for DragLabel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .h(px(self.height))
            .px(px(self.padding))
            .rounded(px(self.radius))
            .bg(self.background)
            .border(px(1.))
            .border_color(self.border)
            .text_color(self.text)
            .child(self.label.clone())
    }
}
