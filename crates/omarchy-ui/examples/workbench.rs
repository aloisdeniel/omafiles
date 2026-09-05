//! The skeleton of an Omarchy app: the three-column [`Workbench`] with a
//! sidebar, a listing and a detail panel, each under its own bar, a status
//! bar, and a modal. Copy this file to start one.
//!
//! ```text
//! cargo run -p omarchy-ui --example workbench
//! ```
//!
//! Try: collapse a panel from its bar, expand it from the strip, drag the
//! rule beside a panel, narrow the window until the panels float, press
//! `?` for the modal and `escape` to close it, `[` and `]` for the panels.

use gpui::{
    App, AppContext as _, Context, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    KeyBinding, ParentElement, Render, Styled, Subscription, Window, actions, div, px,
};
use omarchy_ui::{
    ActionBar, ActionButton, ActiveTheme as _, Bar, Breadcrumb, Column, ColumnHeader, FactSheet,
    Icon, KeyHint, Modal, ModalSize, PanelSide, Panels, PanelsEvent, QuietRow, Row, RowLabel,
    SectionHeader, Separator, ShortcutSheet, SidePanel, StatusBar, Workbench, spacer,
};

actions!(
    workbench,
    [ShowHelp, Dismiss, ToggleLeft, ToggleRight, MoveDown, MoveUp]
);

const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    ("Move", &[("j / k", "down / up")]),
    ("Panels", &[("[", "sidebar"), ("]", "detail")]),
    ("Help", &[("?", "this sheet"), ("esc", "close")]),
];

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        omarchy_ui::init(cx);
        cx.bind_keys([
            KeyBinding::new("?", ShowHelp, None),
            KeyBinding::new("escape", Dismiss, None),
            KeyBinding::new("[", ToggleLeft, None),
            KeyBinding::new("]", ToggleRight, None),
            KeyBinding::new("j", MoveDown, None),
            KeyBinding::new("k", MoveUp, None),
        ]);
        let options = omarchy_ui::window_options("dev.omarchy.omafiles.workbench", "workbench");
        cx.open_window(options, |window, cx| cx.new(|cx| Skeleton::new(window, cx)))
            .expect("failed to open window");
        cx.activate(true);
    });
}

const ROWS: &[(&str, &str, &str)] = &[
    ("crates/", "—", "2h"),
    ("plan/", "—", "1d"),
    ("Cargo.toml", "412 B", "2h"),
    ("README.md", "3.1 kB", "3d"),
    (".gitignore", "24 B", "9d"),
];

struct Skeleton {
    panels: gpui::Entity<Panels>,
    focus: FocusHandle,
    cursor: usize,
    help: bool,
    /// Held: dropping either silently stops the view following its state.
    _subscriptions: Vec<Subscription>,
}

