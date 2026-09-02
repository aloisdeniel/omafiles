//! `InteractiveSurface` — the one primitive every row, tab, button and tile
//! composes.
//!
//! Omarchy's interaction vocabulary has five states, and the distinction that
//! matters for a keyboard-driven app is that **hover and the keyboard cursor
//! are the same state** while **focus is separate**. A pointer hovering row 3
//! and the keyboard cursor sitting on row 7 must look alike; the pane that
//! currently owns focus is what changes.
//!
//! Getting this right is most of what "looks like Omarchy" means, which is why
//! it is a primitive rather than something each component reimplements.

use gpui::{Div, Hsla, Styled, div, px};

use crate::Theme;

/// Which interaction state a surface is painting.
///
/// Ordered by precedence: later variants win when several are true at once, so
/// a pressed row still reads as pressed while it is also selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SurfaceState {
    /// Idle chrome.
    #[default]
    Normal,
    /// Mouse hover *or* the keyboard cursor. Deliberately one state.
    Hover,
    /// Real keyboard focus. Defaults to the hover treatment unless a theme
    /// separates them.
    Focus,
    /// A persistent chosen state — the current directory, a selected file.
    Selected,
    /// Momentary, while a press is held.
    Pressed,
}

impl SurfaceState {
    /// Resolve the state from independent flags, applying precedence once so
    /// call sites do not each invent their own ordering.
    pub fn resolve(hovered: bool, focused: bool, selected: bool, pressed: bool) -> Self {
        match (pressed, selected, focused, hovered) {
            (true, ..) => Self::Pressed,
            (_, true, ..) => Self::Selected,
            (_, _, true, _) => Self::Focus,
            (_, _, _, true) => Self::Hover,
            _ => Self::Normal,
        }
    }

    /// The fill wash for this state.
    pub fn fill(self, theme: &Theme) -> Hsla {
        match self {
            Self::Normal => theme.normal_fill(),
            Self::Hover => theme.hover_fill(),
            Self::Focus => theme.focus_fill(),
            Self::Selected => theme.selected_fill(),
            Self::Pressed => theme.pressed_fill(),
        }
    }

    /// Border colour and alpha for this state.
    fn border(self, theme: &Theme) -> (Hsla, f32) {
        let controls = &theme.tokens.controls;
        let style = match self {
            Self::Normal => &controls.normal,
            Self::Hover | Self::Pressed => &controls.hover_cursor,
            Self::Focus => &controls.focus,
            Self::Selected => &controls.selected,
        };
        (crate::color(style.border), style.border_width)
    }

    /// Whether the surface should read as active rather than idle. Used to
    /// decide whether text steps up to the brighter foreground.
    pub fn is_active(self) -> bool {
        !matches!(self, Self::Normal)
    }
}

/// How much chrome a surface paints when idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Chrome {
    /// Fill and border only once the surface is interacted with.
    ///
    /// The right default for list rows: a directory listing of 200 files with
    /// 200 visible borders is noise.
    #[default]
    Quiet,
    /// Always draw the idle fill and border — buttons, inputs, cards.
    Always,
}

/// Apply Omarchy's interaction treatment to a `div`.
///
/// ```ignore
/// InteractiveSurface::new(state)
///     .chrome(Chrome::Always)
///     .paint(div().px_2(), cx.theme())
///     .child("Open")
/// ```
#[derive(Debug, Clone, Copy)]
pub struct InteractiveSurface {
    state: SurfaceState,
    chrome: Chrome,
    rounded: bool,
    bordered: bool,
}

/// Written out rather than derived: two of the fields default to *true*, and a
/// derived `Default` would silently hand back a square, borderless surface that
/// `new()` never produces.
impl Default for InteractiveSurface {
    fn default() -> Self {
        Self::new(SurfaceState::default())
    }
}

impl InteractiveSurface {
    pub fn new(state: SurfaceState) -> Self {
        Self {
            state,
            chrome: Chrome::default(),
            rounded: true,
            bordered: true,
        }
    }

    /// Build from independent flags rather than a resolved state.
    pub fn from_flags(hovered: bool, focused: bool, selected: bool, pressed: bool) -> Self {
        Self::new(SurfaceState::resolve(hovered, focused, selected, pressed))
    }

    pub fn chrome(mut self, chrome: Chrome) -> Self {
        self.chrome = chrome;
        self
    }

    /// Suppress corner rounding — for surfaces that span an edge, where a
    /// radius would leave a visible notch.
    pub fn square(mut self) -> Self {
        self.rounded = false;
        self
    }

