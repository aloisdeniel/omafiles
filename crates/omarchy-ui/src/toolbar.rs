//! A bar of verbs that folds into a menu when it runs out of room.

use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement as _, IntoElement, ParentElement,
    Pixels, Point, RenderOnce, Styled, Window, div, px,
};

use crate::{ActionButton, ActiveTheme as _, QuietButton, spacer};

/// How many of a row of widths, `gap` apart, fit in `room`: all of them
/// when they all do, else the leading ones that fit beside a `tail` (the
/// bar's menu button) which then stands for the rest.
pub fn leading_that_fit(widths: &[f32], gap: f32, room: f32, tail: f32) -> usize {
    let total = widths.iter().sum::<f32>() + gap * widths.len().saturating_sub(1) as f32;
    if total <= room {
        return widths.len();
    }
    let mut used = 0.0;
    let mut count = 0;
    for width in widths {
        let need = if count == 0 {
            *width
        } else {
            used + gap + width
        };
        if need + gap + tail > room {
            break;
        }
        used = need;
        count += 1;
    }
    count
}

/// The `…` button was clicked: how many verbs the bar is showing, so the
/// menu can list the rest, and where the click was, so the menu can open
/// there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverflowEvent {
    pub shown: usize,
    pub position: Point<Pixels>,
}

type OverflowHandler = Box<dyn Fn(&OverflowEvent, &mut Window, &mut App) + 'static>;

/// The verbs of a panel's bar, most reached-for first: the bar shows them
/// from the left and drops the rest into an overflow menu behind a `…`
/// button. Takes the width of a [`crate::Bar`] item, so it sits beside the
/// bar's own controls.
///
/// ```ignore
/// ActionBar::new("verbs", panel_width)
///     .compact(!config.button_labels)
///     .actions(verbs)
///     .on_overflow(cx.listener(|this, event: &OverflowEvent, window, cx| {
///         this.open_more_menu(event.shown, event.position, window, cx)
///     }))
/// ```
#[derive(IntoElement)]
pub struct ActionBar {
    id: ElementId,
    actions: Vec<ActionButton>,
    compact: bool,
    /// The width of the panel the bar sits in — what the verbs must fit
    /// beside the bar's collapse control and the overflow button.
    panel_width: f32,
    on_overflow: Option<OverflowHandler>,
}

impl ActionBar {
    pub fn new(id: impl Into<ElementId>, panel_width: f32) -> Self {
        Self {
            id: id.into(),
            actions: Vec::new(),
            compact: true,
            panel_width,
            on_overflow: None,
        }
    }

    /// Glyphs alone, with the verb a hover away. The default.
    pub fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }

    pub fn action(mut self, action: ActionButton) -> Self {
        self.actions.push(action);
        self
    }

    pub fn actions(mut self, actions: impl IntoIterator<Item = ActionButton>) -> Self {
        self.actions.extend(actions);
        self
    }

    pub fn on_overflow(
        mut self,
        handler: impl Fn(&OverflowEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_overflow = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for ActionBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let inset = theme.space().sm();
        let more_width = theme.icon_column();
        // The collapse control at the far right is always there; the verbs
        // share what is left of the bar with the menu button.
        let room = self.panel_width - inset * 3.0 - more_width;

        let actions: Vec<ActionButton> = self
            .actions
            .into_iter()
            .map(|a| a.compact(self.compact))
            .collect();
        let widths: Vec<f32> = actions.iter().map(|a| a.estimated_width(theme)).collect();
        let shown = leading_that_fit(&widths, inset, room, more_width);
        let hidden = shown < actions.len();

        let verbs: Vec<AnyElement> = actions
            .into_iter()
            .take(shown)
            .map(IntoElement::into_any_element)
            .collect();
        let mut bar = div()
            .id(self.id)
            .flex()
            .flex_row()
            .items_center()
            .flex_1()
            .min_w(px(0.))
            .gap(px(inset))
            .overflow_hidden()
            .children(verbs)
            .child(spacer());
        if hidden {
            let mut more = QuietButton::new("more", "\u{f141}"); // nf-fa-ellipsis_h
            if let Some(handler) = self.on_overflow {
                more = more.on_click(move |event: &ClickEvent, window, cx| {
                    let overflow = OverflowEvent {
                        shown,
                        position: event.position(),
                    };
                    handler(&overflow, window, cx)
                });
            }
            bar = bar.child(more);
        }
        bar
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_verb_fits_when_the_bar_is_wide_enough() {
        // Three of 20 and two gaps of 4: 68 exactly.
        assert_eq!(leading_that_fit(&[20., 20., 20.], 4., 68., 16.), 3);
    }

    #[test]
    fn a_narrow_bar_keeps_the_leading_verbs_and_room_for_the_menu() {
        // 67 is one short of all three; two (44) plus a gap and the tail
        // (64) fit, three would not even without the tail.
        assert_eq!(leading_that_fit(&[20., 20., 20.], 4., 67., 16.), 2);
        // The tail is charged even when the verbs alone would have fit:
        // two verbs and the tail need 64, so 60 shows only one.
        assert_eq!(leading_that_fit(&[20., 20., 20.], 4., 60., 16.), 1);
    }

    #[test]
    fn a_bar_too_narrow_for_any_verb_shows_only_the_menu() {
        assert_eq!(leading_that_fit(&[20., 20.], 4., 30., 16.), 0);
        assert_eq!(leading_that_fit(&[], 4., 30., 16.), 0);
    }
}
