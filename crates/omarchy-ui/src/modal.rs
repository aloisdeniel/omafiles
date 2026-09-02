//! A generic modal overlay.
//!
//! Omarchy's shell has one of these — `[menu]` and `[launcher]` in `shell.toml`
//! define a scrim, a card and a selected-row treatment — so this reads their
//! tokens rather than inventing an overlay style. A dialog that looks like the
//! Omarchy menu is the point.
//!
//! Deliberately unopinionated about content: it supplies the scrim, the card,
//! the title and the footer, and the caller fills the middle. The prompt in
//! `omafiles` and the search palette are both this.

use gpui::{
    AnyElement, App, ClickEvent, InteractiveElement as _, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled, Window, div, px,
};

use crate::{ActiveTheme as _, KeyHint, Separator, color};

/// Named because the boxed closure type is unreadable inline.
type DismissHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// How much of the window a modal card takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModalSize {
    /// A prompt: one field and two hints.
    #[default]
    Small,
    /// A palette: a field plus a scrolling result list.
    Large,
}

/// A card over a dimmed scrim, hung from the top of the window.
///
/// ```ignore
/// Modal::new("new-workspace", "New workspace")
///     .child(TextInput::new(&self.input))
///     .hint("⏎", "create")
///     .hint("esc", "cancel")
///     .on_dismiss(cx.listener(|this, _, _, cx| this.close(cx)))
/// ```
#[derive(IntoElement)]
pub struct Modal {
    id: SharedString,
    title: SharedString,
    subtitle: Option<SharedString>,
    size: ModalSize,
    children: Vec<AnyElement>,
    hints: Vec<(SharedString, SharedString)>,
    on_dismiss: Option<DismissHandler>,
}

impl Modal {
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle: None,
            size: ModalSize::default(),
            children: Vec::new(),
            hints: Vec::new(),
            on_dismiss: None,
        }
    }

    /// A line under the title — what this modal will act on.
    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub fn size(mut self, size: ModalSize) -> Self {
        self.size = size;
        self
    }

    /// A key hint in the footer. The modal is keyboard-driven; these are how
    /// anyone finds that out.
    pub fn hint(mut self, key: impl Into<SharedString>, action: impl Into<SharedString>) -> Self {
        self.hints.push((key.into(), action.into()));
        self
    }

    /// Clicking the scrim dismisses. **Escape must be wired by the caller** —
    /// it is an action on the focused element, and this component does not own
    /// focus. A modal you can only leave with the mouse would be a bad one, so
    /// this is worth stating rather than assuming.
    pub fn on_dismiss(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss = Some(Box::new(handler));
        self
    }
}

impl ParentElement for Modal {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Modal {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let space = theme.space();
        let menu = &theme.tokens.surfaces.menu;

        // Hung from the top rather than centred: a palette whose result list
        // grows and shrinks as you type would otherwise bounce around the
        // window's middle, and the eye keeps returning to the same place for
        // every overlay. The same margin below caps the card, and anything
        // taller scrolls inside it.
        let margin = space.space(64.0);
        let max_height = (f32::from(window.viewport_size().height) - margin * 2.0).max(margin);

        let width = match self.size {
            ModalSize::Small => space.searchable_dropdown_width() * 2.0,
            ModalSize::Large => space.searchable_dropdown_width() * 3.0,
        };
        let card_background = color(menu.background).opacity(menu.background_alpha);
        let border = color(menu.border).opacity(menu.border_alpha);
        // `darker_background`, not `background`: the shell draws its scrim over
        // the desktop, but ours sits over a window already painted in
        // `background` — using the same colour makes the scrim invisible. This
        // is darker than the window on every theme in the corpus, light ones
        // included, because Omarchy derives it as a mix toward black.
        let scrim = color(theme.tokens.palette.darker_background()).opacity(0.72);

        // The card carries no padding of its own — each section does. That is
        // what lets the rules between sections, and the ones between a list's
        // rows, run edge to edge to the card border, the same discipline the
        // window chrome adopted when its panels went flush.
        let pad = space.popup_padding();
        let mut card = div()
            .flex()
            .flex_col()
            .w(px(width))
            .max_h(px(max_height))
            .rounded(px(theme.radius()))
            .bg(card_background)
            .border(px(theme.border_width().max(1.0)))
            .border_color(border)
            // Clicks inside must not fall through to the scrim's dismiss.
            .occlude()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space.xxs()))
                    .px(px(pad))
                    .pt(px(pad))
                    .pb(px(space.md()))
                    .child(
                        div()
                            .text_size(px(theme.type_scale().subtitle()))
                            .text_color(theme.bright_foreground())
                            .child(self.title),
                    )
                    .children(self.subtitle.map(|text| {
                        div()
                            .text_size(px(theme.type_scale().caption()))
                            .text_color(theme.dim_foreground())
                            .child(text)
                    })),
            )
            // A rule between the header and whatever the modal carries, so
            // every contextual menu divides the same way — edge to edge.
            .child(Separator::horizontal())
            .child(
                // The body: vertical rhythm only. A child that is not a flush
                // list (an input, prose) brings its own horizontal inset.
                //
                // Scrolls when the card hits its cap. A result list inside
                // brings its own scroll and shrinks first (a scroll container's
                // automatic minimum is zero), so this only takes over for
                // content that cannot shrink — prose, a stack of fields.
                div()
                    .id("modal-body")
                    .flex()
                    .flex_col()
                    .min_h(px(0.))
                    .gap(px(space.md()))
                    .py(px(space.md()))
                    .overflow_y_scroll()
                    .children(self.children),
            );

        if !self.hints.is_empty() {
            card = card.child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(space.control_gap()))
                    .px(px(pad))
                    .pb(px(pad))
                    .children(self.hints.into_iter().map(|(k, a)| KeyHint::new(k, a))),
            );
        }

        // Explicit insets rather than `size_full()`: a percentage height on an
        // absolutely positioned child resolves against the containing block in
        // a way that left the scrim covering only part of the window.
        let mut scrim_layer = div()
            .id(self.id)
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(margin))
            .bg(scrim);

        if let Some(handler) = self.on_dismiss {
            scrim_layer = scrim_layer.on_click(handler);
        }
        scrim_layer.child(card)
    }
}
