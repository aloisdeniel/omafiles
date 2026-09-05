//! A scrolling region with a visible scrollbar.
//!
//! Bare gpui paints no scrollbar at all; `gpui-component` supplies one, and
//! it renders in Omarchy's palette through [`crate::sync_gpui_component`].
//! This puts the two together: the content fills the region and the bar
//! rides its right edge without taking width from it.

use gpui::{AnyElement, App, IntoElement, ParentElement, RenderOnce, Styled, Window, div};
use gpui_component::scroll::{Scrollbar, ScrollbarHandle};

/// A column that fills its parent, scrolls, and shows a scrollbar for the
/// handle its content is tracking.
///
/// ```ignore
/// ScrollArea::new(&self.scroll).child(
///     uniform_list("rows", count, cx.processor(build_rows)).h_full().track_scroll(&self.scroll),
/// )
/// ```
///
/// The content must call `track_scroll` with the same handle; the bar only
/// reads it.
#[derive(IntoElement)]
pub struct ScrollArea {
    scrollbar: Scrollbar,
    children: Vec<AnyElement>,
}

impl ScrollArea {
    pub fn new<H: ScrollbarHandle + Clone>(handle: &H) -> Self {
        Self {
            scrollbar: Scrollbar::vertical(handle),
            children: Vec::new(),
        }
    }
}

impl ParentElement for ScrollArea {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for ScrollArea {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(gpui::px(0.))
            .children(self.children)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .child(self.scrollbar),
            )
    }
}