    /// Carry the state in the fill and the text alone, with no outline.
    ///
    /// For surfaces that are *items in a list* rather than controls: an outline
    /// around the cursor row draws a box in the middle of a column of names,
    /// and the wash plus a step up in text colour says the same thing more
    /// quietly. Buttons, inputs and cards keep their border — there the outline
    /// is what says "this is a control".
    pub fn borderless(mut self) -> Self {
        self.bordered = false;
        self
    }

    pub fn state(&self) -> SurfaceState {
        self.state
    }

    /// Paint fill, border and radius onto `element`.
    ///
    /// Generic over [`Styled`] rather than taking a `Div`, because callers that
    /// need interaction handlers have already called `.id()` and hold a
    /// `Stateful<Div>`.
    pub fn paint<E: Styled>(self, element: E, theme: &Theme) -> E {
        let paints_chrome = self.chrome == Chrome::Always || self.state.is_active();

        let element = if self.rounded {
            element.rounded(px(theme.radius()))
        } else {
            element
        };

        if !paints_chrome {
            return element;
        }

        let (border_color, border_width) = self.state.border(theme);
        let element = element.bg(self.state.fill(theme));

        if !self.bordered || border_width <= 0.0 {
            // A zero width is a real theme choice — `selected` defaults to it —
            // so it must mean "no border", not "hairline".
            element
        } else {
            element.border(px(border_width)).border_color(border_color)
        }
    }

    /// Paint onto a fresh `div`.
    pub fn build(self, theme: &Theme) -> Div {
        self.paint(div(), theme)
    }

    /// The text colour that belongs with this state.
    ///
    /// Idle sits at the *secondary* foreground and only the active states come
    /// up to the primary one. A list is mostly idle rows, so this is the step
    /// that makes the cursor row the focal point — it is doing the work an
    /// outline used to do, which is why [`borderless`](Self::borderless)
    /// surfaces still read.
    pub fn text_color(self, theme: &Theme) -> Hsla {
        if self.state.is_active() {
            theme.foreground()
        } else {
            theme.dim_foreground()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_is_applied_once_and_consistently() {
        assert_eq!(
            SurfaceState::resolve(false, false, false, false),
            SurfaceState::Normal
        );
        assert_eq!(
            SurfaceState::resolve(true, false, false, false),
            SurfaceState::Hover
        );
        assert_eq!(
            SurfaceState::resolve(false, true, false, false),
            SurfaceState::Focus
        );
        assert_eq!(
            SurfaceState::resolve(false, false, true, false),
            SurfaceState::Selected
        );
        assert_eq!(
            SurfaceState::resolve(false, false, false, true),
            SurfaceState::Pressed
        );

        // Pressed beats everything; selected beats focus and hover.
        assert_eq!(
            SurfaceState::resolve(true, true, true, true),
            SurfaceState::Pressed
        );
        assert_eq!(
            SurfaceState::resolve(true, true, true, false),
            SurfaceState::Selected
        );
        assert_eq!(
            SurfaceState::resolve(true, true, false, false),
            SurfaceState::Focus
        );
    }

    #[test]
    fn only_normal_is_inactive() {
        assert!(!SurfaceState::Normal.is_active());
        for state in [
            SurfaceState::Hover,
            SurfaceState::Focus,
            SurfaceState::Selected,
            SurfaceState::Pressed,
        ] {
            assert!(state.is_active(), "{state:?}");
        }
    }

    #[test]
    fn quiet_chrome_is_the_default() {
        assert_eq!(
            InteractiveSurface::new(SurfaceState::Normal).chrome,
            Chrome::Quiet
        );
    }

    #[test]
    fn the_default_surface_is_the_one_new_builds() {
        // The trap a derived `Default` walks into: two fields default to true,
        // so the derive would hand back a square, borderless surface that no
        // call site ever asks for.
        let (default, fresh) = (
            InteractiveSurface::default(),
            InteractiveSurface::new(SurfaceState::Normal),
        );
        assert_eq!(default.rounded, fresh.rounded);
        assert_eq!(default.bordered, fresh.bordered);
        assert_eq!(default.chrome, fresh.chrome);
    }

    #[test]
    fn idle_text_is_secondary_and_active_text_is_primary() {
        // The rule a borderless row leans on: with no outline to change, the
        // step from secondary to primary text is what marks the cursor row, so
        // the two must actually be different colours.
        let theme = Theme::new(crate::fallback_tokens());
        let idle = InteractiveSurface::new(SurfaceState::Normal).text_color(&theme);
        assert_eq!(idle, theme.dim_foreground());

        for state in [
            SurfaceState::Hover,
            SurfaceState::Focus,
            SurfaceState::Selected,
            SurfaceState::Pressed,
        ] {
            let active = InteractiveSurface::new(state).text_color(&theme);
            assert_eq!(active, theme.foreground(), "{state:?}");
            assert_ne!(active, idle, "{state:?}");
        }
    }
}