impl Skeleton {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let panels = cx.new(|_| Panels::new());
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let subscriptions = vec![
            omarchy_ui::observe_theme(cx),
            cx.observe(&panels, |_, _, cx| cx.notify()),
            // Where an app would write the width to its config.
            cx.subscribe(&panels, |_, _, event, _| {
                let PanelsEvent::Resized { side, width } = event;
                eprintln!("{side:?} panel resized to {width}px");
            }),
        ];
        Self {
            panels,
            focus,
            cursor: 2,
            help: false,
            _subscriptions: subscriptions,
        }
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let places = [
            ("\u{f015}", "Home"),
            ("\u{f07b}", "Documents"),
            ("\u{f08d}", "omafiles"),
        ];
        div()
            .flex()
            .flex_col()
            .w_full()
            .py(px(cx.theme().space().sm()))
            .child(SectionHeader::new("places"))
            .children(places.into_iter().enumerate().map(|(i, (glyph, label))| {
                Row::new(("place", i))
                    .selected(i == 2)
                    .focused(true)
                    .child(Icon::new(glyph))
                    .child(RowLabel::new(label))
            }))
            .child(QuietRow::new("place-new", "\u{f067}", "Pin a directory"))
    }

    fn listing(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let icon = cx.theme().icon_column();
        let accent = cx.theme().accent();
        let cursor = self.cursor;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .child(
                Bar::new()
                    .child(ActionButton::new("back").glyph("\u{f060}").enabled(false))
                    .child(ActionButton::new("up").glyph("\u{f062}"))
                    .child(Breadcrumb::new(["~", "Documents", "Github", "omafiles"])),
            )
            .child(Separator::horizontal())
            .child(
                ColumnHeader::new()
                    .leading(icon)
                    .column(Column::flex("name"))
                    .column(Column::fixed("size", 72.))
                    .column(Column::fixed("age", 48.))
                    .sorted(0, false),
            )
            .child(Separator::horizontal())
            .children(ROWS.iter().enumerate().map(move |(i, (name, size, age))| {
                let is_dir = name.ends_with('/');
                let mut icon = Icon::new(if is_dir { "\u{f07b}" } else { "\u{f15b}" });
                if i == cursor {
                    icon = icon.color(accent);
                }
                Row::new(("entry", i))
                    .cursor(i == cursor)
                    .focused(true)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.cursor = i;
                        cx.notify();
                    }))
                    .child(icon)
                    .child(RowLabel::new(*name))
                    .child(div().w(px(72.)).flex_shrink_0().child(*size))
                    .child(div().w(px(48.)).flex_shrink_0().child(*age))
            }))
    }

    fn detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (name, size, age) = ROWS[self.cursor];
        let pad = cx.theme().space().row_padding_x();
        div().p(px(pad)).child(
            FactSheet::new()
                .title(name)
                .fact("size", size)
                .fact("modified", age),
        )
    }

    fn detail_verbs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let width = self.panels.read(cx).width(PanelSide::Right, cx.theme());
        ActionBar::new("verbs", width)
            .action(ActionButton::new("open").glyph("\u{f08e}").label("Open"))
            .action(ActionButton::new("copy").glyph("\u{f0c5}").label("Copy"))
            .action(ActionButton::new("share").glyph("\u{f1e0}").label("Share"))
            .action(ActionButton::new("trash").glyph("\u{f1f8}").label("Trash"))
            .on_overflow(|event, _window, _cx| {
                eprintln!("{} verbs shown; the rest would open a menu", event.shown);
            })
    }

    fn help(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        self.help.then(|| {
            Modal::new("help", "Shortcuts")
                .size(ModalSize::Large)
                .child(omarchy_ui::modal_inset(cx).child(ShortcutSheet::from_table(SHORTCUTS)))
                .hint("esc", "close")
                .on_dismiss(cx.listener(|this, _, _, cx| {
                    this.help = false;
                    cx.notify();
                }))
                .into_any_element()
        })
    }
}

impl Focusable for Skeleton {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Skeleton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let panels = self.panels.clone();
        // Handlers on a div *around* the workbench, so they stay reachable
        // while a modal has focus (see `Workbench`).
        div()
            .size_full()
            .on_action(cx.listener(|this, _: &ShowHelp, _, cx| {
                this.help = true;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| {
                this.help = false;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| {
                this.cursor = (this.cursor + 1).min(ROWS.len() - 1);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| {
                this.cursor = this.cursor.saturating_sub(1);
                cx.notify();
            }))
            .on_action({
                let panels = panels.clone();
                move |_: &ToggleLeft, _, cx| {
                    panels.update(cx, |p, cx| p.toggle(PanelSide::Left, cx))
                }
            })
            .on_action(move |_: &ToggleRight, _, cx| {
                panels.update(cx, |p, cx| p.toggle(PanelSide::Right, cx))
            })
            .child(
                Workbench::new(&self.panels)
                    .focus("Skeleton", &self.focus)
                    .left(
                        SidePanel::new(self.sidebar(cx)).bar([
                            ActionButton::new("find")
                                .glyph("\u{f002}")
                                .into_any_element(),
                            spacer().into_any_element(),
                        ]),
                    )
                    .center(self.listing(cx))
                    .right(
                        SidePanel::new(self.detail(cx))
                            .bar([self.detail_verbs(cx).into_any_element()]),
                    )
                    .status(
                        StatusBar::new()
                            .leading(div().child(format!("{} entries", ROWS.len())))
                            .leading(KeyHint::new("?", "help"))
                            .trailing(
                                ActionButton::new("terminal")
                                    .glyph("\u{f120}")
                                    .label("Terminal"),
                            )
                            .trailing(ActionButton::new("help").glyph("?").label("Help").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.help = true;
                                    cx.notify();
                                }),
                            )),
                    )
                    .overlay(self.help(cx)),
            )
    }
}
