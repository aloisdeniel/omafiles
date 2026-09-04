//! The explorer shell.
//!
//! Two panes, a real directory listing, virtualised scrolling, the keyboard
//! model, live refresh, tabs and workspaces, search, and the preview. File
//! actions are M9.
//!
//! Composition, decided in `GPUI-NOTES.md` §9:
//!
//! - gpui's own `uniform_list` virtualises (our rows are a fixed
//!   `control_height`, which is exactly what it wants)
//! - `omarchy_ui::Row` renders each row, so interaction states come from
//!   Omarchy's tokens
//! - gpui-component supplies only the **scrollbar**, which bare gpui does not
//!   have at all
//!
//! We do *not* take gpui-component's table or list: `Row` already exists, and
//! their table cannot do modifier-aware multi-select without a fork.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, DragMoveEvent, Entity, FocusHandle,
    Focusable, HighlightStyle, ImageFormat, InteractiveElement as _, IntoElement, KeyBinding,
    ObjectFit, ParentElement, Pixels, Point, Render, ScrollStrategy,
    StatefulInteractiveElement as _, StyleRefinement, Styled, StyledImage as _, StyledText,
    Subscription, Task, TitlebarOptions, UniformListScrollHandle, Window,
    WindowBackgroundAppearance, WindowDecorations, WindowOptions, actions, div, img, px,
    uniform_list,
};
use gpui_component::highlighter::SyntaxHighlighter;
use gpui_component::input::{Input, InputEvent, InputState, Rope};
use gpui_component::scroll::Scrollbar;
use gpui_component::text::TextView;
use omafiles::actions;
use omafiles::config;
use omafiles::entry::{Entry, Kind, format_age, format_size, natural_cmp, nearest_existing};
use omafiles::fileops;
use omafiles::git;
use omafiles::grep;
use omafiles::imageops;
use omafiles::keymap;
use omafiles::listing::{Listing, SortKey, describe_empty, is_navigable};
use omafiles::network;
use omafiles::places::{Origin, Place, Places};
use omafiles::preview::{self, Body, Preview, Target};
use omafiles::recent;
use omafiles::search::{Match, Search, walk};
use omafiles::server;
use omafiles::session::{Session, Tab};
use omafiles::views::{DirectoryView, Views};
use omarchy_ui::{
    ActionButton, ActiveTheme as _, Badge, Breadcrumb, Modal, ModalSize, Row, SectionHeader,
    Separator, SyntaxPalette, Theme,
};

const APP_ID: &str = "dev.omarchy.omafiles";

/// Rows a page-jump moves. A true page depends on viewport height, which the
/// model does not know; this is the conventional half-page.
const PAGE: isize = 10;

actions!(
    omafiles,
    [
        MoveDown,
        MoveUp,
        MoveFirst,
        MoveLast,
        PageDown,
        PageUp,
        Open,
        GoUp,
        GoBack,
        GoForward,
        ToggleHidden,
        Refresh,
        Quit,
        // Sidebar (M4)
        FocusNext,
        FocusPrevious,
        PinCurrent,
        UnpinSelected,
        MovePinUp,
        MovePinDown,
        // Tabs & workspaces (M5)
        NewTab,
        CloseTab,
        NextTab,
        PreviousTab,
        NewWorkspace,
        DeleteWorkspace,
        MoveTabToNextWorkspace,
        // Search & modals (M6)
        StartSearch,
        // Network locations
        AddNetwork,
        Dismiss,
        Confirm,
        RenameWorkspace,
        // Chrome (M6+)
        ShowHelp,
        ToggleLeftPanel,
        ToggleRightPanel,
        EditPath,
        GoParent,
        // Preview (M7)
        TogglePreview,
        // Git (M8)
        SwitchBranch,
        // Actions (M9)
        OpenTerminal,
        AskAgent,
        ShareEntry,
        // HTTP server (M10)
        ServerMenu,
        // Polish (M11)
        CommandPalette,
        // File operations & context menu
        CopyEntry,
        PasteHere,
        CompressEntry,
        EntryMenu,
        // Multi-server list
        ServerList,
        // Move & create
        MoveEntry,
        CreateFile,
        // Cut, and delete to the trash
        CutEntry,
        DeleteEntry,
        CopyPath,
        // Selection, for drag and drop
        ExtendDown,
        ExtendUp,
        SelectAll,
        ToggleSelect,
        // Settings toggles
        ToggleButtonLabels,
    ]
);

fn main() {
    // The detached serving mode: `omafiles --serve <dir> [--lan]`. Handled
    // before gpui exists — a server has no window, and this is what lets the
    // windowed app close while its servers keep answering.
    let args: Vec<String> = std::env::args().collect();
    if matches!(args.get(1).map(String::as_str), Some("--version" | "-V")) {
        println!("omafiles {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.get(1).map(String::as_str) == Some("--serve") {
        let Some(root) = args.get(2).map(PathBuf::from) else {
            eprintln!("usage: omafiles --serve <directory> [--lan]");
            std::process::exit(2);
        };
        let lan = args.iter().any(|arg| arg == "--lan");
        match server::serve_forever(root, lan) {
            Ok(never) => match never {},
            Err(message) => {
                eprintln!("omafiles --serve: {message}");
                std::process::exit(1);
            }
        }
    }

    gpui_platform::application().run(|cx: &mut App| {
        Theme::install(cx);
        gpui_component::init(cx);
        // Push Omarchy's palette into gpui-component so its scrollbar matches
        // the rest of the window.
        let tokens = cx.theme().tokens.clone();
        omarchy_ui::sync_gpui_component(&tokens, cx);

        let keymap = keymap::Keymap::load(&config_dir().join("omafiles/keymap.toml"));
        bind_keys(cx, &keymap);
        let config_path = config_dir().join("omafiles/config.toml");
        let config = config::Config::load(&config_path);
        cx.on_action(|_: &Quit, cx: &mut App| cx.quit());

        let start = start_directory();
        let options = WindowOptions {
            app_id: Some(APP_ID.to_string()),
            titlebar: Some(TitlebarOptions {
                title: Some("omafiles".into()),
                ..Default::default()
            }),
            window_decorations: Some(WindowDecorations::Server),
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        };

        cx.open_window(options, |window, cx| {
            cx.new(|cx| Explorer::new(start, keymap, config, config_path, window, cx))
        })
        .expect("failed to open window");
        cx.activate(true);
    });
}

/// The directory to open in: an argument if given, else the working directory,
/// so `omafiles .` and launching from a terminal both do the obvious thing.
/// Routed through [`nearest_existing`] so a stale path still opens something.
fn start_directory() -> PathBuf {
    let requested = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"));

    let absolute = if requested.is_absolute() {
        requested
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&requested))
            .unwrap_or(requested)
    };
    nearest_existing(&absolute)
}

/// Expand a leading `~`, which is what anyone typing a path will use.
fn expand_tilde(input: &str) -> String {
    match input.strip_prefix('~') {
        Some(rest) => format!("{}{}", home_dir().display(), rest),
        None => input.to_string(),
    }
}

/// The file extension a clipboard picture is written with.
fn image_extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Gif => "gif",
        ImageFormat::Webp => "webp",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
        ImageFormat::Ico => "ico",
        ImageFormat::Svg => "svg",
        _ => "png",
    }
}

/// `$HOME`, or `/` if the environment has no idea.
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// The finder's content section, with files already named above removed —
/// one file, one row, whichever way it matched.
fn finder_content_rows<'a>(names: &[Match], hits: &'a [grep::Hit]) -> Vec<&'a grep::Hit> {
    hits.iter()
        .filter(|hit| !names.iter().any(|m| m.path == hit.path))
        .collect()
}

/// What one context-menu row does when picked.
type ContextAction = Box<dyn Fn(&mut Explorer, &mut Window, &mut Context<Explorer>)>;

/// A context menu's surround: a card at the click's point — clamped so it
/// stays on screen — or an ordinary centred modal for the keyboard route.
fn menu_surround(
    title: String,
    rows: Vec<AnyElement>,
    position: Option<Point<Pixels>>,
    viewport: gpui::Size<Pixels>,
    dismiss: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    cx: &mut Context<Explorer>,
) -> AnyElement {
    match position {
        Some(point) => {
            let theme = cx.theme();
            let space = theme.space();
            let menu = &theme.tokens.surfaces.menu;
            let width = space.dropdown_width();
            let estimated = rows.len() as f32 * space.control_height()
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
                .bg(omarchy_ui::color(menu.background).opacity(menu.background_alpha))
                .border(px(theme.border_width().max(1.0)))
                .border_color(omarchy_ui::color(menu.border).opacity(menu.border_alpha))
                .occlude()
                .child(
                    div()
                        .px(px(space.row_padding_x()))
                        .pt(px(space.sm()))
                        .pb(px(space.xs()))
                        .text_size(px(theme.type_scale().caption()))
                        .text_color(theme.dim_foreground())
                        .overflow_hidden()
                        .child(title),
                )
                // The same header/content rule the modals draw.
                .child(Separator::horizontal())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .py(px(space.xs()))
                        .children(separated(rows)),
                );

            div()
                .id("ctx-scrim")
                .absolute()
                .inset_0()
                .on_click(dismiss)
                .child(
                    div()
                        .absolute()
                        .left(px(x.max(0.0)))
                        .top(px(y.max(0.0)))
                        .child(card),
                )
                .into_any_element()
        }
        None => Modal::new("context", title)
            .child(div().flex().flex_col().children(separated(rows)))
            .hint("esc", "close")
            .on_dismiss(dismiss)
            .into_any_element(),
    }
}

/// A modal body child's horizontal inset. The card itself is flush so rules
/// can run edge to edge; anything that is not a flush list — an input, prose,
/// a status line — sits in one of these.
fn modal_inset(cx: &App) -> gpui::Div {
    div().px(px(cx.theme().space().popup_padding()))
}

/// Interleave the subtle rule between a menu's rows, so every contextual
/// list divides the same way.
fn separated(rows: Vec<AnyElement>) -> Vec<AnyElement> {
    let mut out = Vec::with_capacity(rows.len() * 2);
    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 {
            out.push(Separator::horizontal().subtle().into_any_element());
        }
        out.push(row);
    }
    out
}

/// One line, cut in the middle: the first fifth and the tail survive, so a
/// long path shows where it starts *and* where it ends.
fn middle_truncate(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    let head = max / 5;
    let tail = max.saturating_sub(head + 1);
    let mut out: String = chars[..head].iter().collect();
    out.push('\u{2026}');
    out.extend(chars[chars.len() - tail..].iter());
    out
}

/// A path as a person would say it: `~`-relative when under home.
fn display_path(path: &Path) -> String {
    match path.strip_prefix(home_dir()) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// `~/.local/state/omafiles/session.toml`.
///
/// State rather than config: it is machine-written on nearly every navigation.
/// `places.toml` stays under `~/.config` because it is user-curated.
fn session_path() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".local/state"))
        .join("omafiles/session.toml")
}

/// `$XDG_CONFIG_HOME`, else `~/.config`.
fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config"))
}

/// Every shortcut, for the help modal.
///
/// Deliberately adjacent to [`bind_keys`]: the bindings need typed actions and
/// cannot be generated from data, so the only defence against drift is that the
/// two are impossible to read separately.
const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "Move",
        &[
            ("j / \u{2193}", "next"),
            ("k / \u{2191}", "previous"),
            ("g / \u{21e7}g", "first / last"),
            ("^d / ^u", "half page"),
            ("\u{21e5}", "switch pane"),
        ],
    ),
    (
        "Select",
        &[
            (
                "\u{21e7}\u{2193} / \u{21e7}\u{2191}",
                "extend the selection",
            ),
            ("insert", "toggle the entry, step down"),
            ("^a", "select everything"),
            ("esc", "clear the selection"),
            ("^click / \u{21e7}click", "toggle / extend, with the mouse"),
            ("", "drag onto a directory, a tab or a place to move"),
        ],
    ),
    (
        "Navigate",
        &[
            ("\u{23ce} / l", "open"),
            ("\u{232b} / h", "parent directory"),
            ("\u{2325}\u{2190} / \u{2325}\u{2192}", "back / forward"),
            ("^l", "edit path"),
            ("^r / F5", "refresh"),
        ],
    ),
    (
        "Preview",
        &[
            ("space", "expand over the listing"),
            ("j / k", "next / previous, while expanded"),
            ("esc", "collapse"),
        ],
    ),
    (
        "Act",
        &[
            ("\u{23ce}", "on a file: open with its app"),
            ("t", "terminal here"),
            ("a", "ask the agent about it"),
            ("s", "share via LocalSend"),
            ("^c / ^x / ^v", "copy \u{b7} cut \u{b7} paste here"),
            ("^\u{21e7}c", "copy the path"),
            ("\u{2326}", "move to the trash"),
            ("z", "compress to zip"),
            ("m", "move to another directory"),
            ("n", "new file here \u{2014} a trailing / makes a directory"),
            ("\u{21e7}F10", "entry menu"),
            ("^s", "http server menu"),
            ("^\u{21e7}s", "all http servers"),
        ],
    ),
    (
        "Find",
        &[
            ("/", "find below here \u{2014} recent, names, contents"),
            ("^h", "toggle hidden files"),
        ],
    ),
    (
        "Git",
        &[
            ("^\u{21e7}g", "switch branch"),
            ("", "markers sit on the entry icons"),
            ("", "a changed file previews as its diff"),
        ],
    ),
    (
        "Tabs & workspaces",
        &[
            ("^t", "new tab here"),
            ("^w", "close tab"),
            ("^\u{21e5}", "next tab"),
            ("^n", "new workspace"),
            ("^\u{21e7}r", "rename workspace"),
            ("^\u{21e7}m", "move tab to next workspace"),
            ("^\u{21e7}w", "delete workspace"),
        ],
    ),
    (
        "Places",
        &[
            ("^p", "pin this directory"),
            ("^\u{21e7}n", "add a network location"),
            ("\u{2326}", "unpin"),
            ("\u{2325}j / \u{2325}k", "reorder a pin"),
        ],
    ),
    (
        "Window",
        &[
            ("^k", "command palette"),
            ("^b", "toggle the sidebar"),
            ("^\u{21e7}b", "toggle the detail panel"),
            ("?", "this list"),
            ("esc", "close an overlay"),
            ("q / ^q", "quit"),
        ],
    ),
];

/// Bind the effective keymap — the data in `omafiles::keymap`, which is the
/// defaults with the user's `keymap.toml` merged over them (M11).
///
/// Context scoping is the load-bearing part, as it always was: `j` means
/// "next place" in the sidebar and "next entry" in the listing with no runtime
/// check in either handler, and `/` stops firing the moment a text field takes
/// focus. What changed in M11 is only where the table lives.
///
/// ⚠ The arrow keys and `^g` are bound **globally**, not to an `Input`
/// context — gpui-component's `Input` sets no `key_context` at all, so a
/// binding scoped to one never matches (found the hard way in M8). Safe,
/// because each handler is a no-op unless its overlay is open.
fn bind_keys(cx: &mut App, keymap: &keymap::Keymap) {
    let bindings: Vec<KeyBinding> = keymap
        .bindings
        .iter()
        .filter_map(|binding| {
            let context = match binding.context {
                keymap::Context::Global => None,
                keymap::Context::Listing => Some("Listing"),
                keymap::Context::Sidebar => Some("Sidebar"),
            };
            typed_binding(&binding.keys, binding.action, context)
        })
        .collect();
    cx.bind_keys(bindings);
}

/// The one place a keymap action name meets its typed gpui action.
///
/// `keymap::known_actions` validates names against `DEFAULTS`, so an arm
/// missing here would be a silently dead binding — which is what the
/// `every_keymap_action_binds` test exists to prevent.
fn typed_binding(keys: &str, action: &str, context: Option<&str>) -> Option<KeyBinding> {
    Some(match action {
        "move_down" => KeyBinding::new(keys, MoveDown, context),
        "move_up" => KeyBinding::new(keys, MoveUp, context),
        "move_first" => KeyBinding::new(keys, MoveFirst, context),
        "move_last" => KeyBinding::new(keys, MoveLast, context),
        "page_down" => KeyBinding::new(keys, PageDown, context),
        "page_up" => KeyBinding::new(keys, PageUp, context),
        "open" => KeyBinding::new(keys, Open, context),
        "go_up" => KeyBinding::new(keys, GoUp, context),
        "go_back" => KeyBinding::new(keys, GoBack, context),
        "go_forward" => KeyBinding::new(keys, GoForward, context),
        "toggle_hidden" => KeyBinding::new(keys, ToggleHidden, context),
        "refresh" => KeyBinding::new(keys, Refresh, context),
        "quit" => KeyBinding::new(keys, Quit, context),
        "focus_next" => KeyBinding::new(keys, FocusNext, context),
        "focus_previous" => KeyBinding::new(keys, FocusPrevious, context),
        "pin_current" => KeyBinding::new(keys, PinCurrent, context),
        "unpin_selected" => KeyBinding::new(keys, UnpinSelected, context),
        "move_pin_up" => KeyBinding::new(keys, MovePinUp, context),
        "move_pin_down" => KeyBinding::new(keys, MovePinDown, context),
        "new_tab" => KeyBinding::new(keys, NewTab, context),
        "close_tab" => KeyBinding::new(keys, CloseTab, context),
        "next_tab" => KeyBinding::new(keys, NextTab, context),
        "previous_tab" => KeyBinding::new(keys, PreviousTab, context),
        "new_workspace" => KeyBinding::new(keys, NewWorkspace, context),
        "delete_workspace" => KeyBinding::new(keys, DeleteWorkspace, context),
        "move_tab_to_next_workspace" => KeyBinding::new(keys, MoveTabToNextWorkspace, context),
        "rename_workspace" => KeyBinding::new(keys, RenameWorkspace, context),
        "start_search" => KeyBinding::new(keys, StartSearch, context),
        "add_network" => KeyBinding::new(keys, AddNetwork, context),
        "dismiss" => KeyBinding::new(keys, Dismiss, context),
        "show_help" => KeyBinding::new(keys, ShowHelp, context),
        "toggle_left_panel" => KeyBinding::new(keys, ToggleLeftPanel, context),
        "toggle_right_panel" => KeyBinding::new(keys, ToggleRightPanel, context),
        "edit_path" => KeyBinding::new(keys, EditPath, context),
        "toggle_preview" => KeyBinding::new(keys, TogglePreview, context),
        "switch_branch" => KeyBinding::new(keys, SwitchBranch, context),
        "open_terminal" => KeyBinding::new(keys, OpenTerminal, context),
        "ask_agent" => KeyBinding::new(keys, AskAgent, context),
        "share_entry" => KeyBinding::new(keys, ShareEntry, context),
        "server_menu" => KeyBinding::new(keys, ServerMenu, context),
        "command_palette" => KeyBinding::new(keys, CommandPalette, context),
        "server_list" => KeyBinding::new(keys, ServerList, context),
        "copy_entry" => KeyBinding::new(keys, CopyEntry, context),
        "paste_here" => KeyBinding::new(keys, PasteHere, context),
        "compress_entry" => KeyBinding::new(keys, CompressEntry, context),
        "entry_menu" => KeyBinding::new(keys, EntryMenu, context),
        "move_entry" => KeyBinding::new(keys, MoveEntry, context),
        "create_file" => KeyBinding::new(keys, CreateFile, context),
        "cut_entry" => KeyBinding::new(keys, CutEntry, context),
        "copy_path" => KeyBinding::new(keys, CopyPath, context),
        "delete_entry" => KeyBinding::new(keys, DeleteEntry, context),
        "extend_down" => KeyBinding::new(keys, ExtendDown, context),
        "extend_up" => KeyBinding::new(keys, ExtendUp, context),
        "select_all" => KeyBinding::new(keys, SelectAll, context),
        "toggle_select" => KeyBinding::new(keys, ToggleSelect, context),
        "toggle_button_labels" => KeyBinding::new(keys, ToggleButtonLabels, context),
        _ => return None,
    })
}

/// What the command palette offers (M11): every action worth invoking by
/// name, with the keymap name its hint is looked up by.
///
/// Movement verbs are deliberately absent — a palette that moves the cursor
/// one row is slower than the key it documents, and teaches nothing.
struct Command {
    label: &'static str,
    /// The `keymap` action name, for the effective-keys hint.
    action: &'static str,
    build: fn() -> Box<dyn gpui::Action>,
}

const COMMANDS: &[Command] = &[
    Command {
        label: "Go to parent directory",
        action: "go_up",
        build: || Box::new(GoUp),
    },
    Command {
        label: "Go back",
        action: "go_back",
        build: || Box::new(GoBack),
    },
    Command {
        label: "Go forward",
        action: "go_forward",
        build: || Box::new(GoForward),
    },
    Command {
        label: "Edit path",
        action: "edit_path",
        build: || Box::new(EditPath),
    },
    Command {
        label: "Refresh",
        action: "refresh",
        build: || Box::new(Refresh),
    },
    Command {
        label: "Toggle hidden files",
        action: "toggle_hidden",
        build: || Box::new(ToggleHidden),
    },
    Command {
        label: "Find files",
        action: "start_search",
        build: || Box::new(StartSearch),
    },
    Command {
        label: "Add network location",
        action: "add_network",
        build: || Box::new(AddNetwork),
    },
    Command {
        label: "Expand preview",
        action: "toggle_preview",
        build: || Box::new(TogglePreview),
    },
    Command {
        label: "Open with default app",
        action: "open",
        build: || Box::new(Open),
    },
    Command {
        label: "Terminal here",
        action: "open_terminal",
        build: || Box::new(OpenTerminal),
    },
    Command {
        label: "Ask the agent",
        action: "ask_agent",
        build: || Box::new(AskAgent),
    },
    Command {
        label: "Share via LocalSend",
        action: "share_entry",
        build: || Box::new(ShareEntry),
    },
    Command {
        label: "Copy",
        action: "copy_entry",
        build: || Box::new(CopyEntry),
    },
    Command {
        label: "Cut",
        action: "cut_entry",
        build: || Box::new(CutEntry),
    },
    Command {
        label: "Copy path",
        action: "copy_path",
        build: || Box::new(CopyPath),
    },
    Command {
        label: "Paste here",
        action: "paste_here",
        build: || Box::new(PasteHere),
    },
    Command {
        label: "Move to trash",
        action: "delete_entry",
        build: || Box::new(DeleteEntry),
    },
    Command {
        label: "Compress to zip",
        action: "compress_entry",
        build: || Box::new(CompressEntry),
    },
    Command {
        label: "Move to\u{2026}",
        action: "move_entry",
        build: || Box::new(MoveEntry),
    },
    Command {
        label: "New file or directory",
        action: "create_file",
        build: || Box::new(CreateFile),
    },
    Command {
        label: "Entry menu",
        action: "entry_menu",
        build: || Box::new(EntryMenu),
    },
    Command {
        label: "HTTP servers",
        action: "server_list",
        build: || Box::new(ServerList),
    },
    Command {
        label: "HTTP server",
        action: "server_menu",
        build: || Box::new(ServerMenu),
    },
    Command {
        label: "Switch git branch",
        action: "switch_branch",
        build: || Box::new(SwitchBranch),
    },
    Command {
        label: "New tab",
        action: "new_tab",
        build: || Box::new(NewTab),
    },
    Command {
        label: "Close tab",
        action: "close_tab",
        build: || Box::new(CloseTab),
    },
    Command {
        label: "Next tab",
        action: "next_tab",
        build: || Box::new(NextTab),
    },
    Command {
        label: "Previous tab",
        action: "previous_tab",
        build: || Box::new(PreviousTab),
    },
    Command {
        label: "New workspace",
        action: "new_workspace",
        build: || Box::new(NewWorkspace),
    },
    Command {
        label: "Rename workspace",
        action: "rename_workspace",
        build: || Box::new(RenameWorkspace),
    },
    Command {
        label: "Delete workspace",
        action: "delete_workspace",
        build: || Box::new(DeleteWorkspace),
    },
    Command {
        label: "Move tab to next workspace",
        action: "move_tab_to_next_workspace",
        build: || Box::new(MoveTabToNextWorkspace),
    },
    Command {
        label: "Pin this directory",
        action: "pin_current",
        build: || Box::new(PinCurrent),
    },
    Command {
        label: "Toggle sidebar",
        action: "toggle_left_panel",
        build: || Box::new(ToggleLeftPanel),
    },
    Command {
        label: "Toggle detail panel",
        action: "toggle_right_panel",
        build: || Box::new(ToggleRightPanel),
    },
    Command {
        label: "Select all",
        action: "select_all",
        build: || Box::new(SelectAll),
    },
    Command {
        label: "Toggle button labels",
        action: "toggle_button_labels",
        build: || Box::new(ToggleButtonLabels),
    },
    Command {
        label: "Keyboard shortcuts",
        action: "show_help",
        build: || Box::new(ShowHelp),
    },
    Command {
        label: "Quit",
        action: "quit",
        build: || Box::new(Quit),
    },
];

/// What the location picker is for. The panel is shared — the same field,
/// the same completion, the same create row — and only what Enter does differs.
#[derive(Clone)]
enum PathPurpose {
    /// `^l`, or a click on the breadcrumb: the tab goes there.
    GoTo,
    /// `m`: the entry moves into the directory picked.
    MoveInto { source: PathBuf, name: String },
    /// `n`: an empty file is created at the typed path — or a directory,
    /// when the path ends in `/`. Directory rows complete the field rather
    /// than confirm, because an existing directory is never the answer here.
    CreateFile,
}

/// What the modal layer is showing, if anything.
///
/// One field rather than a bag of booleans: two overlays open at once is not a
/// state this app should be able to represent.
enum Overlay {
    /// The finder — recents, names and contents below the current
    /// directory, in one window (Search, Recent and Grep, merged on
    /// request).
    Finder {
        /// Pinned at open, like the server menus' root.
        root: PathBuf,
        /// The query the sections below answer.
        query: String,
        /// What an empty query shows: the newest files below the root.
        recent: Vec<recent::RecentFile>,
        /// A typed query's first section: fuzzy matches over walked paths.
        names: Vec<Match>,
        /// The second section: files whose *contents* match. Deduplicated
        /// against `names` at render, so a file never appears twice.
        hits: Vec<grep::Hit>,
        /// The walk (recents and the fuzzy corpus) is still out.
        scanning: bool,
        /// The content search is still out.
        searching: bool,
        truncated_walk: bool,
        truncated_grep: bool,
        cursor: usize,
    },
    /// Create or rename a workspace.
    Workspace { editing: Option<usize> },
    /// Add a network location: one field, the URI (§sidebar NETWORK).
    AddNetwork,
    /// Right click on a network row: open, unmount, forget.
    NetworkMenu {
        index: usize,
        position: Option<Point<Pixels>>,
    },
    /// Every shortcut, grouped.
    Help,
    /// The `…` menu on a workspace header.
    WorkspaceMenu { workspace: usize },
    /// The location picker: a modal panel prefilled with a path, completing
    /// from the typed path's descendants. One panel, three verbs — see
    /// [`PathPurpose`].
    Path {
        purpose: PathPurpose,
        /// Directories completing the typed text — the children of an exact
        /// directory, or the parent's children filtered by the last component.
        suggestions: Vec<PathBuf>,
        /// The typed path when no such directory exists yet: offered as a
        /// "create directory" row, below the suggestions, in a colour of its
        /// own so nobody mistakes it for a place that is already there.
        create: Option<PathBuf>,
        /// `None` means Enter takes the typed text; arrows move into the list.
        /// Indices past `suggestions` name the create row.
        cursor: Option<usize>,
    },

    /// The command palette (M11): every action, filterable, Enter runs it.
    Palette {
        /// Indices into [`COMMANDS`] matching the query.
        results: Vec<usize>,
        cursor: usize,
    },

    /// Every running HTTP server, from the navigation bar's globe: copy its
    /// URL, jump to its directory, or kill it.
    Servers,
    /// The right-click menu on a listing entry: the item's actions, gathered.
    Context {
        path: PathBuf,
        name: String,
        is_dir: bool,
        /// Where the click landed — the card opens there. `None` (the
        /// keyboard route) centres it like any other modal.
        position: Option<Point<Pixels>>,
    },
    /// The HTTP server's contextual menu (§6.7), scoped to one root — the
    /// current directory from `^s` and the badge, any server's from the
    /// globe list's logs button.
    ///
    /// Carries no state of its own: what it shows is read from
    /// `Explorer::server` at render time, so the same open menu turns from
    /// the start options into the log and stop the moment a server starts —
    /// and back again when one stops.
    Server { root: PathBuf },
    /// Compose the prompt before launching the agent (§6.6).
    ///
    /// A dialog rather than an immediate launch because the prompt is *ours*,
    /// and a canned sentence fired at an agent unseen is exactly the kind of
    /// magic a keyboard-first app should not do. Enter launches, Escape backs
    /// out with nothing spawned.
    Agent {
        /// Where the agent starts — the entry's directory, so it sits beside
        /// the file it was asked about.
        cwd: PathBuf,
        /// The configured agent's name, for the subtitle: "launch claude in
        /// ~/x" answers what Enter is about to do.
        agent: String,
    },
    /// `^c` on a picture: the file, or PNG bytes at one of several sizes, so
    /// the paste lands in a browser or a chat as a picture rather than a
    /// path. The PNGs are made in the background as the modal opens; a row
    /// shows its byte count once made, and a placeholder until then.
    CopyImage {
        path: PathBuf,
        name: String,
        /// Row 0 is the file itself; row `i + 1` is `variants[i]`.
        variants: Vec<imageops::Variant>,
        /// Per variant, the bytes once made — or why they could not be.
        encoded: Vec<Option<Result<Vec<u8>, String>>>,
        cursor: usize,
    },
    /// A drop that cannot happen, or a move that did not: what was refused
    /// and why, one line each. A modal rather than a status-bar notice
    /// because a drag that ends in nothing reads as a bug, and because
    /// several items can fail for several reasons at once.
    Refused {
        title: &'static str,
        subtitle: String,
        reasons: Vec<String>,
    },
    /// `\u{2326}`: the one destructive verb, and it asks first.
    Delete {
        path: PathBuf,
        name: String,
        is_dir: bool,
    },
    /// Pick a branch to switch to.
    ///
    /// Filtered from a field like the search palette rather than being a plain
    /// list: a repository with fifty branches is common, and reusing the input
    /// also reuses `Enter` and the arrow keys instead of binding a second set.
    Branches {
        all: Vec<String>,
        results: Vec<String>,
        cursor: usize,
        current: Option<String>,
        /// git's own words when it refuses a switch. Kept on screen — the whole
        /// point of shelling out is that its refusal is better than ours.
        error: Option<String>,
    },
}

/// What git says about the directory we are in.
///
/// The head is read inline — it is two filesystem reads — so the status bar has
/// a branch the instant you navigate. The status is the 400 ms half and arrives
/// later, which is the same "render now, fill in when it lands" discipline M3
/// uses for directory reads.
struct Git {
    repo: git::Repo,
    head: git::Head,
    /// `None` until the first background read for this repository lands.
    status: Option<git::Status>,
}

/// A preview and its resolved syntax colours.
///
/// The highlighting is done on the same background thread as the read, and
/// resolved to concrete colours there: the alternative is parsing 4,000 lines on
/// the main thread on every frame, which is exactly the keystroke-blocking
/// §6.5 forbids.
struct Loaded {
    preview: Preview,
    /// Byte ranges into `Body::Text`'s text. Empty for every other body, and
    /// for text with no grammar.
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    /// Rows for `Body::Diff`, flattened and already coloured. Empty otherwise.
    ///
    /// Resolved here rather than at render time for the same reason as
    /// `highlights`: it is two tree-sitter parses, and a preview re-renders on
    /// every cursor move.
    diff: Vec<DiffRow>,
}

/// One rendered line of a diff.
enum DiffRow {
    /// The gap between two hunks, carrying git's guess at what follows.
    Skip(Option<String>),
    Code {
        kind: git::LineKind,
        /// The line's number **in the file as it stands now**, and `None` for a
        /// removed line, which has none.
        ///
        /// Showing the old number there instead would put a 140 between a 145
        /// and a 146 and make the column stop counting. Zed leaves it blank for
        /// the same reason, and the sign already says which side the row is on.
        number: Option<u32>,
        text: String,
        /// Byte ranges into `text`, not into the file it came from.
        highlights: Vec<(Range<usize>, HighlightStyle)>,
    },
}

/// Parse and colour a text preview. **Blocking** — background threads only.
fn highlight(preview: &Preview, syntax: &SyntaxPalette) -> Vec<(Range<usize>, HighlightStyle)> {
    let Body::Text {
        text,
        language: Some(language),
        ..
    } = &preview.body
    else {
        return Vec::new();
    };
    styles_of(text, Some(language), syntax)
}

/// Colour a whole buffer. **Blocking.**
fn styles_of(
    text: &str,
    language: Option<&str>,
    syntax: &SyntaxPalette,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let Some(language) = language else {
        return Vec::new();
    };
    let mut highlighter = SyntaxHighlighter::new(language);
    // `None` for the timeout parses to completion. The incremental path exists
    // for an editor reparsing on every keystroke; a preview parses once.
    highlighter.update(None, &Rope::from(text), None);
    highlighter.styles(&(0..text.len()), syntax)
}

/// A file, indexed by line, so a diff row can borrow that line's colours.
///
/// This is the piece that makes the diff look like Zed's rather than like
/// `git diff` in a terminal: the colours come from parsing the **whole file**
/// with its own grammar, so a hunk in the middle of a function is coloured by a
/// parser that has seen the function. Handing tree-sitter the hunk on its own
/// gives an `ERROR` node a line or two in and no captures after it.
struct Side {
    lines: Vec<Range<usize>>,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
}

impl Side {
    /// **Blocking** — this is where the parse happens.
    fn new(text: &str, language: Option<&str>, syntax: &SyntaxPalette) -> Self {
        let mut lines = Vec::new();
        let mut start = 0;
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                lines.push(start..index);
                start = index + 1;
            }
        }
        if start < text.len() {
            lines.push(start..text.len());
        }
        Self {
            lines,
            highlights: styles_of(text, language, syntax),
        }
    }

    /// The colours for one 1-based line, rebased onto that line's own text.
    ///
    /// `expected` guards the mapping: the diff carries the text and the file
    /// carries the colours, and if the two disagree about a line — a diff taken
    /// against a file that has since been rewritten, or a truncated preview —
    /// plain text is right and colours shifted by a few bytes are not.
    fn line(&self, number: u32, expected: &str) -> Vec<(Range<usize>, HighlightStyle)> {
        let Some(range) = number
            .checked_sub(1)
            .and_then(|index| self.lines.get(index as usize))
        else {
            return Vec::new();
        };
        if range.len() != expected.len() {
            return Vec::new();
        }

        let first = self
            .highlights
            .partition_point(|(span, _)| span.end <= range.start);
        self.highlights[first..]
            .iter()
            .take_while(|(span, _)| span.start < range.end)
            .map(|(span, style)| {
                let start = span.start.max(range.start) - range.start;
                let end = span.end.min(range.end) - range.start;
                (start..end, *style)
            })
            .filter(|(span, _)| !span.is_empty())
            .collect()
    }
}

/// Show a changed file's diff in place of its contents (§6.9).
///
/// A diff *is* a preview, so it becomes one — and it becomes the file rather
/// than the patch. The rows are lines of the file, syntax-highlighted by the
/// file's own grammar, under a full-width wash that says added or removed. That
/// is how Zed renders a diff, and it is better than the obvious alternative for
/// three reasons: the `diff --git` / `index` / `---` / `+++` preamble and the
/// `+`/`-` prefixes never reach the screen, the code keeps its own colours
/// instead of being tinted wholesale by line, and a wash spans the row where a
/// coloured glyph run stops at the end of the text.
///
/// *(Revised after the first version, which rendered `git diff`'s output as
/// text under the `diff` grammar. That was cheap and it read like a terminal.)*
///
/// Only textual bodies are replaced: a modified PNG's diff is one line saying
/// the binary files differ, which tells you less than the picture does.
///
/// **Blocking** — background threads only. Two tree-sitter parses and up to two
/// forks live here.
fn with_diff(
    preview: &mut Preview,
    repo: &git::Repo,
    path: &Path,
    syntax: &SyntaxPalette,
) -> Vec<DiffRow> {
    let language = match &preview.body {
        Body::Text { language, .. } => *language,
        Body::Markdown(_) => Some("markdown"),
        _ => return Vec::new(),
    };
    let Some(diff) = git::diff(repo, path) else {
        return Vec::new();
    };

    // The two sides of the change, each parsed whole so every row can be
    // coloured by a grammar that has seen its file. The new side is the text
    // already read for the ordinary preview, so only the old one costs a fork.
    let new = match &preview.body {
        Body::Text { text, .. } | Body::Markdown(text) => Side::new(text, language, syntax),
        _ => unreachable!("the body was matched above"),
    };
    let old = git::blob_at_head(repo, path)
        .map(|text| Side::new(&text, language, syntax))
        .unwrap_or_else(|| Side::new("", None, syntax));

    let mut rows = Vec::new();
    for (index, hunk) in diff.hunks.iter().enumerate() {
        // Not before the first hunk: a gap mark at the very top implies skipped
        // lines above a hunk that may well start at line 1.
        if index > 0 || hunk.lines.first().is_some_and(|line| line.number > 1) {
            rows.push(DiffRow::Skip(hunk.heading.clone()));
        }
        for line in &hunk.lines {
            let side = match line.kind {
                git::LineKind::Removed => &old,
                _ => &new,
            };
            rows.push(DiffRow::Code {
                kind: line.kind,
                // The number is still what finds the line's colours on its own
                // side; it is only the *gutter* a removed line has nothing for.
                number: (line.kind != git::LineKind::Removed).then_some(line.number),
                highlights: side.line(line.number, &line.text),
                text: line.text.clone(),
            });
        }
    }

    preview.body = Body::Diff(diff);
    rows
}

/// Which pane owns the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pane {
    Sidebar,
    Listing,
}

/// The two side panels, as the things that can be resized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanelSide {
    Left,
    Right,
}

/// A panel edge being dragged: which one, where the pointer started, and
/// how wide the panel was then. The width is applied to the config as the
/// pointer moves and written to disk when the button comes up.
#[derive(Debug, Clone, Copy)]
struct PanelResize {
    side: PanelSide,
    start_x: f32,
    start_width: f32,
}

/// The share of the window the centre column always keeps: however wide
/// the panels are dragged, the listing stays at least this fraction. Below
/// it the listing would be a sliver, and the panels are about the listing.
const CENTER_MIN_FRACTION: f32 = 0.3;

/// The two draggable boundaries in the listing header. Named by the column
/// on their right, which is the one whose left edge they are: the name
/// column takes whatever is left, so it has no width of its own to drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnDivider {
    /// Between the name and the size column.
    Size,
    /// Between the size and the age column.
    Age,
}

/// A column boundary being dragged: which one, where the pointer started,
/// and how wide the two right-hand columns were then. Applied to the
/// directory's view as the pointer moves and written to disk on mouse up,
/// the same shape as [`PanelResize`].
#[derive(Debug, Clone, Copy)]
struct ColumnResize {
    divider: ColumnDivider,
    start_x: f32,
    start_size: f32,
    start_age: f32,
}

/// The narrowest a listing column can be dragged: room for a label and the
/// glyph after it. Below that the column says nothing, and the values in it
/// would be clipped to a digit.
const COLUMN_MIN: f32 = 40.;

/// The widest. Past this the size or age column is eating the name column,
/// which is the one column a listing cannot do without.
const COLUMN_MAX: f32 = 320.;

/// The narrowest a panel goes, as a share of `dropdown-width`. Half the
/// default keeps every row's icon and a few characters of its label; below
/// that the panel says nothing, and collapsing it is the honest gesture.
const PANEL_MIN_FACTOR: f32 = 0.5;

/// What a Pin button can say about a directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PinState {
    /// Not in the sidebar: the button pins.
    Pinnable,
    /// A user pin: the button unpins.
    Pinned,
    /// A system place — permanently in the sidebar, nothing to toggle.
    System,
}

/// The Pin button both scopes share, differing only in target and id.
fn pin_button(
    id: &'static str,
    path: PathBuf,
    state: PinState,
    compact: bool,
    cx: &mut Context<Explorer>,
) -> ActionButton {
    ActionButton::new(id)
        .glyph("\u{f08d}") // nf-fa-thumb_tack
        .label(if state == PinState::Pinned {
            "Unpin"
        } else {
            "Pin"
        })
        .compact(compact)
        .enabled(state != PinState::System)
        .on_click(cx.listener(move |this, _e, _w, cx| this.toggle_pin(&path, cx)))
}

struct Explorer {
    places: Places,
    /// Saved network locations (`network.toml`) — the NETWORK section.
    network: Vec<network::Location>,
    /// Cursor within the sidebar, over `places.all()`.
    place_cursor: usize,
    pane: Pane,
    sidebar_focus: FocusHandle,
    config_dir: PathBuf,
    session_path: PathBuf,
    session: Session,
    /// Listing per tab id, so switching tabs is instant rather than a re-read.
    listings: HashMap<String, Listing>,
    /// Resolved cursor index per tab id. `Tab::cursor_name` is the durable
    /// form — indices shift when a directory changes, names do not — and this
    /// is the fast one for rendering and movement.
    cursors: HashMap<String, usize>,
    /// The marked entries per tab id, by **name** — what a drag carries and
    /// what `\u{21e7}\u{2193}` extends. Names for the same reason
    /// `Tab::cursor_name` is one: a reload keeps the marks on the same
    /// files. Never persisted — a selection is a gesture, not a place.
    selected: HashMap<String, HashSet<String>>,
    /// The revision we last wrote, so the watcher can ignore our own writes.
    written_revision: u64,
    show_hidden: bool,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    /// The in-flight directory read. Held because dropping a `Task` cancels it:
    /// that is what stops a slow network mount from overwriting a directory you
    /// have since navigated away from.
    _pending: Option<Task<()>>,
    watcher: Option<DirWatcher>,
    session_watcher: Option<DirWatcher>,
    overlay: Option<Overlay>,
    /// Panel visibility. Both are *user intent*; a narrow window overrides them
    /// at render time without forgetting what was asked for, so widening the
    /// window restores the panels rather than leaving them shut.
    left_open: bool,
    right_open: bool,
    /// The panel edge under the pointer while it is being dragged.
    resizing: Option<PanelResize>,
    /// The listing column boundary under the pointer while it is dragged.
    column_resizing: Option<ColumnResize>,
    /// How each directory's listing is sorted and its columns sized.
    views: Views,
    /// The window's width as of the last frame, so the panel widths can be
    /// clamped against it from code that only has the app context.
    viewport_width: f32,
    /// Scroll state for the two panels, so their content can overflow.
    left_scroll: gpui::ScrollHandle,
    right_scroll: gpui::ScrollHandle,
    /// One input entity reused by every overlay: they are mutually exclusive,
    /// and a fresh one per open would lose gpui-component's undo history.
    input: Entity<InputState>,
    /// Held: dropping it unsubscribes and the search stops reacting to typing.
    _input_events: Subscription,
    search: Search,
    /// The preview for the entry under the cursor, once it has landed.
    preview: Option<Loaded>,
    /// What `preview` is, or is being, loaded for. Set the moment a load is
    /// spawned rather than when it lands, so a slow read is not requested again
    /// on every frame in between.
    ///
    /// The theme name is part of it because syntax colours are resolved at load
    /// time: without it, switching themes would leave a highlighted file in the
    /// old palette until the cursor moved, in an app whose whole premise is that
    /// it retints live.
    preview_request: Option<(preview::Key, String, Option<git::State>)>,
    /// Held: dropping the task cancels the read, which is what stops a slow
    /// file on a network mount from landing over a file you have since left.
    _preview_task: Option<Task<()>>,
    /// Items from the last recursive walk, kept so re-ranking on each keystroke
    /// does not re-walk the tree.
    walked: Vec<(PathBuf, String)>,
    /// Repo lookups, negatives included: most directories are not in a
    /// repository, and without this every navigation stat-walks to `/`.
    repos: git::Cache,
    /// What git says about the current directory, if it is in a repository.
    git: Option<Git>,
    /// The repo and generation `git` is, or is being, loaded for — the same
    /// shape as `preview_request`, and for the same reason: a slow status read
    /// must not be requested again on every frame in between.
    git_request: Option<(PathBuf, u64)>,
    /// Bumped by anything that could change what git would say. Part of the
    /// request key, so a bump is what re-runs the read.
    git_generation: u64,
    /// Held: dropping the task cancels a status read for a repository we have
    /// since left.
    _git_task: Option<Task<()>>,
    /// Watches `.git`, so a commit or a switch made in a terminal is noticed.
    git_watcher: Option<DirWatcher>,
    /// A branch switch in flight. Held so it is not cancelled mid-checkout.
    _switch_task: Option<Task<()>>,
    /// The copied entry, waiting to be pasted. Ours rather than the system
    /// clipboard: gpui's clipboard speaks text and images, not file lists,
    /// and a path pasted as text into a terminal is still available via the
    /// system clipboard anyway (copy also writes the path there).
    clipboard: Option<PathBuf>,
    /// The copied entry was *cut*: pasting moves it, once.
    clipboard_cut: bool,
    /// The PNG conversions behind the copy-image modal, in flight.
    _copy_task: Option<Task<()>>,
    /// What the last action had to say, shown in the status bar (M9). The
    /// bool is urgency: errors read urgent, confirmations read plain.
    notice: Option<(String, bool)>,
    /// Bumped per notice, so an old notice's expiry timer cannot clear a newer
    /// one that replaced it.
    notice_generation: u64,
    _notice_task: Option<Task<()>>,
    /// An action's run-and-report task (share, open-with). Held so its report
    /// is not cancelled; replaced by the next action.
    _action_task: Option<Task<()>>,
    /// The effective keymap, for the palette's key hints (M11).
    keymap: keymap::Keymap,
    /// The settings that are not keys (`config.toml`), and where to write
    /// them back when a toggle flips one.
    config: config::Config,
    config_path: PathBuf,
    /// Where a dragged tab would land, while one is over a tab row: the
    /// insertion line the sidebar draws. Cleared by the drop, by the pointer
    /// leaving the rows, and by the drag ending anywhere else.
    tab_drop: Option<TabDrop>,
    /// Bumped per content-search keystroke, so a slow `rg` cannot land its
    /// results over a newer query's.
    grep_generation: u64,
    /// Held: the debounce and the running search live here.
    _grep_task: Option<Task<()>>,
    /// The recent-files walk in flight. Dropped when a newer view replaces it.
    _recent_task: Option<Task<()>>,
    /// Every detached serving process the registry knows (M10, revised to
    /// outlive the window). Refreshed from disk each frame — the registry is
    /// a couple of tiny files, and another window may have started one.
    servers: Vec<server::Info>,
    /// Repaints the open server menu while requests arrive, so the log is
    /// live. Ends itself when the menu closes or the server stops.
    _server_refresh: Option<Task<()>>,
    _theme: Subscription,
}

impl Explorer {
    fn new(
        start: PathBuf,
        keymap: keymap::Keymap,
        config: config::Config,
        config_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let home = home_dir();
        let config_dir = config_dir();
        let session_path = session_path();
        let session = Session::load(&session_path, start);
        let views = Views::load(&config_dir);

        let mut listings = HashMap::new();
        let mut cursors = HashMap::new();
        // Seed the active tab synchronously so the first frame has content;
        // the rest load lazily when switched to.
        if let Some(tab) = session.active_tab() {
            let listing = Listing::read_sorted(tab.path(), views.get(tab.path()).sort());
            if let Some(index) = tab
                .cursor_name
                .as_deref()
                .and_then(|n| listing.index_of_name(n))
                .or_else(|| first_visible(&listing, false))
            {
                cursors.insert(tab.id.clone(), index);
            }
            listings.insert(tab.id.clone(), listing);
        }

        let mut explorer = Self {
            places: Places::load(&home, &config_dir),
            network: network::load(&config_dir),
            place_cursor: 0,
            pane: Pane::Listing,
            sidebar_focus: cx.focus_handle(),
            config_dir,
            session_path,
            session,
            listings,
            cursors,
            selected: HashMap::new(),
            written_revision: 0,
            show_hidden: false,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            _pending: None,
            watcher: None,
            session_watcher: None,
            overlay: None,
            left_open: true,
            right_open: true,
            resizing: None,
            column_resizing: None,
            views,
            viewport_width: 0.0,
            left_scroll: gpui::ScrollHandle::new(),
            right_scroll: gpui::ScrollHandle::new(),
            input: input.clone(),
            // `subscribe_in` rather than `subscribe`: confirming can execute a
            // palette command, and dispatching an action needs the window.
            _input_events: cx.subscribe_in(
                &input,
                window,
                |this, _input, event: &InputEvent, window, cx| {
                    match event {
                        // Re-rank on every keystroke. Directory filtering is
                        // over an already-loaded list, so this is cheap; the
                        // recursive walk caches its items and only re-ranks.
                        InputEvent::Change => this.filter_overlay(cx),
                        InputEvent::PressEnter { .. } => this.confirm_overlay(window, cx),
                        _ => {}
                    }
                },
            ),
            search: Search::new(),
            preview: None,
            preview_request: None,
            _preview_task: None,
            walked: Vec::new(),
            repos: git::Cache::new(),
            git: None,
            git_request: None,
            git_generation: 0,
            _git_task: None,
            git_watcher: None,
            _switch_task: None,
            clipboard: None,
            clipboard_cut: false,
            _copy_task: None,
            notice: None,
            notice_generation: 0,
            _notice_task: None,
            _action_task: None,
            keymap,
            config,
            config_path,
            tab_drop: None,
            grep_generation: 0,
            _grep_task: None,
            _recent_task: None,
            servers: server::list(),
            _server_refresh: None,
            _theme: cx.observe_global::<Theme>(|_this, cx| {
                // Keep gpui-component's palette in step with ours, then repaint.
                let tokens = cx.theme().tokens.clone();
                omarchy_ui::sync_gpui_component(&tokens, cx);
                cx.notify();
            }),
        };
        explorer.watch_current(cx);
        explorer.watch_session(cx);
        // A broken keymap.toml must not stop the app (the defaults stand),
        // but it must not be silent either — a rebind that quietly did not
        // take reads as the key being broken.
        if !explorer.keymap.problems.is_empty() {
            let message = explorer.keymap.problems.join(" \u{b7} ");
            explorer.notify_user(message, cx);
        }
        explorer
    }

    /// Watch the session file so a second instance's changes converge here.
    ///
    /// ⚠ **Our own atomic rename trips this watcher.** Without the revision
    /// guard the reload clobbers whatever the user did between the write and
    /// the event — a race that only shows up when a write and an edit overlap,
    /// which is precisely why it is guarded rather than tested by hand.
    ///
    /// Watches the *parent directory*, not the file: the write is a rename, and
    /// a watch on the file itself detaches the first time it lands. Same lesson
    /// as the theme directory in `omarchy-tokens`.
    fn watch_session(&mut self, cx: &mut Context<Self>) {
        let Some(parent) = self.session_path.parent().map(Path::to_path_buf) else {
            return;
        };
        if std::fs::create_dir_all(&parent).is_err() {
            return;
        }
        let Ok(watcher) = DirWatcher::new(parent) else {
            return;
        };
        let events = watcher.events.clone();
        self.session_watcher = Some(watcher);

        cx.spawn(async move |this, cx| {
            loop {
                let outcome = cx
                    .background_spawn({
                        let events = events.clone();
                        async move { events.wait(Duration::from_millis(500)) }
                    })
                    .await;

                match outcome {
                    Wait::Closed => return,
                    Wait::Idle => {
                        if this.update(cx, |_, _| ()).is_err() {
                            return;
                        }
                    }
                    Wait::Changed => {
                        let ok = this.update(cx, |this, cx| this.absorb_session_change(cx));
                        if ok.is_err() {
                            return;
                        }
                    }
                }
            }
        })
        .detach();
    }

    /// Adopt a session written by another instance — but never our own.
    fn absorb_session_change(&mut self, cx: &mut Context<Self>) {
        let path = self.session_path.clone();
        let start = self.current_path();
        let incoming = Session::load(&path, start);

        // The guard. `written_revision` is what we last wrote; anything at or
        // below it is our own echo.
        if incoming.revision() <= self.written_revision {
            return;
        }

        self.session = incoming;
        self.written_revision = self.session.revision();
        // Listings are per-tab and the ids changed, so drop the caches and let
        // the active tab re-read.
        self.listings.clear();
        self.cursors.clear();
        self.reload(cx);
        cx.notify();
    }

    // ------------------------------------------------------------ active tab

    /// The active tab's id. Returned by value: almost every caller then needs
    /// `&mut self`, and holding a borrow across that fights the borrow checker
    /// for no benefit.
    fn tab_id(&self) -> String {
        self.session
            .active_tab()
            .map(|t| t.id.clone())
            .unwrap_or_default()
    }

    fn current_path(&self) -> PathBuf {
        self.session
            .active_tab()
            .map(|t| t.path().to_path_buf())
            .unwrap_or_else(home_dir)
    }

    /// `None` while the active tab's directory is still being read.
    fn listing(&self) -> Option<&Listing> {
        self.listings.get(&self.tab_id())
    }

    fn cursor(&self) -> Option<usize> {
        self.cursors.get(&self.tab_id()).copied()
    }

    /// Move the cursor and keep the durable name in step.
    fn set_cursor(&mut self, index: Option<usize>) {
        let id = self.tab_id();
        let name = index
            .and_then(|i| self.listings.get(&id).and_then(|l| l.get(i)))
            .map(|e| e.name.clone());

        match index {
            Some(i) => {
                self.cursors.insert(id.clone(), i);
            }
            None => {
                self.cursors.remove(&id);
            }
        }
        if let Some(tab) = self.session.active_tab_mut() {
            tab.cursor_name = name;
        }
    }

    // -------------------------------------------------------------- selection

    fn selected_names(&self) -> Option<&HashSet<String>> {
        self.selected.get(&self.tab_id())
    }

    fn is_selected(&self, name: &str) -> bool {
        self.selected_names()
            .is_some_and(|names| names.contains(name))
    }

    /// How many entries are marked in the active tab.
    fn selected_count(&self) -> usize {
        self.selected_names().map_or(0, HashSet::len)
    }

    /// The marked entries of the active tab, as listing indices in listing
    /// order, hidden ones left out: what a drag carries.
    fn selection(&self) -> Vec<usize> {
        let (Some(listing), Some(names)) = (self.listing(), self.selected_names()) else {
            return Vec::new();
        };
        listing
            .visible(self.show_hidden)
            .into_iter()
            .filter(|&index| listing.get(index).is_some_and(|e| names.contains(&e.name)))
            .collect()
    }

    fn clear_selection(&mut self) {
        self.selected.remove(&self.tab_id());
    }

    fn mark(&mut self, index: usize, on: bool) {
        let Some(name) = self
            .listing()
            .and_then(|l| l.get(index))
            .map(|e| e.name.clone())
        else {
            return;
        };
        let id = self.tab_id();
        let names = self.selected.entry(id.clone()).or_default();
        if on {
            names.insert(name);
        } else {
            names.remove(&name);
        }
        if names.is_empty() {
            self.selected.remove(&id);
        }
    }

    fn toggle_mark(&mut self, index: usize) {
        let on = !self
            .listing()
            .and_then(|l| l.get(index))
            .is_some_and(|e| self.is_selected(&e.name));
        self.mark(index, on);
    }

    /// Mark every visible entry between two indices, both included.
    fn mark_range(&mut self, from: usize, to: usize) {
        let Some(visible) = self.listing().map(|l| l.visible(self.show_hidden)) else {
            return;
        };
        let position = |index| visible.iter().position(|&i| i == index);
        let (Some(a), Some(b)) = (position(from), position(to)) else {
            return;
        };
        for &index in &visible[a.min(b)..=a.max(b)] {
            self.mark(index, true);
        }
    }

    /// `\u{21e7}\u{2193}` / `\u{21e7}\u{2191}`: mark the cursor row and the one
    /// it steps to, so holding the key sweeps a run.
    fn extend_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.overlay.is_some() || self.pane != Pane::Listing {
            return;
        }
        let Some(cursor) = self.cursor() else {
            return;
        };
        self.mark(cursor, true);
        self.move_cursor(delta, cx);
        if let Some(cursor) = self.cursor() {
            self.mark(cursor, true);
        }
        cx.notify();
    }

    /// Insert: flip the cursor entry and step down — the file-manager
    /// convention, so tapping it down a list picks out every other file.
    fn toggle_select(&mut self, cx: &mut Context<Self>) {
        if self.overlay.is_some() || self.pane != Pane::Listing {
            return;
        }
        let Some(cursor) = self.cursor() else {
            return;
        };
        self.toggle_mark(cursor);
        self.move_cursor(1, cx);
        cx.notify();
    }

    /// `^a`: every visible entry.
    fn select_all(&mut self, cx: &mut Context<Self>) {
        if self.overlay.is_some() || self.pane != Pane::Listing {
            return;
        }
        let Some(visible) = self.listing().map(|l| l.visible(self.show_hidden)) else {
            return;
        };
        for index in visible {
            self.mark(index, true);
        }
        cx.notify();
    }

    /// Forget marks on entries a tab's listing no longer has — a file that
    /// was moved or deleted must not stay marked in memory and come back
    /// should a file of the same name appear.
    fn prune_selection_of(&mut self, id: &str) {
        let Some(listing) = self.listings.get(id) else {
            return;
        };
        if let Some(names) = self.selected.get_mut(id) {
            names.retain(|name| listing.index_of_name(name).is_some());
            if names.is_empty() {
                self.selected.remove(id);
            }
        }
    }

    // ------------------------------------------------------------- navigation

    /// The name under the cursor, for history's cursor memory.
    fn cursor_name(&self) -> Option<String> {
        self.cursor()
            .and_then(|i| self.listing()?.get(i))
            .map(|e| e.name.clone())
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let id = self.tab_id();
        let Some(tab) = self.session.active_tab() else {
            return;
        };
        let path = tab.path().to_path_buf();
        let restore = tab
            .navigation
            .remembered_cursor()
            .map(str::to_string)
            .or_else(|| tab.cursor_name.clone());
        let sort = self.views.get(&path).sort();

        // Read off the main thread; the previous listing stays on screen until
        // the new one lands, so navigation never blanks the window.
        self._pending = Some(cx.spawn(async move |this, cx| {
            let listing = cx
                .background_spawn(async move { Listing::read_sorted(&path, sort) })
                .await;

            let _ = this.update(cx, |this, cx| {
                // A slower read for a tab the user has since left must not
                // overwrite the tab they are looking at now.
                let restored = restore
                    .as_deref()
                    .and_then(|n| listing.index_of_name(n))
                    .filter(|i| listing.visible(this.show_hidden).contains(i))
                    .or_else(|| first_visible(&listing, this.show_hidden));

                // Marks belong to a directory: they go with it, and a
                // re-read of the same one only sheds the marks on files
                // that have since gone.
                let same_dir = this
                    .listings
                    .get(&id)
                    .is_some_and(|old| old.path == listing.path);
                this.listings.insert(id.clone(), listing);
                if same_dir {
                    this.prune_selection_of(&id);
                } else {
                    this.selected.remove(&id);
                }
                if this.tab_id() == id {
                    this.set_cursor(restored);
                    this.scroll_to_cursor();
                    this.watch_current(cx);
                }
                this.persist_session();
                cx.notify();
            });
        }));
    }

    /// Put the cursor back on a named entry, if it is still present and visible.
    fn restore_cursor(&self, name: Option<&str>) -> Option<usize> {
        let listing = self.listing()?;
        let index = listing.index_of_name(name?)?;
        listing
            .visible(self.show_hidden)
            .contains(&index)
            .then_some(index)
    }

    fn open_selected(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.cursor().and_then(|i| self.listing()?.get(i)) else {
            return;
        };
        if is_navigable(entry) {
            let path = entry.path.clone();
            let leaving = self.cursor_name();
            if let Some(tab) = self.session.active_tab_mut() {
                tab.navigation.go(path, leaving.as_deref());
            }
            self.reload(cx);
            return;
        }
        // On a file, Enter hands it to the default application (M9). Failures
        // land in the status bar — which is what the gap in M3 was waiting on.
        let path = entry.path.clone();
        self.open_with_system(path, cx);
    }

    // ---------------------------------------------------------------- cursor

    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(visible) = self.listing().map(|l| l.visible(self.show_hidden)) else {
            return;
        };
        if visible.is_empty() {
            return;
        }
        let position = self
            .cursor()
            .and_then(|c| visible.iter().position(|&i| i == c))
            .unwrap_or(0) as isize;

        let next = (position + delta).clamp(0, visible.len() as isize - 1) as usize;
        self.set_cursor(Some(visible[next]));
        self.scroll_to_cursor();
        cx.notify();
    }

    fn move_to_edge(&mut self, last: bool, cx: &mut Context<Self>) {
        let Some(visible) = self.listing().map(|l| l.visible(self.show_hidden)) else {
            return;
        };
        let target = if last {
            visible.last().copied()
        } else {
            visible.first().copied()
        };
        self.set_cursor(target);
        self.scroll_to_cursor();
        cx.notify();
    }

    /// Keep the cursor row on screen — otherwise `j` past the fold moves a
    /// cursor the user cannot see.
    fn scroll_to_cursor(&mut self) {
        let Some(visible) = self.listing().map(|l| l.visible(self.show_hidden)) else {
            return;
        };
        if let Some(position) = self
            .cursor()
            .and_then(|c| visible.iter().position(|&i| i == c))
        {
            self.scroll.scroll_to_item(position, ScrollStrategy::Center);
        }
    }

    fn toggle_hidden(&mut self, cx: &mut Context<Self>) {
        self.show_hidden = !self.show_hidden;
        // The cursor may have been sitting on an entry we just hid.
        if let Some(visible) = self.listing().map(|l| l.visible(self.show_hidden))
            && !self.cursor().is_some_and(|c| visible.contains(&c))
        {
            let first = visible.first().copied();
            self.set_cursor(first);
        }
        self.scroll_to_cursor();
        cx.notify();
    }

    // ---------------------------------------------------------------- places

    /// True while a modal owns the window: the listing verbs must not act
    /// under one. Field-less modals (help, the menus) leave *focus* on the
    /// pane, so context-scoped bindings still resolve — this is the guard
    /// that keeps them from reaching the world behind the overlay.
    fn overlay_owns_input(&self) -> bool {
        self.overlay.is_some()
    }

    fn move_in_pane(&mut self, delta: isize, cx: &mut Context<Self>) {
        // An open overlay owns the arrow keys; the panes behind it do not move
        // under a modal. The expanded preview is not an overlay — it is the
        // listing column rendered differently — so `j`/`k` keep moving the
        // cursor there, which is what §6.5 asks for.
        if self.overlay.is_some() {
            self.move_overlay_cursor(delta, cx);
            return;
        }
        match self.pane {
            Pane::Sidebar => self.move_place_cursor(delta, cx),
            Pane::Listing => self.move_cursor(delta, cx),
        }
    }

    fn edge_in_pane(&mut self, last: bool, cx: &mut Context<Self>) {
        if self.overlay_owns_input() {
            return;
        }
        match self.pane {
            Pane::Sidebar => {
                let target = if last {
                    self.places.len().saturating_sub(1)
                } else {
                    0
                };
                self.place_cursor = target;
                cx.notify();
            }
            Pane::Listing => self.move_to_edge(last, cx),
        }
    }

    fn focus_handle_for(&self, pane: Pane) -> &FocusHandle {
        match pane {
            Pane::Sidebar => &self.sidebar_focus,
            Pane::Listing => &self.focus,
        }
    }

    fn focus_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        self.pane = pane;
        window.focus(self.focus_handle_for(pane), cx);
        cx.notify();
    }

    fn move_place_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.places.is_empty() {
            return;
        }
        let last = self.places.len() as isize - 1;
        self.place_cursor = (self.place_cursor as isize + delta).clamp(0, last) as usize;
        cx.notify();
    }

    /// Open a place — into a *tab*, never over one (revised on request from
    /// M5's "a place navigates the current tab"): a tab already sitting on
    /// that directory is brought forward, and otherwise a fresh tab opens.
    /// Either way, no tab loses where it was.
    fn open_place(&mut self, cx: &mut Context<Self>) {
        let Some(place) = self.places.get(self.place_cursor) else {
            return;
        };
        // A pinned directory can be removed or unmounted between sessions, so
        // never navigate blind — fall back the same way a stale tab will in M5.
        let target = nearest_existing(&place.path.clone());
        let existing = self
            .session
            .flat()
            .into_iter()
            .find(|(_, _, tab)| tab.path() == target)
            .map(|(workspace, tab, _)| (workspace, tab));
        match existing {
            Some((workspace, tab)) => {
                self.session.activate_tab(workspace, tab);
            }
            None => {
                self.session.new_tab(target);
            }
        }
        self.after_tab_change(cx);
    }

    fn pin_current(&mut self, cx: &mut Context<Self>) {
        let current = self.current_path().as_path().to_path_buf();
        if self.places.pin(&current) {
            self.persist_places();
            // Put the sidebar cursor on what was just pinned, so the effect of
            // the keystroke is visible rather than silent.
            self.place_cursor = self.places.len().saturating_sub(1);
            cx.notify();
        }
    }

    /// What the Pin button should say and do for a given directory.
    fn pin_state(&self, path: &Path) -> PinState {
        match self.places.index_of_path(path) {
            None => PinState::Pinnable,
            Some(index) => match self.places.pinned_index(index) {
                Some(_) => PinState::Pinned,
                // A system place (Home, Downloads, …): permanently in the
                // sidebar, so there is nothing to pin and nothing to remove.
                None => PinState::System,
            },
        }
    }

    /// The Pin buttons (M9's mouse actions, extended): pin a directory, or
    /// unpin it if it already is — one button, reading which way it points.
    fn toggle_pin(&mut self, path: &Path, cx: &mut Context<Self>) {
        match self.pin_state(path) {
            PinState::System => {}
            PinState::Pinned => {
                let Some(pin) = self
                    .places
                    .index_of_path(path)
                    .and_then(|index| self.places.pinned_index(index))
                else {
                    return;
                };
                if self.places.unpin(pin) {
                    self.persist_places();
                    self.place_cursor = self.place_cursor.min(self.places.len().saturating_sub(1));
                    cx.notify();
                }
            }
            PinState::Pinnable => {
                if self.places.pin(path) {
                    self.persist_places();
                    // Put the sidebar cursor on what was just pinned, so the
                    // effect is visible rather than silent.
                    self.place_cursor = self.places.len().saturating_sub(1);
                    cx.notify();
                }
            }
        }
    }

    fn unpin_selected(&mut self, cx: &mut Context<Self>) {
        let Some(pin) = self.places.pinned_index(self.place_cursor) else {
            return; // a system place: not the user's to remove
        };
        if self.places.unpin(pin) {
            self.persist_places();
            self.place_cursor = self.place_cursor.min(self.places.len().saturating_sub(1));
            cx.notify();
        }
    }

    fn move_pin(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(pin) = self.places.pinned_index(self.place_cursor) else {
            return;
        };
        if let Some(moved) = self.places.move_pin(pin, delta) {
            self.persist_places();
            self.place_cursor = self.places.system().len() + moved;
            cx.notify();
        }
    }

    /// Write the session, remembering the revision so the watcher can tell our
    /// own write apart from someone else's.
    fn persist_session(&mut self) {
        match self.session.save(&self.session_path) {
            Ok(revision) => self.written_revision = revision,
            Err(err) => eprintln!("omafiles: could not save session: {err}"),
        }
    }

    /// Save pins, reporting failure rather than losing it silently.
    fn persist_places(&self) {
        if let Err(err) = self.places.save(&self.config_dir) {
            eprintln!("omafiles: could not save places: {err}");
        }
    }

    // --------------------------------------------------------- tabs & spaces

    /// Open a tab on the current directory, **in the active workspace**.
    fn new_tab(&mut self, cx: &mut Context<Self>) {
        let path = self.current_path();
        self.session.new_tab(path);
        self.after_tab_change(cx);
    }

    /// Fold or unfold a workspace's tabs in the sidebar. Persisted like every
    /// other session change, so the sidebar reopens the way it was left.
    fn toggle_workspace_collapsed(&mut self, workspace: usize, cx: &mut Context<Self>) {
        self.session.toggle_collapsed(workspace);
        self.persist_session();
        cx.notify();
    }

    fn close_tab(&mut self, cx: &mut Context<Self>) {
        let workspace = self.session.active_workspace();
        let Some(tab) = self.session.workspace(workspace).map(|w| w.active_tab) else {
            return;
        };
        self.close_tab_at(workspace, tab, cx);
    }

    /// Close one specific tab — the sidebar's hover `×`, where the target is
    /// whichever row the pointer is on rather than the active tab.
    fn close_tab_at(&mut self, workspace: usize, tab: usize, cx: &mut Context<Self>) {
        let Some(id) = self
            .session
            .workspace(workspace)
            .and_then(|w| w.tabs.get(tab))
            .map(|t| t.id.clone())
        else {
            return;
        };
        if self.session.close_tab(workspace, tab) {
            // Drop the cached listing with the tab, or a long session leaks one
            // Vec<Entry> per directory ever opened.
            self.listings.remove(&id);
            self.cursors.remove(&id);
            self.after_tab_change(cx);
        }
    }

    /// Cycle through every tab in display order, across workspaces.
    fn cycle_tab(&mut self, delta: isize, cx: &mut Context<Self>) {
        let flat: Vec<(usize, usize)> = self
            .session
            .flat()
            .iter()
            .map(|(w, t, _)| (*w, *t))
            .collect();
        if flat.len() < 2 {
            return;
        }
        let active = self.session.active_workspace();
        let active_tab = self
            .session
            .workspace(active)
            .map(|w| w.active_tab)
            .unwrap_or(0);
        let position = flat
            .iter()
            .position(|&(w, t)| w == active && t == active_tab)
            .unwrap_or(0) as isize;

        // Wrapping is right here: cycling tabs is a loop, unlike reordering.
        let next = (position + delta).rem_euclid(flat.len() as isize) as usize;
        let (w, t) = flat[next];
        self.session.activate_tab(w, t);
        self.after_tab_change(cx);
    }

    fn activate_tab(&mut self, workspace: usize, tab: usize, cx: &mut Context<Self>) {
        if self.session.activate_tab(workspace, tab) {
            self.after_tab_change(cx);
        }
    }

    fn delete_workspace(&mut self, cx: &mut Context<Self>) {
        let active = self.session.active_workspace();
        if self.session.delete_workspace(active) {
            self.after_tab_change(cx);
        }
    }

    fn move_tab_to_next_workspace(&mut self, cx: &mut Context<Self>) {
        let count = self.session.workspaces().len();
        if count < 2 {
            return;
        }
        let from = self.session.active_workspace();
        let tab = self
            .session
            .workspace(from)
            .map(|w| w.active_tab)
            .unwrap_or(0);
        let to = (from + 1) % count;
        if self.session.move_tab(from, tab, to) {
            self.after_tab_change(cx);
        }
    }

    /// Everything a tab or workspace change needs: load the listing if we have
    /// not seen this tab, persist, repaint.
    fn after_tab_change(&mut self, cx: &mut Context<Self>) {
        let id = self.tab_id();
        if !self.listings.contains_key(&id) {
            self.reload(cx);
        } else {
            self.watch_current(cx);
        }
        self.persist_session();
        cx.notify();
    }

    // ---------------------------------------------------------------- search

    /// `^k`: the command palette — §6.8's discoverability mechanism, and the
    /// place a rebound key shows its *effective* binding.
    fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Some(Overlay::Palette {
            results: (0..COMMANDS.len()).collect(),
            cursor: 0,
        });
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();
    }

    /// Filter the palette. Substring rather than fuzzy: two dozen labels do
    /// not need a ranking model, and stable order beats clever order in a
    /// list this small.
    fn refresh_palette(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().to_lowercase();
        if let Some(Overlay::Palette { results, cursor }) = &mut self.overlay {
            *results = COMMANDS
                .iter()
                .enumerate()
                .filter(|(_, command)| command.label.to_lowercase().contains(&query))
                .map(|(i, _)| i)
                .collect();
            *cursor = (*cursor).min(results.len().saturating_sub(1));
            cx.notify();
        }
    }

    // ------------------------------------------------------------------ finder

    /// `/`, or the navigation bar's magnifier: one window for everything
    /// below the current directory. Empty, it shows the newest files; typed,
    /// fuzzy name matches first, then files whose contents match.
    fn open_finder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.current_path();
        self.overlay = Some(Overlay::Finder {
            root: root.clone(),
            query: String::new(),
            recent: Vec::new(),
            names: Vec::new(),
            hits: Vec::new(),
            scanning: true,
            searching: false,
            truncated_walk: false,
            truncated_grep: false,
            cursor: 0,
        });
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();

        // One background walk feeds both sections: its items become the fuzzy
        // corpus, and the newest hundred of them the recents — so the whole
        // window answers from one corpus, truncation flag included.
        let show_hidden = self.show_hidden;
        self.walked.clear();
        self._recent_task = Some(cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    let walk = walk(&root, show_hidden);
                    let mut newest: Vec<recent::RecentFile> = walk
                        .items
                        .iter()
                        .filter_map(|(path, _)| {
                            let meta = std::fs::metadata(path).ok()?;
                            if !meta.is_file() {
                                return None;
                            }
                            Some(recent::RecentFile {
                                path: path.clone(),
                                modified: meta.modified().ok()?,
                            })
                        })
                        .collect();
                    newest.sort_by_key(|file| std::cmp::Reverse(file.modified));
                    newest.truncate(recent::LIMIT);
                    (walk, newest)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                let (walk, newest) = outcome;
                this.walked = walk.items;
                if let Some(Overlay::Finder {
                    recent,
                    scanning,
                    truncated_walk,
                    ..
                }) = &mut this.overlay
                {
                    *recent = newest;
                    *scanning = false;
                    *truncated_walk = walk.truncated;
                }
                // The corpus may have landed after typing began.
                this.refresh_finder(cx);
            });
        }));
    }

    /// A keystroke in the finder: names re-rank instantly, contents follow a
    /// debounce. The generation guard is the grep modal's, inherited.
    fn refresh_finder(&mut self, cx: &mut Context<Self>) {
        let typed = self.input.read(cx).value().trim().to_string();
        self.grep_generation = self.grep_generation.wrapping_add(1);
        let generation = self.grep_generation;

        let Some(Overlay::Finder { root, .. }) = &self.overlay else {
            return;
        };
        let root = root.clone();

        // Fuzzy over the walked corpus — instant, and empty while the walk is
        // still out (the scanning line says so).
        let ranked = if typed.is_empty() {
            Vec::new()
        } else {
            let items = std::mem::take(&mut self.walked);
            let borrowed = items
                .iter()
                .enumerate()
                .map(|(i, (path, label))| (i, label.as_str(), path.clone()));
            let mut ranked = self.search.rank(&typed, borrowed);
            self.walked = items;
            // Sixty names is past the point of scanning a list; the content
            // section still gets its turn below them.
            ranked.truncate(60);
            ranked
        };

        if let Some(Overlay::Finder {
            query,
            names,
            hits,
            searching,
            truncated_grep,
            cursor,
            ..
        }) = &mut self.overlay
        {
            *query = typed.clone();
            *names = ranked;
            *cursor = 0;
            if typed.chars().count() < 2 {
                hits.clear();
                *searching = false;
                *truncated_grep = false;
                self._grep_task = None;
                cx.notify();
                return;
            }
            *searching = true;
        }
        cx.notify();

        self._grep_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            if this
                .read_with(cx, |this, _| this.grep_generation != generation)
                .unwrap_or(true)
            {
                return;
            }
            let query = typed.clone();
            let outcome = cx
                .background_spawn(async move { grep::search(&root, &query) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.grep_generation != generation {
                    return;
                }
                match outcome {
                    Ok(found) => {
                        if let Some(Overlay::Finder {
                            hits,
                            searching,
                            truncated_grep,
                            ..
                        }) = &mut this.overlay
                        {
                            *hits = found.hits;
                            *truncated_grep = found.truncated;
                            *searching = false;
                        }
                    }
                    Err(message) => {
                        if let Some(Overlay::Finder { searching, .. }) = &mut this.overlay {
                            *searching = false;
                        }
                        this.notify_user(message, cx);
                    }
                }
                cx.notify();
            });
        }));
    }

    // ----------------------------------------------------- network locations

    /// `^⇧n`: remember a network location — one field, the URI. GVfs does
    /// the rest when it is opened.
    fn open_add_network(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Some(Overlay::AddNetwork);
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();
    }

    /// Enter in the add dialog: validate, name, persist. An invalid URI keeps
    /// the dialog open with a notice — closing on garbage loses the typing.
    fn confirm_add_network(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let uri = self.input.read(cx).value().trim().to_string();
        if !network::looks_like_uri(&uri) {
            self.notify_user(
                "that does not look like a URI \u{2014} smb://host/share, sftp://user@host/\u{2026}"
                    .to_string(),
                cx,
            );
            return;
        }
        if self.network.iter().any(|l| l.uri == uri) {
            self.dismiss_overlay(window, cx);
            return; // already there; adding twice helps nobody
        }
        self.network.push(network::Location {
            name: network::derive_name(&uri),
            uri,
        });
        if let Err(err) = network::save(&self.config_dir, &self.network) {
            self.notify_user(format!("could not save network.toml: {err}"), cx);
        }
        self.dismiss_overlay(window, cx);
    }

    /// Open a network location: navigate straight to it when mounted, mount
    /// first when not — on the background executor, because a mount talks to
    /// a network and §6.5's rule about keystrokes applies to mounts too.
    fn open_network(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(location) = self.network.get(index).cloned() else {
            return;
        };
        if let Some(path) = network::mount_point(&location.uri) {
            let leaving = self.cursor_name();
            if let Some(tab) = self.session.active_tab_mut() {
                tab.navigation.go(path, leaving.as_deref());
            }
            self.reload(cx);
            return;
        }

        self.inform_user(format!("mounting {}\u{2026}", location.name), cx);
        self._action_task = Some(cx.spawn(async move |this, cx| {
            let uri = location.uri.clone();
            let outcome = cx
                .background_spawn(async move { network::mount(&uri) })
                .await;
            let _ = this.update(cx, |this, cx| match outcome {
                Ok(()) => match network::mount_point(&location.uri) {
                    Some(path) => {
                        let leaving = this.cursor_name();
                        if let Some(tab) = this.session.active_tab_mut() {
                            tab.navigation.go(path, leaving.as_deref());
                        }
                        this.reload(cx);
                    }
                    // Mounted but not where we looked: GVfs named it in a way
                    // the heuristic missed. Say so rather than doing nothing.
                    None => this.notify_user(
                        format!(
                            "mounted, but could not find where \u{2014} look under {}",
                            std::env::var("XDG_RUNTIME_DIR").unwrap_or_default() + "/gvfs"
                        ),
                        cx,
                    ),
                },
                // gio's own words: "authentication required" and friends are
                // exactly what the user needs to hear.
                Err(message) => this.notify_user(message, cx),
            });
        }));
    }

    /// The context menu's Unmount: hand the mount back to GVfs.
    ///
    /// If the tab was inside it, land on the nearest surviving ancestor —
    /// showing an unreadable-directory error for a mount *we* just removed
    /// would be blaming the user for our own action.
    fn unmount_network(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(location) = self.network.get(index).cloned() else {
            return;
        };
        let inside = network::mount_point(&location.uri)
            .is_some_and(|mount| self.current_path().starts_with(&mount));
        self._action_task = Some(cx.spawn(async move |this, cx| {
            let uri = location.uri.clone();
            let outcome = cx
                .background_spawn(async move { network::unmount(&uri) })
                .await;
            let _ = this.update(cx, |this, cx| match outcome {
                Ok(()) => {
                    this.inform_user(format!("unmounted {}", location.name), cx);
                    if inside {
                        let target = nearest_existing(&this.current_path());
                        let leaving = this.cursor_name();
                        if let Some(tab) = this.session.active_tab_mut() {
                            tab.navigation.go(target, leaving.as_deref());
                        }
                        this.reload(cx);
                    }
                }
                Err(message) => this.notify_user(message, cx),
            });
        }));
    }

    /// Right click on a network row: its actions, gathered like an entry's.
    fn open_network_menu(
        &mut self,
        index: usize,
        position: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.network.len() {
            return;
        }
        self.overlay = Some(Overlay::NetworkMenu { index, position });
        let owner = self.focus_handle_for(self.pane).clone();
        window.focus(&owner, cx);
        cx.notify();
    }

    /// The menu's Forget: drop a location from the list. Forgetting does not
    /// unmount — the mount belongs to GVfs, not to us.
    fn remove_network(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.network.len() {
            self.network.remove(index);
            if let Err(err) = network::save(&self.config_dir, &self.network) {
                self.notify_user(format!("could not save network.toml: {err}"), cx);
            }
            cx.notify();
        }
    }

    fn open_workspace_prompt(
        &mut self,
        editing: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = editing
            .and_then(|i| self.session.workspace(i))
            .and_then(|w| w.name.clone())
            .unwrap_or_default();

        self.overlay = Some(Overlay::Workspace { editing });
        self.input.update(cx, |input, cx| {
            input.set_value(existing, window, cx);
        });
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();
    }

    // ---------------------------------------------------------------- preview

    /// Keep the preview in step with the cursor.
    ///
    /// Called from `render` rather than from each of the dozen places that move
    /// the cursor — opening a tab, absorbing another instance's session, a
    /// directory changing under the watcher. The key check makes it free when
    /// nothing changed, and a single call site cannot be forgotten the way a
    /// dozen can.
    fn refresh_preview(&mut self, cx: &mut Context<Self>) {
        let entry = self.cursor().and_then(|i| self.listing()?.get(i)).cloned();
        let theme_name = cx.theme().tokens.theme_name.clone();
        // The git state is part of the key rather than a generation counter, so
        // the reload happens exactly when it changes this file's preview: the
        // moment the first status lands and turns it into a diff, and again when
        // a commit turns it back into a file. An unrelated change elsewhere in
        // the repository re-reads nothing.
        let state = entry.as_ref().and_then(|entry| self.git_state(&entry.path));
        let wanted = entry
            .as_ref()
            .map(|entry| (preview::Key::of(entry), theme_name, state));

        if self.preview_request == wanted {
            return;
        }
        self.preview_request = wanted;
        self.preview = None;

        let Some(entry) = entry else {
            self._preview_task = None;
            return;
        };

        // Cloned rather than read inside the task: the task runs on a
        // background thread and cannot touch the theme global.
        let syntax = cx.theme().syntax().clone();
        let key = preview::Key::of(&entry);
        // §6.9's diff view. Only for a file git says has changed against HEAD —
        // an untracked file has no diff, and a newly added one is better read
        // than seen as one long run of `+` lines.
        let diff_from = matches!(
            state,
            Some(git::State::Modified) | Some(git::State::Conflicted)
        )
        .then(|| self.git.as_ref().map(|git| git.repo.clone()))
        .flatten();

        self._preview_task = Some(cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    let mut preview = Preview::load(&entry);
                    let diff = match diff_from {
                        Some(repo) => with_diff(&mut preview, &repo, &entry.path, &syntax),
                        None => Vec::new(),
                    };
                    // Only one of the two is ever non-empty: `with_diff`
                    // replaces the body it read, so a diff has no text body left
                    // to colour.
                    let highlights = highlight(&preview, &syntax);
                    Loaded {
                        preview,
                        highlights,
                        diff,
                    }
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                // A read for an entry the cursor has since left must not land
                // over the one it is on now.
                if this.preview_request.as_ref().map(|(k, _, _)| k) == Some(&key) {
                    this.preview = Some(loaded);
                    cx.notify();
                }
            });
        }));
    }

    /// Whether the active tab has the preview expanded over its listing.
    fn preview_expanded(&self) -> bool {
        self.session
            .active_tab()
            .is_some_and(|tab| tab.preview_expanded)
    }

    /// `Space`: expand the preview over the listing column, and back again.
    ///
    /// The state lives on the tab, so switching tabs shows each as it was left
    /// rather than carrying one tab's mode onto another.
    fn toggle_preview(&mut self, cx: &mut Context<Self>) {
        // Space over a prompt belongs to the field, and over the help sheet it
        // would change something the user cannot see.
        if self.overlay.is_some() {
            return;
        }

        let expanded = self.preview_expanded();
        // Expanding onto a directory or a hex dump would replace the listing
        // with something that has less to say than the pane already showed.
        if !expanded
            && !self
                .preview
                .as_ref()
                .is_some_and(|l| l.preview.body.is_expandable())
        {
            return;
        }

        if let Some(tab) = self.session.active_tab_mut() {
            tab.preview_expanded = !expanded;
        }
        self.persist_session();
        cx.notify();
    }

    /// `Escape`: collapse the preview before anything else claims the key.
    fn collapse_preview(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.preview_expanded() {
            return false;
        }
        if let Some(tab) = self.session.active_tab_mut() {
            tab.preview_expanded = false;
        }
        self.persist_session();
        cx.notify();
        true
    }

    // ---------------------------------------------------------- actions (M9)

    /// Put a sentence in the status bar, and take it away again.
    ///
    /// The expiry is generation-guarded: a notice that has already been
    /// replaced must not have its slower timer clear the newer one.
    fn notify_user(&mut self, message: String, cx: &mut Context<Self>) {
        self.post_notice(message, true, cx);
    }

    /// A quiet confirmation — same slot, plain colour.
    fn inform_user(&mut self, message: String, cx: &mut Context<Self>) {
        self.post_notice(message, false, cx);
    }

    fn post_notice(&mut self, message: String, urgent: bool, cx: &mut Context<Self>) {
        self.notice = Some((message, urgent));
        self.notice_generation = self.notice_generation.wrapping_add(1);
        let generation = self.notice_generation;

        self._notice_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(6)).await;
            let _ = this.update(cx, |this, cx| {
                if this.notice_generation == generation {
                    this.notice = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    /// `t`: a terminal in the current directory.
    ///
    /// Inline: the launch is a spawn that either takes or fails immediately,
    /// and the terminal itself is not ours to wait for.
    fn open_terminal_here(&mut self, cx: &mut Context<Self>) {
        if let Err(message) = actions::open_terminal(&self.current_path()) {
            self.notify_user(message, cx);
        }
    }

    /// `a`: compose a prompt about the entry under the cursor, then launch the
    /// default agent beside it.
    fn ask_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // On a file: its directory, and a prompt naming it. On a directory —
        // or on nothing — the directory itself.
        let entry = self.cursor().and_then(|i| self.listing()?.get(i)).cloned();
        let (cwd, prompt) = match entry {
            Some(entry) if entry.kind.is_dir() => {
                let prompt = actions::compose_prompt(&entry.name, true);
                (entry.path, prompt)
            }
            Some(entry) => {
                let cwd = entry
                    .path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.current_path());
                (cwd, actions::compose_prompt(&entry.name, false))
            }
            None => (
                self.current_path(),
                "Look at this directory and help me with it.".to_string(),
            ),
        };
        self.ask_agent_in(cwd, prompt, window, cx);
    }

    /// The status bar's Agent button: about the directory itself, wherever the
    /// cursor happens to sit.
    fn ask_agent_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.current_path();
        self.ask_agent_in(
            cwd,
            "Look at this directory and help me with it.".to_string(),
            window,
            cx,
        );
    }

    /// Open the prompt dialog for a given directory and prefill.
    ///
    /// The no-agent case surfaces Omarchy's own picker (§6.6): the user chose
    /// no agent, so the useful response is the choosing, not an error about
    /// not having chosen.
    fn ask_agent_in(
        &mut self,
        cwd: PathBuf,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let agent = match actions::default_agent() {
            Ok(Some(agent)) => agent,
            Ok(None) => {
                match actions::summon_agent_picker() {
                    Ok(()) => self.notify_user(
                        "no default agent yet — pick one, then ask again".to_string(),
                        cx,
                    ),
                    Err(message) => self.notify_user(message, cx),
                }
                return;
            }
            Err(message) => {
                self.notify_user(message, cx);
                return;
            }
        };

        self.overlay = Some(Overlay::Agent { cwd, agent });
        self.input.update(cx, |input, cx| {
            input.set_value(prompt, window, cx);
        });
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();
    }

    /// Enter in the agent dialog: launch, and only speak up on failure.
    fn confirm_agent(&mut self, cx: &mut Context<Self>) {
        let Some(Overlay::Agent { cwd, .. }) = self.overlay.take() else {
            return;
        };
        let prompt = self.input.read(cx).value().trim().to_string();
        if prompt.is_empty() {
            // Launching an agent with nothing to say helps nobody; treat it
            // as a cancel.
            cx.notify();
            return;
        }
        if let Err(message) = actions::agent_prompt(&prompt, &cwd) {
            self.notify_user(message, cx);
        }
        cx.notify();
    }

    /// `s`: share the entry under the cursor via LocalSend.
    fn share_selected(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.cursor().and_then(|i| self.listing()?.get(i)).cloned() else {
            return;
        };
        let is_dir = entry.kind.is_dir();
        self.share_path(entry.path, is_dir, cx);
    }

    /// The status bar's Share button: the current directory as a folder.
    fn share_here(&mut self, cx: &mut Context<Self>) {
        let path = self.current_path();
        self.share_path(path, true, cx);
    }

    /// Share one path via LocalSend.
    ///
    /// Run-and-report on the background executor: the script exits once
    /// systemd-run has LocalSend, but that is still a fork and a wait that do
    /// not belong on a keystroke.
    fn share_path(&mut self, path: PathBuf, is_dir: bool, cx: &mut Context<Self>) {
        self._action_task = Some(cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move { actions::share(&path, is_dir) })
                .await;
            if let Err(message) = outcome {
                let _ = this.update(cx, |this, cx| this.notify_user(message, cx));
            }
        }));
    }

    /// Enter on a file: hand it to the default application, and report a
    /// failure in the status bar — the reporting is why this waited for M9.
    fn open_with_system(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self._action_task = Some(cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move { actions::open_path(&path) })
                .await;
            if let Err(message) = outcome {
                let _ = this.update(cx, |this, cx| this.notify_user(message, cx));
            }
        }));
    }

    // ------------------------------------------------- copy · paste · compress

    /// `^c`: remember the entry under the cursor for pasting — or, on a
    /// picture, ask *what* to copy: the file, or PNG bytes at a size.
    fn copy_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.cursor().and_then(|i| self.listing()?.get(i)).cloned() else {
            return;
        };
        if matches!(entry.kind, Kind::File) && preview::is_image(&entry.path) {
            self.open_copy_image(entry, window, cx);
            return;
        }
        self.copy_file(entry.path, entry.name, cx);
    }

    /// The file itself goes on our clipboard, for a paste into a directory.
    ///
    /// The path also goes to the system clipboard as text, so a terminal can
    /// paste it even though gpui's clipboard cannot carry a file list.
    fn copy_file(&mut self, path: PathBuf, name: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
            path.to_string_lossy().into_owned(),
        ));
        self.clipboard = Some(path);
        self.clipboard_cut = false;
        self.inform_user(format!("copied \u{201c}{name}\u{201d}"), cx);
    }

    /// `^x`: like copy, but the paste moves it. Nothing happens to the file
    /// until then — a cut that vanishes the entry on the spot is a delete
    /// with extra steps, and a crash in between would lose it.
    fn cut_selected(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.cursor().and_then(|i| self.listing()?.get(i)).cloned() else {
            return;
        };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
            entry.path.to_string_lossy().into_owned(),
        ));
        self.clipboard = Some(entry.path);
        self.clipboard_cut = true;
        self.inform_user(
            format!(
                "cut \u{201c}{}\u{201d} \u{2014} paste to move it",
                entry.name
            ),
            cx,
        );
    }

    /// `^\u{21e7}c`: the path alone, as text, for a terminal or a chat. Our
    /// file clipboard is left as it was.
    fn copy_path_selected(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.cursor().and_then(|i| self.listing()?.get(i)).cloned() else {
            return;
        };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
            entry.path.to_string_lossy().into_owned(),
        ));
        self.inform_user(
            format!("copied the path of \u{201c}{}\u{201d}", entry.name),
            cx,
        );
    }

    /// The copy-image modal, with its conversions started.
    ///
    /// The picture's size is read first (from the header, or ffprobe for the
    /// formats whose header the preview does not parse), which decides the
    /// rows; then each PNG is made in turn, largest first, and lands in the
    /// open modal — or nowhere, if the modal was closed meanwhile.
    fn open_copy_image(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        let path = entry.path.clone();
        self.overlay = Some(Overlay::CopyImage {
            path: path.clone(),
            name: entry.name,
            variants: Vec::new(),
            encoded: Vec::new(),
            cursor: 0,
        });
        let owner = self.focus_handle_for(self.pane).clone();
        window.focus(&owner, cx);
        cx.notify();

        self._copy_task = Some(cx.spawn(async move |this, cx| {
            let probe = path.clone();
            let dimensions = cx
                .background_spawn(async move {
                    std::fs::read(&probe)
                        .ok()
                        .and_then(|bytes| preview::image_dimensions(&bytes))
                        .or_else(|| imageops::probe_dimensions(&probe))
                })
                .await;
            let variants = match dimensions {
                Some((w, h)) => imageops::variants(w, h),
                // Unknown size: the original alone, unlabelled.
                None => imageops::variants(0, 0),
            };
            let published = variants.clone();
            let live = this
                .update(cx, |this, cx| match &mut this.overlay {
                    Some(Overlay::CopyImage {
                        path: open,
                        variants,
                        encoded,
                        ..
                    }) if *open == path => {
                        *encoded = vec![None; published.len()];
                        *variants = published;
                        cx.notify();
                        true
                    }
                    _ => false,
                })
                .unwrap_or(false);
            if !live {
                return;
            }
            for (i, variant) in variants.iter().enumerate() {
                let source = path.clone();
                let width = (!variant.original).then_some(variant.width);
                let result = cx
                    .background_spawn(async move { imageops::png_bytes(&source, width) })
                    .await;
                let live = this
                    .update(cx, |this, cx| match &mut this.overlay {
                        Some(Overlay::CopyImage {
                            path: open,
                            encoded,
                            ..
                        }) if *open == path => {
                            if let Some(slot) = encoded.get_mut(i) {
                                *slot = Some(result);
                            }
                            cx.notify();
                            true
                        }
                        _ => false,
                    })
                    .unwrap_or(false);
                if !live {
                    return;
                }
            }
        }));
    }

    /// Enter, or a click, in the copy-image modal: the row under the cursor.
    fn confirm_copy_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::CopyImage {
            path,
            name,
            variants,
            encoded,
            cursor,
        }) = &self.overlay
        else {
            return;
        };
        let (path, name, cursor) = (path.clone(), name.clone(), *cursor);
        if cursor == 0 {
            self.dismiss_overlay(window, cx);
            self.copy_file(path, name, cx);
            return;
        }
        let Some(variant) = variants.get(cursor - 1).cloned() else {
            return;
        };
        match encoded.get(cursor - 1).and_then(|slot| slot.as_ref()) {
            Some(Ok(bytes)) => {
                // Through wl-copy, not gpui (see `imageops`), and off the
                // main thread: handing megabytes to a pipe is not a thing to
                // do between two frames.
                let bytes = bytes.clone();
                self.dismiss_overlay(window, cx);
                let size = if variant.width > 0 {
                    format!(", {} \u{d7} {}", variant.width, variant.height)
                } else {
                    String::new()
                };
                self._action_task = Some(cx.spawn(async move |this, cx| {
                    let outcome = cx
                        .background_spawn(async move { imageops::copy_png_to_clipboard(&bytes) })
                        .await;
                    let _ = this.update(cx, |this, cx| match outcome {
                        Ok(()) => this
                            .inform_user(format!("copied \u{201c}{name}\u{201d} as PNG{size}"), cx),
                        Err(why) => this.notify_user(why, cx),
                    });
                }));
            }
            Some(Err(why)) => self.notify_user(why.clone(), cx),
            // Still converting: the row says so; Enter again once it lands.
            None => {}
        }
    }

    /// `^v`: paste the copied entry into the current directory.
    fn paste_here(&mut self, cx: &mut Context<Self>) {
        let target = self.current_path();
        self.paste_into(target, cx);
    }

    /// Paste into `dest` — the current directory, or a directory picked from
    /// the context menu.
    ///
    /// Ours first: an entry copied or cut here. Failing that, the system
    /// clipboard — a picture from a browser lands as a file, files copied
    /// from another program (a `text/uri-list`) are copied in, and a path
    /// copied as text is treated as the file it names.
    fn paste_into(&mut self, dest: PathBuf, cx: &mut Context<Self>) {
        if let Some(source) = self.clipboard.clone() {
            let cut = self.clipboard_cut;
            if cut {
                // A cut pastes once: the file has moved, there is nothing
                // left at the path to paste again.
                self.clipboard = None;
                self.clipboard_cut = false;
            }
            self.land_file(
                cx,
                move || {
                    if cut {
                        fileops::move_into(&source, &dest)
                    } else {
                        fileops::copy_into(&source, &dest)
                    }
                },
                if cut { "moved" } else { "pasted" },
            );
            return;
        }

        let Some(item) = cx.read_from_clipboard() else {
            self.notify_user("nothing to paste".to_string(), cx);
            return;
        };
        for entry in item.entries() {
            match entry {
                gpui::ClipboardEntry::Image(image) => {
                    let (bytes, extension) = (image.bytes.clone(), image_extension(image.format));
                    let name = format!("Pasted image.{extension}");
                    self.land_file(
                        cx,
                        move || fileops::write_new(&dest, &name, &bytes),
                        "pasted",
                    );
                    return;
                }
                gpui::ClipboardEntry::ExternalPaths(paths) => {
                    if let Some(source) = paths.paths().first().cloned() {
                        self.land_file(cx, move || fileops::copy_into(&source, &dest), "pasted");
                        return;
                    }
                }
                gpui::ClipboardEntry::String(text) => {
                    let candidate = PathBuf::from(expand_tilde(text.text().trim()));
                    if candidate.is_absolute() && candidate.exists() {
                        self.land_file(cx, move || fileops::copy_into(&candidate, &dest), "pasted");
                        return;
                    }
                }
            }
        }
        self.notify_user(
            "nothing on the clipboard to paste as a file".to_string(),
            cx,
        );
    }

    /// Run a file-making operation in the background and put the cursor on
    /// what it made. One landing for paste, move-by-paste and a dropped
    /// picture: the watcher reloads, and remembering the name is what puts
    /// the cursor there when it does.
    fn land_file(
        &mut self,
        cx: &mut Context<Self>,
        operation: impl FnOnce() -> Result<PathBuf, String> + Send + 'static,
        verb: &'static str,
    ) {
        // Background: a directory tree can be arbitrarily large, and §6.5's
        // rule about keystrokes applies to copies too.
        self._action_task = Some(cx.spawn(async move |this, cx| {
            let outcome = cx.background_spawn(async move { operation() }).await;
            let _ = this.update(cx, |this, cx| match outcome {
                Ok(target) => {
                    let name = target
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if let Some(tab) = this.session.active_tab_mut() {
                        tab.cursor_name = Some(name.clone());
                    }
                    this.inform_user(format!("{verb} as \u{201c}{name}\u{201d}"), cx);
                }
                Err(message) => this.notify_user(message, cx),
            });
        }));
    }

    /// `\u{2326}`: ask before the entry under the cursor goes to the trash.
    fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.cursor().and_then(|i| self.listing()?.get(i)).cloned() else {
            return;
        };
        self.overlay = Some(Overlay::Delete {
            path: entry.path,
            name: entry.name,
            is_dir: entry.kind.is_dir(),
        });
        let owner = self.focus_handle_for(self.pane).clone();
        window.focus(&owner, cx);
        cx.notify();
    }

    /// Enter in the delete modal: to the trash, in the background.
    fn confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Delete { path, name, .. }) = &self.overlay else {
            return;
        };
        let (path, name) = (path.clone(), name.clone());
        self.dismiss_overlay(window, cx);
        // A trashed entry cannot be pasted; forget it rather than fail later.
        if self.clipboard.as_deref() == Some(path.as_path()) {
            self.clipboard = None;
            self.clipboard_cut = false;
        }
        self._action_task = Some(cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move { fileops::trash(&path) })
                .await;
            let _ = this.update(cx, |this, cx| match outcome {
                Ok(()) => {
                    this.inform_user(format!("moved \u{201c}{name}\u{201d} to the trash"), cx)
                }
                Err(message) => this.notify_user(message, cx),
            });
        }));
    }

    /// `z`: zip the entry under the cursor, beside it.
    fn compress_selected(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.cursor().and_then(|i| self.listing()?.get(i)).cloned() else {
            return;
        };
        self._action_task = Some(cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move { fileops::compress(&entry.path) })
                .await;
            let _ = this.update(cx, |this, cx| match outcome {
                Ok(archive) => {
                    let name = archive
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if let Some(tab) = this.session.active_tab_mut() {
                        tab.cursor_name = Some(name.clone());
                    }
                    this.inform_user(format!("created \u{201c}{name}\u{201d}"), cx);
                }
                Err(message) => this.notify_user(message, cx),
            });
        }));
    }

    // ------------------------------------------------------------ context menu

    /// `shift-F10`, or a right click on a row: the entry's actions, gathered.
    ///
    /// The keyboard route opens it centred; a click opens it where the click
    /// was.
    fn open_entry_menu(
        &mut self,
        position: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.cursor().and_then(|i| self.listing()?.get(i)).cloned() else {
            return;
        };
        self.overlay = Some(Overlay::Context {
            path: entry.path.clone(),
            name: entry.name.clone(),
            is_dir: entry.kind.is_dir(),
            position,
        });
        let owner = self.focus_handle_for(self.pane).clone();
        window.focus(&owner, cx);
        cx.notify();
    }

    // ------------------------------------------------------------------ recent
    // -------------------------------------------------------------------- help

    /// `?`, or the status bar's Help: the shortcut sheet, with a filter.
    ///
    /// The field takes focus, which is also what keeps the listing's own
    /// bindings from firing under the sheet.
    fn show_help(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Some(Overlay::Help);
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();
    }

    // ------------------------------------------------------ http server (M10)

    /// The running server whose root is exactly `path`, if any.
    fn server_index_for(&self, path: &Path) -> Option<usize> {
        self.servers.iter().position(|handle| handle.root == path)
    }

    /// `^s`, or the status-bar button: the *current directory's* server menu.
    ///
    /// One menu for both states — the overlay itself is stateless and reads
    /// the servers at render time, so it shows the start options when this
    /// directory serves nothing and the log and stop when it does. Other
    /// directories' servers live in the globe list (`^⇧s`).
    fn open_server_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.current_path();
        self.open_server_log(root, window, cx);
    }

    /// One server's menu — its log when running, the start options otherwise.
    fn open_server_log(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Some(Overlay::Server { root });
        // No input field in this menu: focus stays with the pane, like the
        // help sheet, so Escape keeps working.
        let owner = self.focus_handle_for(self.pane).clone();
        window.focus(&owner, cx);
        self.watch_server(cx);
        cx.notify();
    }

    /// The globe: every running server, wherever its root.
    fn open_server_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Some(Overlay::Servers);
        let owner = self.focus_handle_for(self.pane).clone();
        window.focus(&owner, cx);
        self.watch_server(cx);
        cx.notify();
    }

    /// Start serving the current directory. The root is pinned here — this is
    /// the moment §6.7 means by "the directory that was current when started".
    /// Several servers may run at once; the port conflict resolves itself
    /// because only the first gets 8080 and the OS picks for the rest.
    fn start_server(&mut self, lan: bool, cx: &mut Context<Self>) {
        let root = self.current_path();
        if self.server_index_for(&root).is_some() {
            return; // this directory is already being served
        }
        match server::spawn_detached(&root, lan) {
            // The child binds and registers itself; the watcher's next tick
            // sees it appear. A bind failure dies in the child — the entry
            // simply never shows, and the menu stays on the start options.
            Ok(()) => self.watch_server(cx),
            Err(message) => self.notify_user(message, cx),
        }
        cx.notify();
    }

    /// Stop one serving process — SIGTERM to its pid, registry swept.
    fn stop_server_at(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(info) = self.servers.get(index) {
            match server::stop(info.pid) {
                Ok(()) => self.servers = server::list(),
                Err(message) => self.notify_user(message, cx),
            }
        }
        cx.notify();
    }

    /// Keep an open server view's log and counters live.
    ///
    /// A half-second repaint tick, only while one of the server overlays is
    /// open *and* something is serving; the task ends itself the moment
    /// either stops being true. The serving threads cannot call `cx.notify`
    /// — this poll is the bridge.
    fn watch_server(&mut self, cx: &mut Context<Self>) {
        self._server_refresh = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                let keep = this.update(cx, |this, cx| {
                    // Live while a server view is open — even with nothing
                    // serving yet, because a just-spawned child registers
                    // itself a beat after the click that started it.
                    let live = matches!(
                        this.overlay,
                        Some(Overlay::Server { .. }) | Some(Overlay::Servers)
                    );
                    if live {
                        this.servers = server::list();
                        cx.notify();
                    }
                    live
                });
                if !matches!(keep, Ok(true)) {
                    return;
                }
            }
        }));
    }

    // -------------------------------------------------------------------- git

    /// Keep the git picture in step with the current directory.
    ///
    /// Called from `render` for the same reason [`Self::refresh_preview`] is:
    /// half a dozen paths change the directory, and a key comparison here is
    /// cheaper than remembering to call it from all of them.
    ///
    /// The head is read inline because it is two filesystem reads. The status is
    /// the 400 ms half (§6.9's measurement on a 4,288-file repo), so it goes to
    /// the background and the listing renders without markers until it lands.
    fn refresh_git(&mut self, cx: &mut Context<Self>) {
        let directory = self.current_path();
        let repo = self.repos.repo_for(&directory);
        let wanted = repo
            .as_ref()
            .map(|repo| (repo.root.clone(), self.git_generation));

        if self.git_request == wanted {
            return;
        }
        self.git_request = wanted;

        let Some(repo) = repo else {
            self.git = None;
            self._git_task = None;
            self.git_watcher = None;
            return;
        };

        // Keep the markers we already have while the new read runs: navigating
        // within one repository should not blink them off and on again.
        let previous = match self.git.take() {
            Some(git) if git.repo.root == repo.root => git.status,
            _ => None,
        };
        self.git = Some(Git {
            head: git::head(&repo),
            repo: repo.clone(),
            status: previous,
        });
        self.watch_git(&repo, cx);

        let key = (repo.root.clone(), self.git_generation);
        self._git_task = Some(cx.spawn(async move |this, cx| {
            let status = cx.background_spawn(async move { git::status(&repo) }).await;
            let _ = this.update(cx, |this, cx| {
                // A status for a repository we have since left must not land
                // over the one we are in now.
                if this.git_request.as_ref() == Some(&key)
                    && let Some(git) = this.git.as_mut()
                {
                    git.status = Some(status);
                    cx.notify();
                }
            });
        }));
    }

    /// Watch `.git`, so committing or switching in a terminal is noticed.
    ///
    /// §6.9 calls this out: M3's watcher covers the current directory, which
    /// says nothing about `HEAD` moving underneath us.
    fn watch_git(&mut self, repo: &git::Repo, cx: &mut Context<Self>) {
        if self
            .git_watcher
            .as_ref()
            .is_some_and(|watcher| watcher.path == repo.git_dir)
        {
            return;
        }
        match DirWatcher::new(repo.git_dir.clone()) {
            Ok(watcher) => {
                let events = watcher.events.clone();
                self.git_watcher = Some(watcher);
                self.poll_git_events(events, cx);
            }
            Err(err) => {
                // A repository we cannot watch is still one we can read; the
                // picture just goes stale until the next navigation.
                eprintln!("omafiles: not watching {}: {err}", repo.git_dir.display());
                self.git_watcher = None;
            }
        }
    }

    fn poll_git_events(&mut self, events: EventStream, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let outcome = cx
                    .background_spawn({
                        let events = events.clone();
                        async move { events.wait(Duration::from_millis(400)) }
                    })
                    .await;

                match outcome {
                    Wait::Closed => return,
                    Wait::Idle => {
                        if this.update(cx, |_, _| ()).is_err() {
                            return;
                        }
                    }
                    Wait::Changed => {
                        if this.update(cx, |this, cx| this.invalidate_git(cx)).is_err() {
                            return;
                        }
                    }
                }
            }
        })
        .detach();
    }

    /// Something happened that could change what git would say.
    ///
    /// Bumping the generation is what makes the request key stale, which is what
    /// re-runs the read. The repo cache goes too: a `git init` — or an
    /// `rm -rf .git` — turns a remembered answer into a wrong one.
    fn invalidate_git(&mut self, cx: &mut Context<Self>) {
        self.git_generation = self.git_generation.wrapping_add(1);
        self.repos.clear();
        cx.notify();
    }

    /// The status of one entry, for its marker.
    fn git_state(&self, path: &Path) -> Option<git::State> {
        self.git.as_ref()?.status.as_ref()?.of(path)
    }

    /// Open the branch switcher.
    ///
    /// `git for-each-ref` runs inline. It is one fork on a deliberate keystroke
    /// rather than one per navigation, and a modal that opens empty and fills in
    /// would be worse than a frame's delay.
    fn open_branches(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.git.as_ref() else {
            return;
        };
        let current = git.head.branch().map(str::to_string);
        let all = git::branches(&git.repo);
        let cursor = current
            .as_ref()
            .and_then(|name| all.iter().position(|branch| branch == name))
            .unwrap_or(0);

        self.overlay = Some(Overlay::Branches {
            results: all.clone(),
            all,
            cursor,
            current,
            error: None,
        });
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();
    }

    fn refresh_branches(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().to_lowercase();
        if let Some(Overlay::Branches {
            all,
            results,
            cursor,
            ..
        }) = &mut self.overlay
        {
            *results = all
                .iter()
                .filter(|branch| branch.to_lowercase().contains(&query))
                .cloned()
                .collect();
            *cursor = (*cursor).min(results.len().saturating_sub(1));
            cx.notify();
        }
    }

    /// Switch to the selected branch, and never force.
    ///
    /// On the background executor because a checkout of a large tree takes real
    /// time, and freezing the window mid-checkout would invite the one thing
    /// nobody should do here — killing the process while git is writing.
    fn confirm_branch(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.git.as_ref().map(|git| git.repo.clone()) else {
            return;
        };
        let branch = {
            let Some(Overlay::Branches {
                results,
                cursor,
                current,
                error,
                ..
            }) = &mut self.overlay
            else {
                return;
            };
            let Some(branch) = results.get(*cursor).cloned() else {
                return;
            };
            // Switching to the branch we are already on is a fork that can only
            // report success.
            if current.as_deref() == Some(branch.as_str()) {
                self.overlay = None;
                cx.notify();
                return;
            }
            *error = None;
            branch
        };
        cx.notify();

        self._switch_task = Some(cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move { git::switch(&repo, &branch) })
                .await;

            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(()) => {
                        // Closing rather than dismissing: `render` returns focus
                        // to the pane that had it, and this task has no window.
                        this.overlay = None;
                        this.invalidate_git(cx);
                        // The working tree just changed underneath the listing.
                        this.reload(cx);
                    }
                    // git's own words, verbatim. Paraphrasing a refusal that
                    // exists to protect uncommitted work would be the wrong
                    // place to be creative.
                    Err(message) => {
                        if let Some(Overlay::Branches { error, .. }) = &mut this.overlay {
                            *error = Some(message);
                        }
                    }
                }
                cx.notify();
            });
        }));
    }

    fn dismiss_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay.take().is_some() {
            // Return focus to the pane that had it, or the window is left with
            // nothing focused and every keystroke goes nowhere.
            let owner = self.focus_handle_for(self.pane).clone();
            window.focus(&owner, cx);
            cx.notify();
        }
    }
    /// Widen from filtering this directory to walking the tree below it.
    /// Move within whichever list the open overlay is showing.
    ///
    /// One function rather than one per overlay: they are mutually exclusive by
    /// construction, and two implementations of "clamp a cursor to a list" would
    /// eventually disagree about the empty case.
    fn move_overlay_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        let (cursor, len) = match &mut self.overlay {
            Some(Overlay::Finder {
                recent,
                names,
                hits,
                query,
                cursor,
                ..
            }) => {
                let len = if query.is_empty() {
                    recent.len()
                } else {
                    names.len() + finder_content_rows(names, hits).len()
                };
                (cursor, len)
            }
            Some(Overlay::Branches {
                results, cursor, ..
            }) => (cursor, results.len()),
            Some(Overlay::Palette { results, cursor }) => (cursor, results.len()),
            Some(Overlay::CopyImage {
                variants, cursor, ..
            }) => (cursor, variants.len() + 1),
            // The path panel's cursor can leave the list entirely — `None`
            // means Enter takes the typed text — so it clamps differently.
            Some(Overlay::Path {
                suggestions,
                create,
                cursor,
                ..
            }) => {
                let len = suggestions.len() + usize::from(create.is_some());
                if len == 0 {
                    return;
                }
                let current = cursor.map(|c| c as isize).unwrap_or(-1);
                let next = (current + delta).clamp(-1, len as isize - 1);
                *cursor = usize::try_from(next).ok();
                cx.notify();
                return;
            }
            _ => return,
        };
        if len == 0 {
            return;
        }
        *cursor = (*cursor as isize + delta).clamp(0, len as isize - 1) as usize;
        cx.notify();
    }

    /// Re-filter whichever overlay owns the input field.
    fn filter_overlay(&mut self, cx: &mut Context<Self>) {
        match self.overlay {
            Some(Overlay::Finder { .. }) => self.refresh_finder(cx),
            Some(Overlay::Branches { .. }) => self.refresh_branches(cx),
            Some(Overlay::Palette { .. }) => self.refresh_palette(cx),
            Some(Overlay::Path { .. }) => self.refresh_path_suggestions(cx),
            // The help filter derives from the input at render time.
            Some(Overlay::Help) => cx.notify(),
            _ => {}
        }
    }

    /// Enter: take the selected result, or create/rename the workspace.
    fn confirm_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The palette: close first, then dispatch — a command that opens its
        // own modal (search, help, the server menu) must not find the palette
        // still standing.
        if let Some(Overlay::Palette { results, cursor }) = &self.overlay {
            let command = results.get(*cursor).map(|&i| &COMMANDS[i]);
            let build = command.map(|c| c.build);
            self.dismiss_overlay(window, cx);
            if let Some(build) = build {
                window.dispatch_action(build(), cx);
            }
            return;
        }
        // Handled before the `take`: a refused switch has to put its message
        // back into the overlay that is still open, not into one we own.
        if matches!(self.overlay, Some(Overlay::Branches { .. })) {
            self.confirm_branch(cx);
            return;
        }
        if matches!(self.overlay, Some(Overlay::Agent { .. })) {
            self.confirm_agent(cx);
            return;
        }
        if matches!(self.overlay, Some(Overlay::Path { .. })) {
            self.confirm_path(window, cx);
            return;
        }
        if matches!(self.overlay, Some(Overlay::AddNetwork)) {
            self.confirm_add_network(window, cx);
            return;
        }
        if matches!(self.overlay, Some(Overlay::CopyImage { .. })) {
            self.confirm_copy_image(window, cx);
            return;
        }
        if matches!(self.overlay, Some(Overlay::Delete { .. })) {
            self.confirm_delete(window, cx);
            return;
        }
        if matches!(self.overlay, Some(Overlay::Refused { .. })) {
            self.dismiss_overlay(window, cx);
            return;
        }
        // Enter in the server menu starts the loopback server — the common
        // case, keyboard-reachable. Stopping stays a deliberate click, and a
        // running server's Enter does nothing rather than toggling.
        if matches!(self.overlay, Some(Overlay::Server { .. })) {
            if self.server_index_for(&self.current_path()).is_none() {
                self.start_server(false, cx);
            }
            return;
        }
        match self.overlay.take() {
            // A finder row navigates to its directory with the cursor on the
            // file — the preview then shows the file itself. One resolution
            // for all three sections.
            Some(Overlay::Finder {
                recent,
                names,
                hits,
                query,
                cursor,
                ..
            }) => {
                let path = if query.is_empty() {
                    recent.get(cursor).map(|f| f.path.clone())
                } else if cursor < names.len() {
                    names.get(cursor).map(|m| m.path.clone())
                } else {
                    finder_content_rows(&names, &hits)
                        .get(cursor - names.len())
                        .map(|h| h.path.clone())
                };
                let Some(hit_path) = path else {
                    return;
                };
                let leaving = self.cursor_name();
                // A directory result opens *it*; a file's directory opens
                // with the cursor on the file.
                let target = if hit_path.is_dir() {
                    hit_path.clone()
                } else {
                    hit_path.parent().map(Path::to_path_buf).unwrap_or_default()
                };
                if let Some(tab) = self.session.active_tab_mut() {
                    tab.navigation.go(target, leaving.as_deref());
                    // Both memories, as `create_file` does: `reload` consults
                    // the navigation's first, and it remembers this directory
                    // whenever it was left by going up — so a hit in the
                    // directory just returned to would otherwise lose to the
                    // child the cursor came back to.
                    if let Some(name) = hit_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                    {
                        tab.navigation.land_on(&name);
                        tab.cursor_name = Some(name);
                    }
                }
                self.reload(cx);
            }
            Some(Overlay::Workspace { editing }) => {
                let name = self.input.read(cx).value().trim().to_string();
                if name.is_empty() {
                    // An unnamed workspace is indistinguishable from global.
                    cx.notify();
                    return;
                }
                match editing {
                    Some(index) => {
                        self.session.rename_workspace(index, name);
                    }
                    None => {
                        let index = self.session.add_workspace(name);
                        // Move the current tab in, so it is not born empty and
                        // therefore unusable.
                        let from = self.session.active_workspace();
                        let tab = self
                            .session
                            .workspace(from)
                            .map(|w| w.active_tab)
                            .unwrap_or(0);
                        self.session.move_tab(from, tab, index);
                    }
                }
                self.persist_session();
            }
            // Help and the workspace menu are dismissed, not confirmed.
            Some(other) => {
                self.overlay = Some(other);
                return;
            }
            None => return,
        }
        cx.notify();
    }

    // ------------------------------------------------------------ path entry

    /// `^l`, or a click on the breadcrumb: edit the path.
    ///
    /// Rebuilt from the inline header field into a modal panel: the input is
    /// prefilled with the current path, and because an exact directory
    /// completes to its *descendants*, the panel opens already offering the
    /// places one level down — the common move — before a key is typed.
    fn edit_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.current_path().to_string_lossy().into_owned();
        self.open_path_picker(PathPurpose::GoTo, current, window, cx);
    }

    /// `m`, the detail panel's Move, or the entry menu: pick where the entry
    /// under the cursor goes. Prefilled with the directory it is in, so the
    /// common move — one level down, or a sibling — is a few keys away.
    fn move_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.cursor().and_then(|i| self.listing()?.get(i)).cloned() else {
            return;
        };
        let here = self.current_path().to_string_lossy().into_owned();
        let purpose = PathPurpose::MoveInto {
            source: entry.path,
            name: entry.name,
        };
        self.open_path_picker(purpose, here, window, cx);
    }

    /// `n`, or the status bar's New: create an empty file, or a directory
    /// if the typed path ends in `/`. The field opens on the current
    /// directory with the slash already typed, so what is left to type is
    /// the name — but the whole path is editable.
    fn create_file_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut here = self.current_path().to_string_lossy().into_owned();
        if !here.ends_with('/') {
            here.push('/');
        }
        self.open_path_picker(PathPurpose::CreateFile, here, window, cx);
    }

    /// The palette's "Toggle button labels": flip the setting and keep it.
    fn toggle_button_labels(&mut self, cx: &mut Context<Self>) {
        self.config.button_labels = !self.config.button_labels;
        match self.config.save(&self.config_path) {
            Ok(()) => {
                let said = if self.config.button_labels {
                    "button labels shown"
                } else {
                    "button labels hidden \u{2014} hover a button for its verb"
                };
                self.inform_user(said.to_string(), cx);
            }
            Err(err) => self.notify_user(format!("could not save the setting: {err}"), cx),
        }
        cx.notify();
    }

    /// The shared location picker, opened for one of its purposes.
    fn open_path_picker(
        &mut self,
        purpose: PathPurpose,
        prefill: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlay = Some(Overlay::Path {
            purpose,
            suggestions: Vec::new(),
            create: None,
            cursor: None,
        });
        self.input.update(cx, |input, cx| {
            input.set_value(prefill, window, cx);
        });
        window.focus(&self.input.focus_handle(cx), cx);
        self.refresh_path_suggestions(cx);
    }

    /// A directory row in the create-file picker: put it in the field, with
    /// the slash, and keep going — completion, not confirmation.
    fn complete_path_field(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let mut text = dir.to_string_lossy().into_owned();
        if !text.ends_with('/') {
            text.push('/');
        }
        self.input.update(cx, |input, cx| {
            input.set_value(text, window, cx);
        });
        self.refresh_path_suggestions(cx);
    }

    /// Directories completing what has been typed so far.
    ///
    /// Two cases, and the first is what makes the panel feel like completion
    /// rather than validation: text naming an **existing directory** offers
    /// that directory's children (its descendants); anything else completes
    /// against the typed path's parent, filtered by the last component — so
    /// `/usr/sh` suggests `/usr/share`, the way a shell does.
    fn refresh_path_suggestions(&mut self, cx: &mut Context<Self>) {
        let typed = self.input.read(cx).value().to_string();
        let expanded = expand_tilde(&typed);

        let as_path = PathBuf::from(&expanded);
        let (dir, prefix) = if expanded.ends_with('/') || as_path.is_dir() {
            (as_path, String::new())
        } else {
            let prefix = as_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (
                as_path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from("/")),
                prefix,
            )
        };

        let lower = prefix.to_lowercase();
        let mut found: Vec<PathBuf> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .filter(|p| {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                // Only offer dotfiles once the user has typed a dot, matching
                // the listing's hidden-file rule.
                (prefix.starts_with('.') || !name.starts_with('.'))
                    && name.to_lowercase().starts_with(&lower)
            })
            .collect();
        found.sort_by(|a, b| {
            let of = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned());
            natural_cmp(&of(a).unwrap_or_default(), &of(b).unwrap_or_default())
        });
        found.truncate(10);

        // The create row: what was typed, when nothing is there yet. Not for
        // the file picker, where the typed path is the file itself.
        let trimmed = expanded.trim_end_matches('/');
        let offer_create = !matches!(
            self.overlay,
            Some(Overlay::Path {
                purpose: PathPurpose::CreateFile,
                ..
            })
        );
        let creatable = (offer_create && !trimmed.is_empty() && !Path::new(trimmed).exists())
            .then(|| PathBuf::from(trimmed));

        if let Some(Overlay::Path {
            suggestions,
            create,
            cursor,
            ..
        }) = &mut self.overlay
        {
            *suggestions = found;
            *create = creatable;
            // Typing returns Enter to the typed text; arrows re-enter the list.
            *cursor = None;
        }
        cx.notify();
    }

    /// Enter in the location picker: the highlighted row, or the typed text —
    /// and then whatever the panel was opened for.
    fn confirm_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Path {
            purpose,
            suggestions,
            create,
            cursor,
        }) = &self.overlay
        else {
            return;
        };
        let purpose = purpose.clone();
        let typed = self.input.read(cx).value().to_string();
        let typed_path = PathBuf::from(expand_tilde(&typed));

        // The maker: the typed text names the file, or the directory when
        // it ends in `/`. A highlighted directory completes the field
        // instead of confirming.
        if matches!(purpose, PathPurpose::CreateFile) {
            if let Some(dir) = cursor.and_then(|i| suggestions.get(i).cloned()) {
                self.complete_path_field(dir, window, cx);
                return;
            }
            let trimmed = typed.trim();
            let directory = trimmed.ends_with('/');
            // `a/b/` names `b`, not an empty last component.
            let target = PathBuf::from(expand_tilde(trimmed.trim_end_matches('/')));
            if target.file_name().is_none() || (!directory && target.is_dir()) {
                let what = if directory { "directory" } else { "file" };
                self.notify_user(format!("type a name for the {what}"), cx);
                return;
            }
            self.dismiss_overlay(window, cx);
            if directory {
                self.create_directory_at(target, cx);
            } else {
                self.create_file_at(target, cx);
            }
            return;
        }

        enum Picked {
            Existing(PathBuf),
            Create(PathBuf),
        }
        let picked = match *cursor {
            Some(i) if i < suggestions.len() => Picked::Existing(suggestions[i].clone()),
            Some(_) => match create {
                Some(path) => Picked::Create(path.clone()),
                None => return,
            },
            None => {
                if typed_path.is_dir() {
                    Picked::Existing(typed_path)
                } else if let Some(first) = suggestions.first() {
                    // An unambiguous half-typed name commits to its completion.
                    Picked::Existing(first.clone())
                } else if matches!(purpose, PathPurpose::GoTo) {
                    // Rather than refusing outright, land on the nearest real
                    // ancestor: a typo deep in a path should still get close.
                    Picked::Existing(nearest_existing(&typed_path))
                } else {
                    // A move has to land exactly where it was aimed. The
                    // create row is one arrow away when that is the intent.
                    self.notify_user(
                        "no such directory \u{2014} pick \u{201c}create\u{201d} to make it"
                            .to_string(),
                        cx,
                    );
                    return;
                }
            }
        };

        self.dismiss_overlay(window, cx);
        let target = match picked {
            Picked::Existing(path) => path,
            Picked::Create(path) => match std::fs::create_dir_all(&path) {
                Ok(()) => path,
                Err(err) => {
                    self.notify_user(format!("could not create {}: {err}", path.display()), cx);
                    return;
                }
            },
        };

        match purpose {
            PathPurpose::GoTo => {
                let leaving = self.cursor_name();
                if let Some(tab) = self.session.active_tab_mut() {
                    tab.navigation.go(target, leaving.as_deref());
                }
                self.reload(cx);
            }
            PathPurpose::MoveInto { source, name } => self.move_into(source, name, target, cx),
            PathPurpose::CreateFile => unreachable!("handled above"),
        }
    }

    /// Move `source` into `dest`, off the main thread — a directory tree can
    /// be arbitrarily large, and across filesystems a move is a copy.
    fn move_into(&mut self, source: PathBuf, name: String, dest: PathBuf, cx: &mut Context<Self>) {
        self._action_task = Some(cx.spawn(async move |this, cx| {
            let dest_for_op = dest.clone();
            let outcome = cx
                .background_spawn(async move { fileops::move_into(&source, &dest_for_op) })
                .await;
            let _ = this.update(cx, |this, cx| match outcome {
                Ok(_) => {
                    let where_to = dest
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| dest.to_string_lossy().into_owned());
                    // The watcher reloads the listing the entry just left.
                    this.inform_user(
                        format!("moved \u{201c}{name}\u{201d} into \u{201c}{where_to}\u{201d}"),
                        cx,
                    );
                }
                Err(message) => this.notify_user(message, cx),
            });
        }));
    }

    /// A drop landed on `dest` — a directory row, a tab, a place.
    ///
    /// Every item is checked first, and one that cannot go refuses the whole
    /// drop with a modal saying why: half a selection moving is harder to
    /// undo than none of it. What passes moves in the background.
    fn drop_entries(&mut self, dragged: &DraggedEntries, dest: PathBuf, cx: &mut Context<Self>) {
        let items = dragged.items.clone();
        let where_to = display_path(&dest);
        let reasons = drop_refusals(&items, &dest);
        if !reasons.is_empty() {
            self.overlay = Some(Overlay::Refused {
                title: "Cannot move here",
                subtitle: format!("nothing was moved into {where_to}"),
                reasons,
            });
            cx.notify();
            return;
        }
        // The marks travel with the files; what is left behind is unmarked.
        self.clear_selection();
        self.move_many(items, dest, cx);
    }

    /// Move `items` into `dest`, off the main thread, and say how it went:
    /// a notice when everything moved, the refusal modal with one line per
    /// failure when anything did not.
    fn move_many(&mut self, items: Arc<Vec<DragItem>>, dest: PathBuf, cx: &mut Context<Self>) {
        self._action_task = Some(cx.spawn(async move |this, cx| {
            let (dest_for_op, items_for_op) = (dest.clone(), items.clone());
            let failures: Vec<String> = cx
                .background_spawn(async move {
                    items_for_op
                        .iter()
                        .filter_map(|item| {
                            fileops::move_into(&item.path, &dest_for_op)
                                .err()
                                .map(|err| format!("\u{201c}{}\u{201d}: {err}", item.name))
                        })
                        .collect()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let where_to = display_path(&dest);
                // The active tab's watcher reloads the directory the files
                // left. Any other tab may be looking at where they landed,
                // and its cached listing is now wrong — it re-reads when
                // next shown.
                this.forget_other_listings();
                if failures.is_empty() {
                    let what = DraggedEntries { items }.label();
                    this.inform_user(format!("moved {what} into {where_to}"), cx);
                } else {
                    let moved = items.len() - failures.len();
                    this.overlay = Some(Overlay::Refused {
                        title: "Could not move",
                        subtitle: if moved > 0 {
                            format!("{moved} of {} moved into {where_to}", items.len())
                        } else {
                            format!("nothing was moved into {where_to}")
                        },
                        reasons: failures,
                    });
                    cx.notify();
                }
            });
        }));
    }

    /// Drop every cached listing but the active tab's, so tabs whose
    /// directory changed under them re-read it when next shown.
    fn forget_other_listings(&mut self) {
        let keep = self.tab_id();
        self.listings.retain(|id, _| *id == keep);
        self.cursors.retain(|id, _| *id == keep);
    }

    /// Create the file, then take the tab to its directory with the cursor on
    /// it — a new file you cannot see is a new file you will look for.
    fn create_file_at(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.land_created(path, fileops::create_file, cx);
    }

    /// The directory twin of [`Self::create_file_at`]: same landing, with
    /// the cursor on the new directory rather than inside it.
    fn create_directory_at(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.land_created(path, fileops::create_directory, cx);
    }

    /// Run `make` in the background, then go to the parent of what it made
    /// with the cursor on it.
    fn land_created(
        &mut self,
        path: PathBuf,
        make: fn(&Path) -> Result<PathBuf, String>,
        cx: &mut Context<Self>,
    ) {
        self._action_task = Some(cx.spawn(async move |this, cx| {
            let outcome = cx.background_spawn(async move { make(&path) }).await;
            let _ = this.update(cx, |this, cx| match outcome {
                Ok(created) => {
                    let name = created
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let parent = created
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| PathBuf::from("/"));
                    let leaving = this.cursor_name();
                    if let Some(tab) = this.session.active_tab_mut() {
                        tab.navigation.go(parent, leaving.as_deref());
                        // Both memories, because `reload` consults the
                        // navigation's first and it may remember this
                        // directory from an earlier visit.
                        tab.navigation.land_on(&name);
                        tab.cursor_name = Some(name.clone());
                    }
                    this.reload(cx);
                    this.inform_user(format!("created \u{201c}{name}\u{201d}"), cx);
                }
                Err(message) => this.notify_user(message, cx),
            });
        }));
    }
    // --------------------------------------------------------------- watching

    /// Watch the current directory.
    ///
    /// The directory itself, not its parent: unlike the theme directory in
    /// `omarchy-tokens`, a browsed directory is not atomically replaced. If it
    /// is removed we get an event and the reload lands on [`nearest_existing`].
    fn watch_current(&mut self, cx: &mut Context<Self>) {
        let path = self.current_path().as_path().to_path_buf();
        if self.watcher.as_ref().is_some_and(|w| w.path == path) {
            return;
        }

        match DirWatcher::new(path.clone()) {
            Ok(watcher) => {
                let events = watcher.events.clone();
                self.watcher = Some(watcher);
                self.poll_events(events, cx);
            }
            Err(err) => {
                // A directory we cannot watch is still one we can browse.
                eprintln!("omafiles: not watching {}: {err}", path.display());
                self.watcher = None;
            }
        }
    }

    fn poll_events(&mut self, events: EventStream, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let outcome = cx
                    .background_spawn({
                        let events = events.clone();
                        async move { events.wait(Duration::from_millis(400)) }
                    })
                    .await;

                match outcome {
                    Wait::Closed => return,
                    Wait::Idle => {
                        // Also stop once the view is gone, or this task
                        // outlives the window it belongs to.
                        if this.update(cx, |_, _| ()).is_err() {
                            return;
                        }
                    }
                    Wait::Changed => {
                        let updated = this.update(cx, |this, cx| {
                            // Keep the cursor on the same *name*: indices shift
                            // when a file appears or disappears above it.
                            let keep = this.cursor_name();
                            let id = this.tab_id();
                            let path = this.current_path();
                            let listing = Listing::read_sorted(&path, this.views.get(&path).sort());
                            this.listings.insert(id.clone(), listing);
                            this.prune_selection_of(&id);
                            let restored = this.restore_cursor(keep.as_deref()).or_else(|| {
                                this.listing()
                                    .and_then(|l| first_visible(l, this.show_hidden))
                            });
                            this.set_cursor(restored);
                            // A file changing in this directory is also a
                            // change git would report differently.
                            this.invalidate_git(cx);
                            cx.notify();
                        });
                        if updated.is_err() {
                            return;
                        }
                    }
                }
            }
        })
        .detach();
    }
}

fn first_visible(listing: &Listing, show_hidden: bool) -> Option<usize> {
    listing.visible(show_hidden).first().copied()
}

impl Focusable for Explorer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Explorer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A tab drag that ended anywhere but on a tab row leaves its
        // insertion line behind; the frame after the drag removes it.
        if self.tab_drop.is_some() && !cx.has_active_drag() {
            self.tab_drop = None;
        }
        self.viewport_width = f32::from(window.viewport_size().width);
        // One call site, at the top of the frame, rather than in each of the
        // dozen places the cursor can move. It is a key comparison when nothing
        // changed, and it cannot be forgotten the way a dozen calls can.
        //
        // Git first: the preview's key includes the cursor entry's git state,
        // so running it second is what lets a changed file turn into its diff on
        // the frame after the status lands.
        self.refresh_git(cx);
        self.refresh_preview(cx);
        // The registry is the truth about servers — ours and any other
        // window's — and it is cheap enough to read whenever we repaint.
        self.servers = server::list();

        // Take the window focus on first render, so the app is keyboard-ready
        // without a click.
        // While an overlay is open the input owns focus; stealing it back here
        // would make the field impossible to type into.
        if self.overlay.is_none() {
            let owner = self.focus_handle_for(self.pane).clone();
            if !owner.is_focused(window) {
                window.focus(&owner, cx);
            }
        }

        let (background, foreground, family, body) = {
            let theme = cx.theme();
            (
                theme.background(),
                theme.foreground(),
                theme.type_scale().family.clone(),
                theme.type_scale().body(),
            )
        };

        // Outermost is a plain positioned container, not the flex column: an
        // absolutely-positioned overlay that is also a flex *item* still gets
        // sized by flex, which left the scrim covering only a band.
        div()
            .relative()
            .size_full()
            // Typography lives on the outermost container so the overlay, which
            // is a sibling of the content column rather than inside it,
            // inherits the same family and size. Without this the modal renders
            // in gpui's default proportional font next to a monospace app.
            .font_family(family.clone())
            .text_size(px(body))
            .text_color(foreground)
            // ⚠ The handlers live on the **outermost** container, not on the
            // content column, and that is load-bearing. The overlay is a
            // *sibling* of the column (M2's layout note), and gpui dispatches an
            // action up the focus path — so every handler on the column is
            // unreachable the moment a modal's text field takes focus. That is
            // why `esc`, `↓`/`↑` and `^g` all did nothing inside a modal until
            // M8: the bindings resolved and the action went nowhere. The
            // bindings stay context-scoped; only the handlers moved up.
            //
            // These verbs are bound in more than one context, so they dispatch
            // on the focused pane rather than duplicating the handlers.
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_in_pane(1, cx)))
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_in_pane(-1, cx)))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| {
                if !this.overlay_owns_input() {
                    this.move_cursor(PAGE, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &PageUp, _, cx| {
                if !this.overlay_owns_input() {
                    this.move_cursor(-PAGE, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &MoveFirst, _, cx| this.edge_in_pane(false, cx)))
            .on_action(cx.listener(|this, _: &MoveLast, _, cx| this.edge_in_pane(true, cx)))
            .on_action(cx.listener(|this, _: &Open, window, cx| {
                // A modal without a text field leaves focus on the pane, so
                // Enter arrives here — it belongs to the modal, not to the
                // listing underneath it. Without this, Enter over the help
                // sheet navigated under the modal.
                if this.overlay.is_some() {
                    this.confirm_overlay(window, cx);
                    return;
                }
                match this.pane {
                    Pane::Sidebar => this.open_place(cx),
                    Pane::Listing => this.open_selected(cx),
                }
            }))
            .on_action(cx.listener(|this, _: &GoUp, _, cx| {
                if this.overlay_owns_input() {
                    return;
                }
                let leaving = this.cursor_name();
                let moved = this
                    .session
                    .active_tab_mut()
                    .is_some_and(|t| t.navigation.go_up(leaving.as_deref()));
                if moved {
                    this.reload(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &GoBack, _, cx| {
                if this.overlay_owns_input() {
                    return;
                }
                let leaving = this.cursor_name();
                let moved = this
                    .session
                    .active_tab_mut()
                    .is_some_and(|t| t.navigation.go_back(leaving.as_deref()));
                if moved {
                    this.reload(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &GoForward, _, cx| {
                if this.overlay_owns_input() {
                    return;
                }
                let leaving = this.cursor_name();
                let moved = this
                    .session
                    .active_tab_mut()
                    .is_some_and(|t| t.navigation.go_forward(leaving.as_deref()));
                if moved {
                    this.reload(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleHidden, _, cx| {
                if !this.overlay_owns_input() {
                    this.toggle_hidden(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| {
                if !this.overlay_owns_input() {
                    this.reload(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &FocusNext, window, cx| {
                let next = match this.pane {
                    Pane::Sidebar => Pane::Listing,
                    Pane::Listing => Pane::Sidebar,
                };
                this.focus_pane(next, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusPrevious, window, cx| {
                let previous = match this.pane {
                    Pane::Sidebar => Pane::Listing,
                    Pane::Listing => Pane::Sidebar,
                };
                this.focus_pane(previous, window, cx);
            }))
            .on_action(cx.listener(|this, _: &PinCurrent, _, cx| this.pin_current(cx)))
            .on_action(cx.listener(|this, _: &UnpinSelected, _, cx| this.unpin_selected(cx)))
            .on_action(cx.listener(|this, _: &MovePinUp, _, cx| this.move_pin(-1, cx)))
            .on_action(cx.listener(|this, _: &MovePinDown, _, cx| this.move_pin(1, cx)))
            .on_action(cx.listener(|this, _: &NewTab, _, cx| this.new_tab(cx)))
            .on_action(cx.listener(|this, _: &CloseTab, _, cx| this.close_tab(cx)))
            .on_action(cx.listener(|this, _: &NextTab, _, cx| this.cycle_tab(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousTab, _, cx| this.cycle_tab(-1, cx)))
            .on_action(cx.listener(|this, _: &NewWorkspace, window, cx| {
                this.open_workspace_prompt(None, window, cx)
            }))
            .on_action(cx.listener(|this, _: &RenameWorkspace, window, cx| {
                let active = this.session.active_workspace();
                if this
                    .session
                    .workspace(active)
                    .is_some_and(|w| !w.is_global())
                {
                    this.open_workspace_prompt(Some(active), window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &StartSearch, window, cx| this.open_finder(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &AddNetwork, window, cx| this.open_add_network(window, cx)),
            )
            .on_action(cx.listener(|this, _: &Dismiss, window, cx| {
                // Ordered: an open overlay is on top of the expanded
                // preview, so it must be the one Escape closes first.
                if this.overlay.is_none() && !this.collapse_preview(cx) {
                    // Nothing else to close: Escape clears the marks.
                    this.clear_selection();
                    cx.notify();
                }
                this.dismiss_overlay(window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &Confirm, window, cx| this.confirm_overlay(window, cx)),
            )
            .on_action(cx.listener(|this, _: &TogglePreview, _, cx| this.toggle_preview(cx)))
            .on_action(
                cx.listener(|this, _: &SwitchBranch, window, cx| this.open_branches(window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenTerminal, _, cx| {
                if !this.overlay_owns_input() {
                    this.open_terminal_here(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &AskAgent, window, cx| {
                // Guarded like the other world verbs: a leaked keystroke under
                // a modal must not summon an external picker.
                if !this.overlay_owns_input() {
                    this.ask_agent(window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &ShareEntry, _, cx| {
                if !this.overlay_owns_input() {
                    this.share_selected(cx)
                }
            }))
            .on_action(
                cx.listener(|this, _: &ServerMenu, window, cx| this.open_server_menu(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &CommandPalette, window, cx| this.open_palette(window, cx)),
            )
            .on_action(cx.listener(|this, _: &CopyEntry, window, cx| {
                if !this.overlay_owns_input() {
                    this.copy_selected(window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &CutEntry, _, cx| {
                if !this.overlay_owns_input() {
                    this.cut_selected(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &CopyPath, _, cx| {
                if !this.overlay_owns_input() {
                    this.copy_path_selected(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &ExtendDown, _, cx| this.extend_selection(1, cx)))
            .on_action(cx.listener(|this, _: &ExtendUp, _, cx| this.extend_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.select_all(cx)))
            .on_action(cx.listener(|this, _: &ToggleSelect, _, cx| this.toggle_select(cx)))
            .on_action(cx.listener(|this, _: &DeleteEntry, window, cx| {
                if !this.overlay_owns_input() {
                    this.delete_selected(window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &PasteHere, _, cx| {
                if !this.overlay_owns_input() {
                    this.paste_here(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &CompressEntry, _, cx| {
                if !this.overlay_owns_input() {
                    this.compress_selected(cx)
                }
            }))
            .on_action(
                cx.listener(|this, _: &EntryMenu, window, cx| {
                    this.open_entry_menu(None, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &MoveEntry, window, cx| {
                if !this.overlay_owns_input() {
                    this.move_selected(window, cx)
                }
            }))
            .on_action(
                cx.listener(|this, _: &ToggleButtonLabels, _, cx| this.toggle_button_labels(cx)),
            )
            .on_action(cx.listener(|this, _: &CreateFile, window, cx| {
                if !this.overlay_owns_input() {
                    this.create_file_here(window, cx)
                }
            }))
            .on_action(
                cx.listener(|this, _: &ServerList, window, cx| this.open_server_list(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowHelp, window, cx| this.show_help(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleLeftPanel, _, cx| {
                this.left_open = !this.left_open;
                cx.notify();
            }))
            // A panel or column resize is tracked here, on the container
            // that spans the window, so the drag keeps following the pointer
            // once it has left the hairline it started on.
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if this.resizing.is_none() && this.column_resizing.is_none() {
                    return;
                }
                // The button came up somewhere the window did not see it.
                if event.pressed_button != Some(gpui::MouseButton::Left) {
                    this.end_panel_resize(cx);
                    this.end_column_resize(cx);
                    return;
                }
                let x = f32::from(event.position.x);
                this.drag_panel_edge(x, cx);
                this.drag_column_divider(x, cx);
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _e, _, cx| {
                    this.end_panel_resize(cx);
                    this.end_column_resize(cx);
                }),
            )
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(|this, _e, _, cx| {
                    this.end_panel_resize(cx);
                    this.end_column_resize(cx);
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleRightPanel, _, cx| {
                this.right_open = !this.right_open;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &EditPath, window, cx| this.edit_path(window, cx)))
            .on_action(cx.listener(|this, _: &GoParent, _, cx| {
                if this.overlay_owns_input() {
                    return;
                }
                let leaving = this.cursor_name();
                let moved = this
                    .session
                    .active_tab_mut()
                    .is_some_and(|t| t.navigation.go_up(leaving.as_deref()));
                if moved {
                    this.reload(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &DeleteWorkspace, _, cx| this.delete_workspace(cx)))
            .on_action(cx.listener(|this, _: &MoveTabToNextWorkspace, _, cx| {
                this.move_tab_to_next_workspace(cx)
            }))
            .child(
                div()
                    .key_context("Listing")
                    .track_focus(&self.focus)
                    .flex()
                    .flex_col()
                    .size_full()
                    .bg(background.opacity(0.94))
                    .text_color(foreground)
                    .font_family(family)
                    .text_size(px(body))
                    // No padding or gap on the shell: the rules must reach the
                    // window edges and sit flush against the bars they divide.
                    // Every region below supplies its own inner spacing.
                    .child(self.panes(window, cx))
                    .child(Separator::horizontal())
                    .child(self.status_bar(cx)),
            )
            .children(self.floating_panels(window, cx))
            .children(self.overlay_layer(window, cx))
    }
}

impl Explorer {
    /// The height both top bars share, so the rule under each meets the
    /// other's across the vertical divider.
    fn bar_height(space: &omarchy_tokens::Spacing) -> f32 {
        space.control_height() + space.sm() * 2.0
    }

    /// The navigation bar over the listing and the detail panel: back, up,
    /// and the path — click to edit. When the sidebar is hidden the tools
    /// that live in its own bar ride here instead, so the mouse can still
    /// reach them.
    fn nav_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let space = theme.space();
        // The status bar's rhythm (on request): one small value on every
        // side and between items.
        let inset = space.sm();
        let height = Self::bar_height(space);
        let can_back = self
            .session
            .active_tab()
            .is_some_and(|t| t.navigation.can_go_back());

        // Always the breadcrumb: editing happens in the modal panel
        // (`edit_path`), not inline — the header never reflows.
        let path_area = div()
            .id("path")
            .flex_1()
            .min_w(px(0.))
            .cursor_pointer()
            .child(Breadcrumb::new(
                self.session
                    .active_tab()
                    .map(|t| t.navigation.breadcrumb())
                    .unwrap_or_default(),
            ))
            .on_click(cx.listener(|this, _e, window, cx| this.edit_path(window, cx)))
            .into_any_element();

        let tools = if self.left_open {
            Vec::new()
        } else {
            self.tool_buttons(cx)
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .flex_shrink_0()
            .h(px(height))
            .gap(px(inset))
            .p(px(inset))
            .child(
                ActionButton::new("nav-back")
                    .glyph("\u{f060}")
                    .enabled(can_back)
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        let leaving = this.cursor_name();
                        let moved = this
                            .session
                            .active_tab_mut()
                            .is_some_and(|t| t.navigation.go_back(leaving.as_deref()));
                        if moved {
                            this.reload(cx);
                        }
                    })),
            )
            .child(
                ActionButton::new("nav-up")
                    .glyph("\u{f062}")
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        let leaving = this.cursor_name();
                        let moved = this
                            .session
                            .active_tab_mut()
                            .is_some_and(|t| t.navigation.go_up(leaving.as_deref()));
                        if moved {
                            this.reload(cx);
                        }
                    })),
            )
            .child(path_area)
            .children(tools)
            .children(
                self.listing()
                    .and_then(|l| l.error.clone())
                    .map(|e| Badge::new(e).urgent()),
            )
            .into_any_element()
    }

    /// The bar over the sidebar: the tools that are about the whole app
    /// rather than the directory being looked at — find, and the servers.
    fn sidebar_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let space = theme.space();
        let inset = space.sm();
        let height = Self::bar_height(space);
        let dim = theme.dim_foreground();
        let tools = self.tool_buttons(cx);
        // Borderless, at the far right: a control about the panel itself,
        // not one of its tools, so it must not read as a third tool.
        let collapse = quiet_button(
            "sidebar-collapse",
            "\u{f100}", // nf-fa-angle_double_left
            dim,
            cx.listener(|this, _e, _w, cx| {
                this.left_open = false;
                cx.notify();
            }),
            cx,
        );
        div()
            .flex()
            .flex_row()
            .items_center()
            .flex_shrink_0()
            .h(px(height))
            .gap(px(inset))
            .p(px(inset))
            .children(tools)
            .child(div().flex_1())
            .child(collapse)
            .into_any_element()
    }

    /// What stands in for the sidebar while it is collapsed: a strip one
    /// button wide, holding only the way to expand it again (on request).
    fn sidebar_strip(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let space = theme.space();
        let inset = space.sm();
        let height = Self::bar_height(space);
        let width = theme.type_scale().caption() * ICON_COLUMN + inset * 2.0;
        let dim = theme.dim_foreground();
        let expand = quiet_button(
            "sidebar-expand",
            "\u{f101}", // nf-fa-angle_double_right
            dim,
            cx.listener(|this, _e, _w, cx| {
                this.left_open = true;
                cx.notify();
            }),
            cx,
        );
        div()
            .flex()
            .flex_col()
            .w(px(width))
            .flex_shrink_0()
            .h_full()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .h(px(height))
                    .p(px(inset))
                    .child(expand),
            )
            .child(Separator::horizontal())
            .into_any_element()
    }

    /// Find and the server globe, wherever the bar that shows them is.
    fn tool_buttons(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let finder = ActionButton::new("finder")
            .glyph("\u{f002}") // nf-fa-search
            .on_click(cx.listener(|this, _e, window, cx| this.open_finder(window, cx)));
        // The globe: every running server, wherever its root. Counted and
        // accented while anything serves, so the bar says at a glance that
        // ports are open.
        let count = self.servers.len();
        let globe = ActionButton::new("servers")
            .glyph("\u{f0ac}") // nf-fa-globe
            .accent(count > 0)
            .on_click(cx.listener(|this, _e, window, cx| this.open_server_list(window, cx)));
        let globe = if count > 0 {
            globe.label(count.to_string())
        } else {
            globe
        };
        vec![finder.into_any_element(), globe.into_any_element()]
    }

    /// The sidebar with its bar on top, the shape it takes docked or floating.
    fn sidebar_column(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let width = self.panel_width(PanelSide::Left, cx);
        div()
            .flex()
            .flex_col()
            .w(px(width))
            .flex_shrink_0()
            .h_full()
            .child(self.sidebar_bar(cx))
            .child(Separator::horizontal())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.))
                    .child(self.sidebar_pane(cx)),
            )
            .into_any_element()
    }

    /// The two panes: the listing, and a detail pane M7 turns into the preview.
    fn panes(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let narrow = self.is_narrow(window, cx);

        // In a narrow window the panels stop taking space and become overlays,
        // so the listing always keeps a usable width. `left_open` is not
        // cleared — it is the user's intent, and widening the window should
        // bring the panel back rather than leave it shut.
        // The sidebar, or the strip that expands it again; a narrow window
        // floats the panel over the listing, so the strip stays as the way in.
        let mut row = div().flex().flex_row().flex_1().min_h(px(0.));
        // The rule beside a docked panel is also its grip; the strip's rule
        // is only a rule, since a collapsed panel has no width to drag.
        let (left, rule) = if !narrow && self.left_open {
            (
                self.sidebar_column(cx),
                self.resize_handle(PanelSide::Left, cx),
            )
        } else {
            (
                self.sidebar_strip(cx),
                Separator::vertical().into_any_element(),
            )
        };
        row = row.child(left).child(rule);

        // The listing and the detail panel share one navigation bar; the
        // sidebar has its own (on request), so the top of the window splits
        // where the panes do.
        let mut content = div().flex().flex_row().flex_1().min_h(px(0.));
        // The expanded preview takes the listing column *and* the detail panel,
        // because the panel was showing the same preview beside it — the same
        // title, the same facts, the same picture, twice. It leaves the sidebar
        // alone, which is what still makes this a pane rather than a modal: you
        // can change directory without collapsing it.
        if self.preview_expanded() {
            content = content.child(self.expanded_pane(cx));
        } else {
            content = content.child(self.listing_pane(cx));
            if !narrow && self.right_open {
                content = content
                    .child(self.resize_handle(PanelSide::Right, cx))
                    .child(self.detail_pane(cx));
            }
        }
        let main = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .child(self.nav_bar(cx))
            .child(Separator::horizontal())
            .child(content);
        row.child(main).into_any_element()
    }

    /// A panel's width for this frame: the one the user dragged it to, or
    /// the theme's `dropdown-width` until they have, kept between the floor
    /// and whatever the centre column can spare beside the other panel.
    fn panel_width(&self, side: PanelSide, cx: &App) -> f32 {
        let default = cx.theme().space().dropdown_width();
        let (floor, room) = self.panel_bounds(default);
        let simple = |asked: Option<u32>| {
            asked
                .map_or(default, |w| w as f32)
                .clamp(floor, room.max(floor))
        };
        // The other panel, at its own simple clamp, is what this one has to
        // fit beside. One-sided on purpose: two panels each clamped against
        // the other's clamped width would chase in circles. A floating panel
        // has no neighbour, and an expanded preview has hidden the right one.
        let docked = self.viewport_width >= default * 3.0;
        let other = match side {
            PanelSide::Left if docked && self.right_open && !self.preview_expanded() => {
                simple(self.config.detail_width)
            }
            PanelSide::Right if docked && self.left_open => simple(self.config.sidebar_width),
            _ => 0.0,
        };
        let asked = match side {
            PanelSide::Left => self.config.sidebar_width,
            PanelSide::Right => self.config.detail_width,
        };
        asked
            .map_or(default, |w| w as f32)
            .clamp(floor, (room - other).max(floor))
    }

    /// The floor every panel keeps, and the width the two panels may share
    /// in this window once the centre column has its minimum.
    fn panel_bounds(&self, default: f32) -> (f32, f32) {
        let floor = (default * PANEL_MIN_FACTOR).round();
        let room = (self.viewport_width * (1.0 - CENTER_MIN_FRACTION)).round();
        (floor, room)
    }

    /// The rule beside a docked panel, widened into a grip: the hairline
    /// stays where it was and a few invisible pixels either side of it take
    /// the pointer. Only drawn while the panel is open — collapsed, there is
    /// nothing to resize, and the rule is a plain rule.
    fn resize_handle(&mut self, side: PanelSide, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let thickness = theme.space().hairline();
        // The grip's reach, either side of the rule. Negative margins keep
        // it from moving the panes: the layout still sees one hairline.
        let reach = theme.space().sm().max(4.0);
        let rule = theme.border().opacity(0.2);
        let dragging = self.resizing.is_some_and(|r| r.side == side);
        let accent = theme.accent();
        let id = match side {
            PanelSide::Left => "resize-left",
            PanelSide::Right => "resize-right",
        };
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
                cx.listener(move |this, event: &gpui::MouseDownEvent, _w, cx| {
                    this.start_panel_resize(side, f32::from(event.position.x), cx)
                }),
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

    fn start_panel_resize(&mut self, side: PanelSide, x: f32, cx: &mut Context<Self>) {
        let start_width = self.panel_width(side, cx);
        self.resizing = Some(PanelResize {
            side,
            start_x: x,
            start_width,
        });
        cx.notify();
    }

    /// Follow the pointer: the panel is as wide as it was at mouse-down
    /// plus how far the pointer has travelled toward the window's centre.
    /// The clamp happens at render, against this frame's window, so the
    /// stored width is the asked-for one and a wider window gives it back.
    fn drag_panel_edge(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some(resize) = self.resizing else {
            return;
        };
        let travel = x - resize.start_x;
        let width = match resize.side {
            PanelSide::Left => resize.start_width + travel,
            PanelSide::Right => resize.start_width - travel,
        };
        let default = cx.theme().space().dropdown_width();
        let (floor, room) = self.panel_bounds(default);
        // Stored already clamped, so a drag past the limit does not bank an
        // invisible surplus that the next drag has to burn through.
        let width = width.clamp(floor, room.max(floor)).round() as u32;
        let slot = match resize.side {
            PanelSide::Left => &mut self.config.sidebar_width,
            PanelSide::Right => &mut self.config.detail_width,
        };
        if *slot != Some(width) {
            *slot = Some(width);
            cx.notify();
        }
    }

    /// Let go: the width the panel landed on is the one the next session
    /// opens with.
    fn end_panel_resize(&mut self, cx: &mut Context<Self>) {
        if self.resizing.take().is_none() {
            return;
        }
        if let Err(err) = self.config.save(&self.config_path) {
            self.notify_user(format!("could not save the panel width: {err}"), cx);
        }
        cx.notify();
    }

    // ----------------------------------------------- the listing's columns

    /// The size and age column widths for the directory on screen: what was
    /// dragged for it, or the built-in widths until then.
    fn column_widths(&self) -> (f32, f32) {
        let view = self.views.get(&self.current_path());
        let width = |asked: Option<u32>, default: f32| {
            asked
                .map_or(default, |w| w as f32)
                .clamp(COLUMN_MIN, COLUMN_MAX)
        };
        (
            width(view.size_width, SIZE_COLUMN),
            width(view.age_width, AGE_COLUMN),
        )
    }

    /// Record how the directory on screen is laid out, apply its sort to
    /// every tab looking at it, and keep the cursor's row in sight.
    fn update_view(&mut self, view: DirectoryView, cx: &mut Context<Self>) {
        let path = self.current_path();
        let sort = view.sort();
        self.views.set(&path, view);
        for listing in self.listings.values_mut() {
            if listing.path == path && listing.sort_order() != sort {
                listing.sort(sort);
            }
        }
        self.scroll_to_cursor();
        cx.notify();
    }

    /// The layout the user reached is the one the directory opens with next
    /// time, in every tab and every session.
    fn save_views(&mut self, cx: &mut Context<Self>) {
        if let Err(err) = self.views.save(&self.config_dir) {
            self.notify_user(format!("could not save the listing layout: {err}"), cx);
        }
    }

    /// A click on a column label: sort by it, or turn the sort around if it
    /// already is. The cursor stays on its entry and follows it to its new
    /// row — sorting is about the rows, not about what is selected.
    fn click_column(&mut self, key: SortKey, cx: &mut Context<Self>) {
        let mut view = self.views.get(&self.current_path());
        view.set_sort(view.sort().clicked(key));
        self.update_view(view, cx);
        self.save_views(cx);
    }

    fn start_column_resize(&mut self, divider: ColumnDivider, x: f32, cx: &mut Context<Self>) {
        let (start_size, start_age) = self.column_widths();
        self.column_resizing = Some(ColumnResize {
            divider,
            start_x: x,
            start_size,
            start_age,
        });
        cx.notify();
    }

    /// Follow the pointer: the boundary moves with it, so the column on its
    /// left grows by what the one on its right gives up. The name column
    /// takes whatever is left, so its boundary only has the size column to
    /// trade with; the other boundary trades size for age.
    fn drag_column_divider(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some(resize) = self.column_resizing else {
            return;
        };
        let travel = x - resize.start_x;
        let (size, age) = match resize.divider {
            ColumnDivider::Size => (resize.start_size - travel, resize.start_age),
            ColumnDivider::Age => {
                // Clamp the pair together, so that when one column hits its
                // floor the other stops too and the boundary stays under the
                // pointer rather than the columns sliding as a block.
                let age = (resize.start_age - travel).clamp(COLUMN_MIN, COLUMN_MAX);
                let size =
                    (resize.start_size + resize.start_age - age).clamp(COLUMN_MIN, COLUMN_MAX);
                (size, resize.start_size + resize.start_age - size)
            }
        };
        // Stored already clamped, so a drag past the limit does not bank an
        // invisible surplus that the next drag has to burn through.
        let size = size.clamp(COLUMN_MIN, COLUMN_MAX).round() as u32;
        let age = age.clamp(COLUMN_MIN, COLUMN_MAX).round() as u32;
        let mut view = self.views.get(&self.current_path());
        if view.size_width != Some(size) || view.age_width != Some(age) {
            view.size_width = Some(size);
            view.age_width = Some(age);
            self.update_view(view, cx);
        }
    }

    /// Let go: the widths the columns landed on are written down.
    fn end_column_resize(&mut self, cx: &mut Context<Self>) {
        if self.column_resizing.take().is_none() {
            return;
        }
        self.save_views(cx);
        cx.notify();
    }

    /// The column labels above the listing.
    ///
    /// Built from a plain `div` rather than a [`Row`]: this one is never the
    /// cursor and never marked, and giving it a row's interaction vocabulary
    /// would only invite it to light up as one. It borrows the row's metrics —
    /// height, padding, gap, column widths — so the labels line up with the
    /// values. Each label is a button that sorts by its column, and the rule
    /// before the size and age labels is a grip that resizes them.
    fn listing_header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let space = theme.space();
        let caption = theme.type_scale().caption();
        let dim = theme.dim_foreground();
        let fg = theme.foreground();
        let gap = space.control_gap();
        let (height, padding) = (space.control_height(), space.row_padding_x());
        let sort = self.views.get(&self.current_path()).sort();
        let (size_width, age_width) = self.column_widths();

        // Uppercase caption in secondary text — the same treatment
        // `SectionHeader` gives the sidebar's group labels, because this is
        // the same kind of thing. The sorted column carries a caret after its
        // name, pointing the way its values run down the list.
        let label = |key: SortKey, text: &'static str, cx: &mut Context<Self>| {
            let caret = (sort.key == key).then_some(if sort.descending {
                "\u{f0d7}" // nf-fa-caret_down
            } else {
                "\u{f0d8}" // nf-fa-caret_up
            });
            div()
                .id(("sort-by", key as usize))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(gap * 0.5))
                .min_w(px(0.))
                .cursor_pointer()
                .text_color(if sort.key == key { fg } else { dim })
                .hover(|style| style.text_color(fg))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.click_column(key, cx);
                }))
                .child(div().truncate().child(text))
                .children(caret)
        };
        let name = label(SortKey::Name, "NAME", cx);
        let size = label(SortKey::Size, "SIZE", cx);
        let age = label(SortKey::Age, "AGE", cx);

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(gap))
            .w_full()
            .h(px(height))
            .px(px(padding))
            .text_size(px(caption))
            .text_color(dim)
            // The icon column has no label — a glyph needs none — but it
            // still has to be reserved, or every label sits one column to
            // the left.
            .child(div().w(px(caption * ICON_COLUMN)).flex_shrink_0())
            .child(div().flex_1().min_w(px(0.)).child(name))
            .child(
                div()
                    .relative()
                    .w(px(size_width))
                    .flex_shrink_0()
                    .child(size)
                    .child(self.column_grip(ColumnDivider::Size, gap, cx)),
            )
            .child(
                div()
                    .relative()
                    .w(px(age_width))
                    .flex_shrink_0()
                    .child(age)
                    .child(self.column_grip(ColumnDivider::Age, gap, cx)),
            )
            .into_any_element()
    }

    /// The rule at a header column's left edge, widened into a grip the way
    /// the panel rules are: a faint hairline in the middle of the gap before
    /// the column, and a few invisible pixels either side that take the
    /// pointer. Absolutely placed, so it costs the header no width and the
    /// labels stay over the values.
    fn column_grip(
        &mut self,
        divider: ColumnDivider,
        gap: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let thickness = theme.space().hairline();
        let reach = theme.space().sm().max(4.0);
        let rule = theme.border().opacity(0.2);
        let lit = theme.border();
        let accent = theme.accent();
        let dragging = self.column_resizing.is_some_and(|r| r.divider == divider);
        let (id, group) = match divider {
            ColumnDivider::Size => ("column-grip-size", "column-grip-size"),
            ColumnDivider::Age => ("column-grip-age", "column-grip-age"),
        };
        div()
            .id(id)
            .group(group)
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(-(gap + thickness) / 2.0 - reach))
            .w(px(thickness + reach * 2.0))
            .flex()
            .flex_row()
            .justify_center()
            .cursor_col_resize()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _w, cx| {
                    this.start_column_resize(divider, f32::from(event.position.x), cx)
                }),
            )
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
            )
            .into_any_element()
    }

    /// Below this the panels dock as overlays instead of taking space.
    ///
    /// Derived from the token scale rather than a magic number, so it tracks
    /// the user's text size: at a larger `base-size` the panels need more room
    /// and should give up sooner.
    fn is_narrow(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let panel = cx.theme().space().dropdown_width();
        // Two panels plus a listing at least as wide as one of them. The
        // same test, from the recorded width, decides in `panel_width`
        // whether a panel has a docked neighbour to leave room for.
        window.viewport_size().width < px(panel * 3.0)
    }

    /// The panels, drawn over the listing when the window is too narrow to
    /// dock them.
    fn floating_panels(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if !self.is_narrow(window, cx) {
            return Vec::new();
        }
        let scrim = omarchy_ui::color(cx.theme().tokens.palette.darker_background()).opacity(0.6);

        let mut layers = Vec::new();
        for (open, left) in [(self.left_open, true), (self.right_open, false)] {
            if !open {
                continue;
            }
            // Docked panels are chromeless, but a floating one covers the
            // listing and would be unreadable without its own ground and an
            // edge to separate it from what it hides.
            let background = cx.theme().background();
            let inner = if left {
                self.sidebar_column(cx)
            } else {
                self.detail_pane(cx)
            };
            let mut panel = div().flex().flex_row().h_full().bg(background);
            panel = if left {
                panel.child(inner).child(Separator::vertical())
            } else {
                panel.child(Separator::vertical()).child(inner)
            };
            let mut layer = div()
                .id(if left { "left-float" } else { "right-float" })
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .right_0()
                .bg(scrim)
                .flex()
                .flex_row()
                .on_click(cx.listener(move |this, _e, _w, cx| {
                    if left {
                        this.left_open = false;
                    } else {
                        this.right_open = false;
                    }
                    cx.notify();
                }));
            layer = if left {
                layer.justify_start()
            } else {
                layer.justify_end()
            };
            layers.push(layer.child(panel).into_any_element());
        }
        layers
    }

    fn sidebar_pane(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let width = self.panel_width(PanelSide::Left, cx);
        let focused = self.pane == Pane::Sidebar;
        let cursor = self.place_cursor;
        // Exact match, so browsing under Home does not leave Home lit the
        // whole time.
        let system_count = self.places.system().len();

        let mut column = div().flex().flex_col().w_full();
        if system_count > 0 {
            column = column.child(SectionHeader::new("places"));
        }
        // Cloned so the `self.places` borrow ends before `cx.listener`.
        let places: Vec<Place> = self.places.all().cloned().collect();
        for (index, place) in places.into_iter().enumerate() {
            if index == system_count {
                column = column.child(SectionHeader::new("pinned"));
            }
            let dest = place.path.clone();
            let row = place_row(index, &place, cursor == index && focused, focused, cx)
                .on_click(cx.listener(move |this, _event, window, cx| {
                    // A place is a shortcut, so one click goes there. Focus lands
                    // in the listing, because browsing is what you do next.
                    this.place_cursor = index;
                    this.open_place(cx);
                    this.focus_pane(Pane::Listing, window, cx);
                }))
                // And somewhere to drop: entries dragged onto a place move
                // into its directory.
                .drag_over::<DraggedEntries>(drop_highlight)
                .on_drop(cx.listener(move |this, dragged: &DraggedEntries, _w, cx| {
                    this.drop_entries(dragged, dest.clone(), cx);
                }));
            column = column.child(row);
        }

        // NETWORK, when anything is saved — the pinned discipline: no rows,
        // no section. A location is a Row like a place, plus its forget ✕.
        if !self.network.is_empty() {
            column = column.child(SectionHeader::new("network"));
            let caption = cx.theme().type_scale().caption();
            let dim = cx
                .theme()
                .dim_foreground_on(cx.theme().tokens.palette.lighter_background());
            let locations: Vec<network::Location> = self.network.clone();
            for (index, location) in locations.into_iter().enumerate() {
                column = column.child(
                    Row::new(("network", index))
                        .child(
                            div()
                                .w(px(caption * ICON_COLUMN))
                                .flex_shrink_0()
                                .text_color(dim)
                                .child("\u{f233}"), // nf-fa-server
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .child(location.name.clone()),
                        )
                        .on_click(cx.listener(move |this, _e, window, cx| {
                            this.open_network(index, cx);
                            this.focus_pane(Pane::Listing, window, cx);
                        }))
                        .on_right_click(cx.listener(
                            move |this, event: &gpui::MouseDownEvent, window, cx| {
                                this.open_network_menu(index, Some(event.position), window, cx);
                            },
                        )),
                );
            }
        }

        // Workspaces sit above the global tabs, per §6.2. The global group is
        // rendered without a header — it is the implicit default, not a
        // container the user made.
        let active_ws = self.session.active_workspace();
        let active_tab = self
            .session
            .workspace(active_ws)
            .map(|w| w.active_tab)
            .unwrap_or(0);

        // One tab row per tab in a workspace, unless it is collapsed.
        let rows_of = |this: &Self, w: usize, cx: &mut Context<Self>| -> Vec<AnyElement> {
            let workspace = &this.session.workspaces()[w];
            if workspace.collapsed {
                return Vec::new();
            }
            let count = workspace.tabs.len();
            let accent = cx.theme().accent();
            // The insertion line, when a dragged tab is over this group.
            let drop_at = this
                .tab_drop
                .filter(|drop| drop.workspace == w)
                .map(|drop| drop.index);
            let line = move |top: bool| {
                let line = div().absolute().left_0().right_0().h(px(2.)).bg(accent);
                if top {
                    line.top(px(-1.))
                } else {
                    line.bottom(px(-1.))
                }
            };
            workspace
                .tabs
                .iter()
                .enumerate()
                .map(|(t, tab)| {
                    let on_close = (this.session.total_tabs() > 1).then(|| {
                        cx.listener(move |this: &mut Self, _event, _window, cx| {
                            this.close_tab_at(w, t, cx);
                        })
                    });
                    let dest = tab.path().to_path_buf();
                    let row = tab_row(w, t, tab, w == active_ws && t == active_tab, on_close, cx)
                        .on_click(cx.listener(move |this, _event, window, cx| {
                            this.activate_tab(w, t, cx);
                            this.focus_pane(Pane::Listing, window, cx);
                        }))
                        // Entries dropped on a tab move into the directory it
                        // shows; the tab itself stays where it is.
                        .drag_over::<DraggedEntries>(drop_highlight)
                        .on_drop(cx.listener(move |this, dragged: &DraggedEntries, _w, cx| {
                            this.drop_entries(dragged, dest.clone(), cx);
                        }))
                        // A tab dragged over this row lands before it from the
                        // top half, after it from the bottom half. gpui fires
                        // drag-move for every pointer move, so the row checks
                        // its own bounds.
                        .on_drag_move::<DraggedTab>(cx.listener(
                            move |this, event: &DragMoveEvent<DraggedTab>, _w, cx| {
                                let position = event.event.position;
                                if !event.bounds.contains(&position) {
                                    return;
                                }
                                let before = position.y < event.bounds.center().y;
                                let index = if before { t } else { t + 1 };
                                let same = this
                                    .tab_drop
                                    .is_some_and(|d| d.workspace == w && d.index == index);
                                if !same {
                                    this.tab_drop = Some(TabDrop {
                                        workspace: w,
                                        index,
                                        bounds: event.bounds,
                                    });
                                    cx.notify();
                                }
                            },
                        ))
                        .on_drop(cx.listener(move |this, dragged: &DraggedTab, _w, cx| {
                            let at = this
                                .tab_drop
                                .take()
                                .filter(|drop| drop.workspace == w)
                                .map_or(t, |drop| drop.index);
                            if this
                                .session
                                .move_tab_to(dragged.workspace, dragged.index, w, at)
                            {
                                this.after_tab_change(cx);
                            }
                            cx.notify();
                        }));
                    // Wrapped so the line can sit on the row's edge: above
                    // this row, or below the last one for "after the end".
                    div()
                        .relative()
                        .w_full()
                        .child(row)
                        .children((drop_at == Some(t)).then(|| line(true)))
                        .children((t + 1 == count && drop_at == Some(count)).then(|| line(false)))
                        .into_any_element()
                })
                .collect()
        };

        // One TABS section for everything: the named workspaces as groups
        // under their own headers, then the global tabs as the plain list —
        // the implicit place tabs live, not something the user made.
        let mut tabs = div().flex().flex_col().w_full().child(tabs_header(cx));
        let (global, named): (Vec<usize>, Vec<usize>) = (0..self.session.workspaces().len())
            .partition(|&w| self.session.workspaces()[w].is_global());

        // The workspaces first, each a named group closed by a rule; the
        // global tabs follow as the plain list at the bottom. One section:
        // a workspace is a way of grouping tabs, not a third kind of thing.
        for w in named {
            // The header doubles as the drop target for its workspace: dropping
            // a tab on the group's name is the obvious gesture, and it stays
            // hittable even when the group is empty.
            let (label, collapsed) = {
                let workspace = &self.session.workspaces()[w];
                (workspace.label().to_string(), workspace.collapsed)
            };
            tabs = tabs.child(drop_header(w, label, collapsed, w == active_ws, cx));
            tabs = tabs.children(rows_of(self, w, cx));
            // Its own faint "new tab", after its last tab: a tab added here
            // belongs to this workspace, not to whichever one is active.
            // Folded away with the tabs when the workspace is collapsed.
            if !collapsed {
                tabs = tabs.child(quiet_row(
                    ("ws-tab-new", w),
                    "\u{f067}", // nf-fa-plus
                    "New tab",
                    cx.listener(move |this, _event, window, cx| {
                        let path = this.current_path();
                        this.session.activate_workspace(w);
                        this.session.new_tab(path);
                        this.after_tab_change(cx);
                        this.focus_pane(Pane::Listing, window, cx);
                    }),
                    cx,
                ));
            }
            // Each group closes with a rule, flush under its last row, so
            // consecutive workspaces do not run into the global list.
            tabs = tabs.child(Separator::horizontal());
        }
        for w in global {
            tabs = tabs.children(rows_of(self, w, cx));
        }

        // A faint "new tab" row after the last tab: `^t` for the mouse. At
        // 0.3 it reads as an affordance rather than a tab, and it brightens
        // under the pointer so it is plainly clickable.
        tabs = tabs.child(quiet_row(
            "tab-new",
            "\u{f067}", // nf-fa-plus
            "New tab",
            cx.listener(|this, _event, window, cx| {
                this.new_tab(cx);
                this.focus_pane(Pane::Listing, window, cx);
            }),
            cx,
        ));

        let pad = cx.theme().space().sm();
        // Places and tabs are two different kinds of thing — shortcuts above,
        // where-you-are below — so the rule between them gets real air, not
        // the row rhythm the sections inside each half keep.
        let gap = cx.theme().space().space(24.0);
        div()
            .id("sidebar")
            .key_context("Sidebar")
            .track_focus(&self.sidebar_focus)
            .w(px(width))
            .flex_shrink_0()
            .h_full()
            // No border, no fill: the panels are regions of one surface, and
            // the vertical rules in `panes` are the only thing dividing them.
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex()
                    .flex_col()
                    .h_full()
                    .py(px(pad))
                    .overflow_y_scroll()
                    .track_scroll(&self.left_scroll)
                    // The pointer leaving every tab row takes the insertion
                    // line with it: drag-move fires here for every move, and
                    // the bounds kept in `tab_drop` say whether it is still
                    // over the row that set it.
                    .on_drag_move::<DraggedTab>(cx.listener(
                        |this, event: &DragMoveEvent<DraggedTab>, _w, cx| {
                            if let Some(drop) = this.tab_drop
                                && !drop.bounds.contains(&event.event.position)
                            {
                                this.tab_drop = None;
                                cx.notify();
                            }
                        },
                    ))
                    .child(column)
                    .child(div().w_full().mt(px(gap)).child(Separator::horizontal()))
                    .child(tabs),
            )
            .into_any_element()
    }

    fn listing_pane(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(listing) = self.listing() else {
            let dim = cx.theme().dim_foreground();
            return div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(dim)
                .child("reading…")
                .into_any_element();
        };
        let visible = listing.visible(self.show_hidden);
        let empty_reason = describe_empty(listing, self.show_hidden);

        if visible.is_empty() {
            let dim = cx.theme().dim_foreground();
            return div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(dim)
                .child(if empty_reason.is_empty() {
                    "nothing here"
                } else {
                    empty_reason
                })
                .into_any_element();
        }

        // `cx.processor` gives the closure `&mut Self`, so rows read straight
        // from the listing with no clone per frame.
        let list = uniform_list(
            "listing",
            visible.len(),
            cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                // Collected up front so the listing borrow ends before
                // `cx.listener` needs `this` again.
                let cursor = this.cursors.get(&this.tab_id()).copied();
                // The marked set, built once for the frame: every marked row
                // offers the whole set as its drag payload.
                let marked: Arc<Vec<DragItem>> = Arc::new(
                    this.selection()
                        .into_iter()
                        .filter_map(|index| this.listing()?.get(index))
                        .map(|entry| DragItem {
                            path: entry.path.clone(),
                            name: entry.name.clone(),
                            is_dir: entry.kind.is_dir(),
                        })
                        .collect(),
                );
                let columns = this.column_widths();
                let Some(listing) = this.listings.get(&this.tab_id()) else {
                    return Vec::new();
                };
                let visible = listing.visible(this.show_hidden);
                let rows: Vec<(usize, usize, Entry, bool)> = range
                    .filter_map(|position| {
                        let index = *visible.get(position)?;
                        let entry = listing.get(index)?;
                        Some((
                            position,
                            index,
                            entry.clone(),
                            this.is_selected(&entry.name),
                        ))
                    })
                    .collect();

                // Looked up per visible row, not walked: the rollup was indexed
                // by path once when the status landed, so this is a hash lookup
                // per row rather than a scan of every change (§6.9).
                let states: Vec<Option<git::State>> = rows
                    .iter()
                    .map(|(_, _, entry, _)| this.git_state(&entry.path))
                    .collect();

                rows.into_iter()
                    .zip(states)
                    .map(|((position, index, entry, is_selected), state)| {
                        // A marked row drags the whole selection; an unmarked
                        // one drags itself alone, and leaves the marks as they
                        // are — the convention everywhere else in the desktop.
                        let items = if is_selected {
                            marked.clone()
                        } else {
                            Arc::new(vec![DragItem {
                                path: entry.path.clone(),
                                name: entry.name.clone(),
                                is_dir: entry.kind.is_dir(),
                            }])
                        };
                        let is_dir = entry.kind.is_dir();
                        let dest = entry.path.clone();
                        let mut row = entry_row(
                            position,
                            &entry,
                            cursor == Some(index),
                            is_selected,
                            state,
                            columns,
                            cx,
                        )
                        .draggable(
                            DraggedEntries { items },
                            |payload: &DraggedEntries, _position, _window, cx: &mut App| {
                                drag_preview(payload.label(), cx)
                            },
                        )
                        .on_click(
                            cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                                // Single click selects, double opens — the
                                // convention everywhere else in the desktop.
                                // Modifiers mark: ^click flips one row,
                                // \u{21e7}click marks the run from the cursor.
                                this.focus_pane(Pane::Listing, window, cx);
                                let modifiers = event.modifiers();
                                if modifiers.shift {
                                    let anchor = this.cursor().unwrap_or(index);
                                    this.mark_range(anchor, index);
                                    this.set_cursor(Some(index));
                                } else if modifiers.control {
                                    this.toggle_mark(index);
                                    this.set_cursor(Some(index));
                                } else {
                                    this.clear_selection();
                                    this.set_cursor(Some(index));
                                    if event.click_count() >= 2 {
                                        this.open_selected(cx);
                                    }
                                }
                                cx.notify();
                            }),
                        )
                        // Right click: cursor follows the click — the menu
                        // acts on the entry under it — and the card opens
                        // at the pointer. Marks survive only when the click
                        // lands on one of them.
                        .on_right_click(cx.listener(
                            move |this, event: &gpui::MouseDownEvent, window, cx| {
                                this.focus_pane(Pane::Listing, window, cx);
                                if !this.is_selected(&entry.name) {
                                    this.clear_selection();
                                }
                                this.set_cursor(Some(index));
                                this.open_entry_menu(Some(event.position), window, cx);
                            },
                        ));
                        // A directory is somewhere to drop; a file is not, and
                        // never lights up — so what lights up can be trusted.
                        if is_dir {
                            row = row.drag_over::<DraggedEntries>(drop_highlight).on_drop(
                                cx.listener(move |this, dragged: &DraggedEntries, _w, cx| {
                                    this.drop_entries(dragged, dest.clone(), cx);
                                }),
                            );
                        }
                        row.into_any_element()
                    })
                    .collect()
            }),
        )
        .h_full()
        .track_scroll(&self.scroll);

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.))
            // The header sits outside the scrolling area, so it stays put while
            // the listing moves under it and the scrollbar does not run across
            // it.
            .child(self.listing_header(cx))
            .child(Separator::horizontal())
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.))
                    .child(list)
                    // Bare gpui paints no scrollbar at all — the one thing
                    // gpui-component is here for in M3.
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            .child(Scrollbar::vertical(&self.scroll)),
                    ),
            )
            .into_any_element()
    }

    fn detail_pane(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let surface = theme.tokens.palette.lighter_background();
        // Reduced from panel-padding: the info is a fact sheet, not a card.
        let pad = theme.space().row_padding_x();
        let pad_y = theme.space().md();
        let (dim, caption, small_gap) = (
            theme.dim_foreground_on(surface),
            theme.type_scale().caption(),
            theme.space().sm(),
        );
        let width = self.panel_width(PanelSide::Right, cx);

        // Three states, and they are genuinely different: nothing selected,
        // something selected whose read has not landed, and a preview.
        let content = match (&self.preview, self.cursor().is_some()) {
            (None, false) => div()
                .text_color(dim)
                .child("no selection")
                .into_any_element(),
            (None, true) => div()
                .text_size(px(caption))
                .text_color(dim)
                .child("reading\u{2026}")
                .into_any_element(),
            (Some(loaded), _) => render_info(loaded, cx),
        };

        // The expand affordance, on the states §6.5 says can overflow.
        let expandable = self
            .preview
            .as_ref()
            .is_some_and(|l| l.preview.body.is_expandable());

        // The selected entry's actions, for the mouse (M9): the same verbs as
        // `⏎`/`a`/`s`, beside the facts they act on. The directory's own
        // actions live in the status bar with the directory's facts.
        let entry = self.cursor().and_then(|i| self.listing()?.get(i)).cloned();
        let selected = entry.is_some();
        // Glyphs alone unless the setting asks for the words; the verb is
        // then a hover away.
        let compact = !self.config.button_labels;
        let mut actions: Vec<ActionButton> = Vec::new();
        if selected {
            actions.push(
                ActionButton::new("detail-open")
                    .glyph("\u{f08e}") // nf-fa-external_link
                    .label("Open")
                    .compact(compact)
                    .on_click(cx.listener(|this, _e, _w, cx| this.open_selected(cx))),
            );
            actions.push(
                ActionButton::new("detail-agent")
                    .glyph("\u{f06a9}") // nf-md-robot
                    .label("Agent")
                    .compact(compact)
                    .on_click(cx.listener(|this, _e, window, cx| this.ask_agent(window, cx))),
            );
            actions.push(
                ActionButton::new("detail-share")
                    .glyph("\u{f1e0}") // nf-fa-share_alt
                    .label("Share")
                    .compact(compact)
                    .on_click(cx.listener(|this, _e, _w, cx| this.share_selected(cx))),
            );
            actions.push(
                ActionButton::new("detail-copy")
                    .glyph("\u{f0c5}") // nf-fa-copy
                    .label("Copy")
                    .compact(compact)
                    .on_click(cx.listener(|this, _e, window, cx| this.copy_selected(window, cx))),
            );
            actions.push(
                ActionButton::new("detail-cut")
                    .glyph("\u{f0c4}") // nf-fa-scissors
                    .label("Cut")
                    .compact(compact)
                    .on_click(cx.listener(|this, _e, _w, cx| this.cut_selected(cx))),
            );
            actions.push(
                ActionButton::new("detail-path")
                    .glyph("\u{f0c1}") // nf-fa-link
                    .label("Path")
                    .compact(compact)
                    .on_click(cx.listener(|this, _e, _w, cx| this.copy_path_selected(cx))),
            );
            actions.push(
                ActionButton::new("detail-move")
                    .glyph("\u{f061}") // nf-fa-arrow_right
                    .label("Move")
                    .compact(compact)
                    .on_click(cx.listener(|this, _e, window, cx| this.move_selected(window, cx))),
            );
            actions.push(
                ActionButton::new("detail-zip")
                    .glyph("\u{f1c6}") // nf-fa-file_archive
                    .label("Zip")
                    .compact(compact)
                    .on_click(cx.listener(|this, _e, _w, cx| this.compress_selected(cx))),
            );
            actions.push(
                ActionButton::new("detail-delete")
                    .glyph("\u{f1f8}") // nf-fa-trash
                    .label("Delete")
                    .compact(compact)
                    .on_click(cx.listener(|this, _e, window, cx| this.delete_selected(window, cx))),
            );
        }
        // Directories additionally pin — a file cannot live in the sidebar.
        if let Some(entry) = entry.as_ref().filter(|e| e.kind.is_dir()) {
            let state = self.pin_state(&entry.path);
            actions.push(pin_button(
                "detail-pin",
                entry.path.clone(),
                state,
                compact,
                cx,
            ));
        }
        // The expand affordance rides with the other verbs (on request).
        if expandable {
            actions.push(
                ActionButton::new("preview-fullscreen")
                    .glyph("\u{f06e}") // nf-fa-eye
                    .label("Preview")
                    .compact(compact)
                    .on_click(cx.listener(|this, _e, _w, cx| this.toggle_preview(cx))),
            );
        }

        // One wrapping row of verbs; too narrow for a line, it wraps tidily.
        // Below the facts, behind its own rule (on request): the sheet reads
        // top to bottom — cover, facts, then what can be done about them.
        let toolbar = (selected || expandable).then(|| {
            div()
                .flex()
                .flex_col()
                .child(Separator::horizontal())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .flex_wrap()
                        .gap(px(small_gap))
                        .px(px(pad))
                        .py(px(pad_y))
                        .children(actions),
                )
        });

        // The preview rides on top, flush to the panel edges like a cover,
        // with a rule between it and the fact sheet below (on request).
        // Double click on it expands, like double click opens in the listing.
        let cover = self
            .preview
            .as_ref()
            .filter(|loaded| loaded.preview.body.has_cover())
            .map(|loaded| {
                div()
                    .id("detail-preview")
                    .w_full()
                    // A cover, not a reading pane: capped at 250, scrollable
                    // past that, and set at caption size — the expanded view
                    // is where actual reading happens.
                    .max_h(px(250.))
                    .overflow_y_scroll()
                    .text_size(px(caption))
                    .on_click(cx.listener(|this, event: &gpui::ClickEvent, _w, cx| {
                        if event.click_count() >= 2 && !this.preview_expanded() {
                            this.toggle_preview(cx);
                        }
                    }))
                    .child(render_body(loaded, Target::Pane { width }, cx))
                    .into_any_element()
            });

        div()
            .w(px(width))
            .flex_shrink_0()
            .h_full()
            .child(
                div()
                    .id("detail-scroll")
                    .flex()
                    .flex_col()
                    .h_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.right_scroll)
                    .children(cover.is_some().then(|| {
                        div()
                            .flex()
                            .flex_col()
                            .children(cover)
                            .child(Separator::horizontal())
                            .into_any_element()
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .px(px(pad))
                            .py(px(pad_y))
                            .child(content),
                    )
                    // In the flow rather than floating over the facts: an
                    // absolutely-positioned control inside a scroll container
                    // scrolls away with the content.
                    .children(toolbar),
            )
            .into_any_element()
    }

    /// The modal layer, when something is open.
    fn overlay_layer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let viewport = window.viewport_size();
        let overlay = self.overlay.as_ref()?;
        let dismiss = cx.listener(|this, _event: &gpui::ClickEvent, window, cx| {
            this.dismiss_overlay(window, cx)
        });

        let modal = match overlay {
            Overlay::Help => {
                let query = self.input.read(cx).value().trim().to_lowercase();
                let theme = cx.theme();
                let (caption, body, dim, bright) = (
                    theme.type_scale().caption(),
                    theme.type_scale().body(),
                    theme.dim_foreground(),
                    theme.bright_foreground(),
                );
                let (gap, row_gap, col_gap) =
                    (theme.space().md(), theme.space().xs(), theme.space().xl());

                // A group survives if its title matches; otherwise it keeps
                // only the entries whose keys or description do.
                let groups: Vec<_> = SHORTCUTS
                    .iter()
                    .filter_map(|(title, entries)| {
                        let kept: Vec<_> =
                            if query.is_empty() || title.to_lowercase().contains(&query) {
                                entries.iter().collect()
                            } else {
                                entries
                                    .iter()
                                    .filter(|(keys, action)| {
                                        keys.to_lowercase().contains(&query)
                                            || action.to_lowercase().contains(&query)
                                    })
                                    .collect()
                            };
                        (!kept.is_empty()).then_some((*title, kept))
                    })
                    .collect();

                let sheet = if groups.is_empty() {
                    div()
                        .text_size(px(caption))
                        .text_color(dim)
                        .child("no shortcut matches")
                        .into_any_element()
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(gap))
                        .children(groups.into_iter().map(|(title, entries)| {
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(row_gap))
                                .child(
                                    div()
                                        .text_size(px(caption))
                                        .text_color(dim)
                                        .child(title.to_uppercase()),
                                )
                                .children(entries.into_iter().map(|(keys, action)| {
                                    div()
                                        .flex()
                                        .flex_row()
                                        .justify_between()
                                        .gap(px(col_gap))
                                        .text_size(px(body))
                                        .child(div().text_color(bright).child(*keys))
                                        .child(div().text_color(dim).child(*action))
                                }))
                        }))
                        .into_any_element()
                };

                Modal::new("help", "Shortcuts")
                    .size(ModalSize::Large)
                    .child(modal_inset(cx).child(Input::new(&self.input)))
                    .child(
                        modal_inset(cx)
                            .id("help-scroll")
                            .flex()
                            .flex_col()
                            .max_h(px(520.))
                            .overflow_y_scroll()
                            .child(sheet),
                    )
                    .hint("esc", "close")
                    .on_dismiss(dismiss)
                    .into_any_element()
            }
            Overlay::WorkspaceMenu { workspace } => {
                let workspace = *workspace;
                let name = self
                    .session
                    .workspace(workspace)
                    .map(|w| w.label().to_string())
                    .unwrap_or_default();
                Modal::new("ws-menu", name)
                    .subtitle("deleting keeps the tabs — they move to the default group")
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                Row::new("ws-rename")
                                    .child(div().flex_1().child("Rename\u{2026}"))
                                    .on_click(cx.listener(move |this, _e, window, cx| {
                                        this.open_workspace_prompt(Some(workspace), window, cx)
                                    })),
                            )
                            .child(Separator::horizontal().subtle())
                            .child(
                                Row::new("ws-collapse")
                                    .child(
                                        div().flex_1().child(
                                            if self
                                                .session
                                                .workspace(workspace)
                                                .is_some_and(|w| w.collapsed)
                                            {
                                                "Expand"
                                            } else {
                                                "Collapse"
                                            },
                                        ),
                                    )
                                    .on_click(cx.listener(move |this, _e, window, cx| {
                                        this.toggle_workspace_collapsed(workspace, cx);
                                        this.dismiss_overlay(window, cx);
                                    })),
                            )
                            .child(Separator::horizontal().subtle())
                            .child(
                                Row::new("ws-delete")
                                    .child(div().flex_1().child("Delete workspace"))
                                    .on_click(cx.listener(move |this, _e, window, cx| {
                                        this.session.delete_workspace(workspace);
                                        this.after_tab_change(cx);
                                        this.dismiss_overlay(window, cx);
                                    })),
                            ),
                    )
                    .hint("esc", "close")
                    .on_dismiss(dismiss)
                    .into_any_element()
            }
            Overlay::AddNetwork => Modal::new("add-network", "Add network location")
                .subtitle("smb://host/share \u{b7} sftp://user@host/path \u{b7} davs://host/path")
                .child(modal_inset(cx).child(Input::new(&self.input)))
                .hint("\u{23ce}", "add")
                .hint("esc", "cancel")
                .on_dismiss(dismiss)
                .into_any_element(),
            Overlay::NetworkMenu { index, position } => {
                let (index, position) = (*index, *position);
                let location = self.network.get(index).cloned()?;
                let mounted = network::mount_point(&location.uri).is_some();
                let dim_half = cx.theme().dim_foreground().opacity(0.5);
                let dim = cx.theme().dim_foreground();
                let icon_w = cx.theme().type_scale().caption() * 1.6;

                let item = |id: &'static str,
                            glyph: &'static str,
                            label: &'static str,
                            enabled: bool,
                            act: ContextAction,
                            cx: &mut Context<Explorer>| {
                    let mut row = Row::new(id)
                        .child(
                            div()
                                .w(px(icon_w))
                                .flex_shrink_0()
                                .text_color(if enabled { dim } else { dim_half })
                                .child(glyph),
                        )
                        .child(div().flex_1().child(label));
                    if enabled {
                        row = row.on_click(cx.listener(move |this, _e, window, cx| {
                            this.dismiss_overlay(window, cx);
                            act(this, window, cx);
                        }));
                    }
                    row.into_any_element()
                };

                let rows: Vec<AnyElement> = vec![
                    item(
                        "net-open",
                        "\u{f07b}",
                        "Open",
                        true,
                        Box::new(move |this, _w, cx| this.open_network(index, cx)),
                        cx,
                    ),
                    item(
                        "net-unmount",
                        "\u{f0ac}",
                        "Unmount",
                        mounted,
                        Box::new(move |this, _w, cx| this.unmount_network(index, cx)),
                        cx,
                    ),
                    item(
                        "net-forget",
                        "\u{f00d}",
                        "Forget",
                        true,
                        Box::new(move |this, _w, cx| this.remove_network(index, cx)),
                        cx,
                    ),
                ];

                menu_surround(location.name.clone(), rows, position, viewport, dismiss, cx)
            }
            Overlay::CopyImage {
                name,
                variants,
                encoded,
                cursor,
                ..
            } => {
                let theme = cx.theme();
                let (caption, dim, row_height) = (
                    theme.type_scale().caption(),
                    theme.dim_foreground(),
                    theme.space().control_height(),
                );
                let cursor = *cursor;
                let mut rows: Vec<AnyElement> = Vec::new();
                let row = |i: usize, label: String, detail: String, cx: &mut Context<Explorer>| {
                    Row::new(("copy-row", i))
                        .cursor(i == cursor)
                        .focused(true)
                        .child(div().flex_1().min_w(px(0.)).truncate().child(label))
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(caption))
                                .text_color(dim)
                                .child(detail),
                        )
                        .on_click(cx.listener(move |this, _e, window, cx| {
                            if let Some(Overlay::CopyImage { cursor, .. }) = &mut this.overlay {
                                *cursor = i;
                            }
                            this.confirm_copy_image(window, cx);
                        }))
                        .into_any_element()
                };
                rows.push(row(
                    0,
                    "The file".to_string(),
                    "paste into a directory, or its path into a terminal".to_string(),
                    cx,
                ));
                for (i, variant) in variants.iter().enumerate() {
                    let label = match (variant.original, variant.width) {
                        (true, 0) => "PNG".to_string(),
                        (true, _) => format!(
                            "PNG, {} \u{d7} {} (original size)",
                            variant.width, variant.height
                        ),
                        (false, _) => format!("PNG, {} \u{d7} {}", variant.width, variant.height),
                    };
                    let detail = match encoded.get(i).and_then(|slot| slot.as_ref()) {
                        Some(Ok(bytes)) => format_size(bytes.len() as u64),
                        Some(Err(_)) => "could not convert".to_string(),
                        None => "converting\u{2026}".to_string(),
                    };
                    rows.push(row(i + 1, label, detail, cx));
                }
                // Until the size is known there are no PNG rows to show, and
                // an empty gap under the first row reads as a glitch.
                if variants.is_empty() {
                    rows.push(
                        modal_inset(cx)
                            .h(px(row_height))
                            .flex()
                            .items_center()
                            .text_size(px(caption))
                            .text_color(dim)
                            .child("measuring the picture\u{2026}")
                            .into_any_element(),
                    );
                }
                Modal::new("copy-image", "Copy")
                    .subtitle(format!(
                        "{name} \u{2014} as a file, or as a picture to paste"
                    ))
                    .child(div().flex().flex_col().children(separated(rows)))
                    .hint("\u{23ce}", "copy")
                    .hint("\u{2193}\u{2191}", "select")
                    .hint("esc", "close")
                    .on_dismiss(dismiss)
                    .into_any_element()
            }
            Overlay::Refused {
                title,
                subtitle,
                reasons,
            } => {
                let theme = cx.theme();
                let (urgent, caption) = (theme.urgent(), theme.type_scale().caption());
                let (row_height, pad_x) = (
                    theme.space().control_height(),
                    theme.space().row_padding_x(),
                );
                let rows = reasons.iter().map(|reason| {
                    div()
                        .flex()
                        .items_center()
                        .h(px(row_height))
                        .px(px(pad_x))
                        .text_size(px(caption))
                        .text_color(urgent)
                        .child(reason.clone())
                });
                Modal::new("refused", *title)
                    .subtitle(subtitle.clone())
                    .child(div().flex().flex_col().children(rows))
                    .hint("esc", "close")
                    .on_dismiss(dismiss)
                    .into_any_element()
            }
            Overlay::Delete { name, is_dir, .. } => {
                let urgent = cx.theme().urgent();
                let row = Row::new("delete-confirm")
                    .cursor(true)
                    .focused(true)
                    .child(div().flex_1().text_color(urgent).child("Move to the trash"))
                    .on_click(cx.listener(|this, _e, window, cx| this.confirm_delete(window, cx)))
                    .into_any_element();
                Modal::new("delete", format!("Delete \u{201c}{name}\u{201d}?"))
                    .subtitle(if *is_dir {
                        "the directory and everything in it go to the trash, where they can be restored"
                    } else {
                        "it goes to the trash, where it can be restored"
                    })
                    .child(div().flex().flex_col().child(row))
                    .hint("\u{23ce}", "trash")
                    .hint("esc", "keep")
                    .on_dismiss(dismiss)
                    .into_any_element()
            }
            Overlay::Workspace { editing } => {
                let title = if editing.is_some() {
                    "Rename workspace"
                } else {
                    "New workspace"
                };
                Modal::new("workspace-prompt", title)
                    .subtitle(if editing.is_some() {
                        "the tabs inside are unaffected"
                    } else {
                        "the current tab moves into it"
                    })
                    .child(modal_inset(cx).child(Input::new(&self.input)))
                    .hint("\u{23ce}", "save")
                    .hint("esc", "cancel")
                    .on_dismiss(dismiss)
                    .into_any_element()
            }
            Overlay::Path {
                purpose,
                suggestions,
                create,
                cursor,
            } => {
                let theme = cx.theme();
                let (caption, dim, accent) = (
                    theme.type_scale().caption(),
                    theme.dim_foreground(),
                    theme.accent(),
                );
                // The create row's colour: not the accent (that is a place
                // that exists) and not urgent (nothing is wrong) — the
                // palette's yellow, for "not yet".
                let pending = omarchy_ui::color(theme.tokens.palette.yellow());
                let row_height = theme.space().control_height();

                let (title, subtitle, enter, arrows) = match purpose {
                    PathPurpose::GoTo => (
                        "Go to",
                        "an existing path completes to what is inside it".to_string(),
                        "go",
                        "pick",
                    ),
                    PathPurpose::MoveInto { name, .. } => (
                        "Move",
                        format!("\u{201c}{name}\u{201d} goes into the directory you pick"),
                        "move",
                        "pick",
                    ),
                    PathPurpose::CreateFile => (
                        "New",
                        "the full path of the file to create \u{2014} end it with / to make a directory instead"
                            .to_string(),
                        "create",
                        "complete",
                    ),
                };
                // In the maker a directory row completes the field; in the
                // others it is the answer.
                let completes = matches!(purpose, PathPurpose::CreateFile);

                let mut rows = suggestions
                    .iter()
                    .enumerate()
                    .map(|(i, path)| {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.to_string_lossy().into_owned());
                        let path = path.clone();
                        Row::new(("path-suggestion", i))
                            .cursor(*cursor == Some(i))
                            .focused(true)
                            .child(div().text_color(accent).child("\u{f07b} ")) // nf-fa-folder
                            .child(div().flex_1().min_w(px(0.)).overflow_hidden().child(name))
                            .on_click(cx.listener(move |this, _e, window, cx| {
                                if completes {
                                    this.complete_path_field(path.clone(), window, cx);
                                    return;
                                }
                                if let Some(Overlay::Path { cursor, .. }) = &mut this.overlay {
                                    *cursor = Some(i);
                                }
                                this.confirm_path(window, cx);
                            }))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>();

                if let Some(path) = create {
                    let index = suggestions.len();
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.to_string_lossy().into_owned());
                    rows.push(
                        Row::new("path-create")
                            .cursor(*cursor == Some(index))
                            .focused(true)
                            .child(div().text_color(pending).child("\u{f067} ")) // nf-fa-plus
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .overflow_hidden()
                                    .text_color(pending)
                                    .child(format!("create directory \u{201c}{name}\u{201d}")),
                            )
                            .on_click(cx.listener(move |this, _e, window, cx| {
                                if let Some(Overlay::Path { cursor, .. }) = &mut this.overlay {
                                    *cursor = Some(index);
                                }
                                this.confirm_path(window, cx);
                            }))
                            .into_any_element(),
                    );
                }

                let body = if rows.is_empty() {
                    modal_inset(cx)
                        .h(px(row_height))
                        .flex()
                        .items_center()
                        .text_size(px(caption))
                        .text_color(dim)
                        .child("no directory matches")
                        .into_any_element()
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .children(separated(rows))
                        .into_any_element()
                };

                Modal::new("path", title)
                    .subtitle(subtitle)
                    .child(modal_inset(cx).child(Input::new(&self.input)))
                    .child(body)
                    .hint("\u{23ce}", enter)
                    .hint("\u{2193}\u{2191}", arrows)
                    .hint("esc", "cancel")
                    .on_dismiss(dismiss)
                    .into_any_element()
            }
            Overlay::Palette { results, cursor } => {
                let theme = cx.theme();
                let (caption, dim) = (theme.type_scale().caption(), theme.dim_foreground());
                let row_height = theme.space().control_height();

                let rows = results
                    .iter()
                    .take(12)
                    .enumerate()
                    .map(|(i, &index)| {
                        let command = &COMMANDS[index];
                        // The hint shows the keymap's *effective* keys, so a
                        // rebound action reads correctly here even though the
                        // help sheet describes the defaults.
                        let keys = self.keymap.keys_for(command.action).join("  ");
                        Row::new(("command", i))
                            .cursor(i == *cursor)
                            .focused(true)
                            .child(div().flex_1().min_w(px(0.)).child(command.label))
                            .child(div().text_size(px(caption)).text_color(dim).child(keys))
                            .on_click(cx.listener(move |this, _e, window, cx| {
                                if let Some(Overlay::Palette { cursor, .. }) = &mut this.overlay {
                                    *cursor = i;
                                }
                                this.confirm_overlay(window, cx);
                            }))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>();

                let body = if results.is_empty() {
                    modal_inset(cx)
                        .h(px(row_height))
                        .flex()
                        .items_center()
                        .text_size(px(caption))
                        .text_color(dim)
                        .child("no command matches")
                        .into_any_element()
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .children(separated(rows))
                        .into_any_element()
                };

                Modal::new("palette", "Commands")
                    .child(modal_inset(cx).child(Input::new(&self.input)))
                    .child(body)
                    .hint("\u{23ce}", "run")
                    .hint("\u{2193}\u{2191}", "select")
                    .hint("esc", "close")
                    .on_dismiss(dismiss)
                    .into_any_element()
            }
            Overlay::Finder {
                root,
                query,
                recent,
                names,
                hits,
                scanning,
                searching,
                truncated_walk,
                truncated_grep,
                cursor,
            } => {
                let theme = cx.theme();
                let (caption, dim, bright) = (
                    theme.type_scale().caption(),
                    theme.dim_foreground(),
                    theme.bright_foreground(),
                );
                let row_height = theme.space().control_height();

                let section = |label: &'static str, cx: &mut Context<Explorer>| {
                    let theme = cx.theme();
                    div()
                        .px(px(theme.space().row_padding_x()))
                        .pt(px(theme.space().xs()))
                        .text_size(px(theme.type_scale().caption()))
                        .text_color(theme.dim_foreground())
                        .child(label.to_uppercase())
                        .into_any_element()
                };
                let status = |text: String, cx: &mut Context<Explorer>| {
                    modal_inset(cx)
                        .h(px(row_height))
                        .flex()
                        .items_center()
                        .text_size(px(caption))
                        .text_color(dim)
                        .child(text)
                        .into_any_element()
                };

                let mut children: Vec<AnyElement> = Vec::new();
                let mut rows: Vec<AnyElement> = Vec::new();

                if query.is_empty() {
                    // The empty query: the newest files below the root.
                    children.push(section("recent", cx));
                    if *scanning {
                        children.push(status("scanning\u{2026}".to_string(), cx));
                    } else if recent.is_empty() {
                        children.push(status("nothing below here".to_string(), cx));
                    }
                    for (i, file) in recent.iter().enumerate() {
                        let name = file
                            .path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let parent = file
                            .path
                            .parent()
                            .map(|p| middle_truncate(&display_path(p), 40))
                            .unwrap_or_default();
                        let age = format_age(file.modified);
                        rows.push(
                            Row::new(("finder-recent", i))
                                .cursor(i == *cursor)
                                .focused(true)
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .max_w_1_2()
                                        .overflow_hidden()
                                        .text_color(bright)
                                        .child(name),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .overflow_hidden()
                                        .text_size(px(caption))
                                        .text_color(dim)
                                        .child(parent),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_size(px(caption))
                                        .text_color(dim)
                                        .child(age),
                                )
                                .on_click(cx.listener(move |this, _e, window, cx| {
                                    if let Some(Overlay::Finder { cursor, .. }) = &mut this.overlay
                                    {
                                        *cursor = i;
                                    }
                                    this.confirm_overlay(window, cx);
                                }))
                                .into_any_element(),
                        );
                    }
                } else {
                    // Typed: names first, then contents — one cursor over both.
                    children.push(section("files", cx));
                    if *scanning {
                        children.push(status("indexing\u{2026}".to_string(), cx));
                    } else if names.is_empty() {
                        children.push(status("no name matches".to_string(), cx));
                    }
                    for (i, hit) in names.iter().enumerate() {
                        rows.push(
                            Row::new(("finder-name", i))
                                .cursor(i == *cursor)
                                .focused(true)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .overflow_hidden()
                                        .child(hit.label.clone()),
                                )
                                .on_click(cx.listener(move |this, _e, window, cx| {
                                    if let Some(Overlay::Finder { cursor, .. }) = &mut this.overlay
                                    {
                                        *cursor = i;
                                    }
                                    this.confirm_overlay(window, cx);
                                }))
                                .into_any_element(),
                        );
                    }
                    let contents = finder_content_rows(names, hits);
                    let offset = names.len();
                    // Only worth a header once there is, or may be, something
                    // in it: a two-section window where one says "no" twice
                    // over reads as failure.
                    if *searching || !contents.is_empty() {
                        rows.push(section("matching contents", cx));
                        if *searching {
                            rows.push(status("searching\u{2026}".to_string(), cx));
                        }
                    }
                    for (i, hit) in contents.iter().enumerate() {
                        let shown = hit
                            .path
                            .strip_prefix(root)
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_else(|_| hit.path.to_string_lossy().into_owned());
                        let index = offset + i;
                        rows.push(
                            Row::new(("finder-hit", i))
                                .cursor(index == *cursor)
                                .focused(true)
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .max_w_1_2()
                                        .overflow_hidden()
                                        .text_color(bright)
                                        .child(format!("{shown}:{}", hit.line)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .overflow_hidden()
                                        .text_size(px(caption))
                                        .text_color(dim)
                                        .child(hit.text.clone()),
                                )
                                .on_click(cx.listener(move |this, _e, window, cx| {
                                    if let Some(Overlay::Finder { cursor, .. }) = &mut this.overlay
                                    {
                                        *cursor = index;
                                    }
                                    this.confirm_overlay(window, cx);
                                }))
                                .into_any_element(),
                        );
                    }
                }

                children.push(
                    div()
                        .id("finder-scroll")
                        .flex()
                        .flex_col()
                        .max_h(px(row_height * 13.0))
                        .overflow_y_scroll()
                        .children(separated(rows))
                        .into_any_element(),
                );

                let mut notes: Vec<String> = Vec::new();
                if *truncated_walk {
                    notes.push(format!(
                        "index stopped at {}",
                        omafiles::search::RECURSIVE_LIMIT
                    ));
                }
                if *truncated_grep {
                    notes.push(format!("first {} content matches", grep::LIMIT));
                }

                Modal::new("finder", "Find")
                    .size(ModalSize::Large)
                    .subtitle(format!(
                        "below {}{}",
                        display_path(root),
                        if notes.is_empty() {
                            String::new()
                        } else {
                            format!(" \u{b7} {}", notes.join(" \u{b7} "))
                        }
                    ))
                    .child(modal_inset(cx).child(Input::new(&self.input)))
                    .child(div().flex().flex_col().children(children))
                    .hint("\u{23ce}", "go")
                    .hint("\u{2193}\u{2191}", "select")
                    .hint("esc", "close")
                    .on_dismiss(dismiss)
                    .into_any_element()
            }
            Overlay::Servers => {
                let theme = cx.theme();
                let (caption, dim, accent) = (
                    theme.type_scale().caption(),
                    theme.dim_foreground(),
                    theme.accent(),
                );
                let (gap, row_gap) = (theme.space().md(), theme.space().sm());
                let count = self.servers.len();

                let rows = self
                    .servers
                    .iter()
                    .enumerate()
                    .map(|(index, handle)| {
                        let url = handle.url();
                        let root = handle.root.clone();
                        // One line always: the path is middle-cut so its
                        // beginning and its end both survive.
                        let facts = format!(
                            "{} \u{b7} {} requests",
                            middle_truncate(&display_path(&root), 44),
                            handle.hits
                        );
                        modal_inset(cx)
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(gap))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .child(div().text_color(accent).child(url.clone()))
                                    .child(
                                        div()
                                            .text_size(px(caption))
                                            .text_color(dim)
                                            .overflow_hidden()
                                            .child(facts),
                                    ),
                            )
                            .child(
                                ActionButton::new(("srv-log", index))
                                    .glyph("\u{f022}") // nf-fa-list_alt
                                    .on_click(cx.listener({
                                        let root = root.clone();
                                        move |this, _e, window, cx| {
                                            this.open_server_log(root.clone(), window, cx);
                                        }
                                    })),
                            )
                            .child(
                                ActionButton::new(("srv-copy", index))
                                    .glyph("\u{f0c5}") // nf-fa-copy
                                    .on_click({
                                        let url = url.clone();
                                        move |_e, _w, cx| {
                                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                                url.clone(),
                                            ))
                                        }
                                    }),
                            )
                            .child(
                                ActionButton::new(("srv-go", index))
                                    .glyph("\u{f07b}") // nf-fa-folder
                                    .on_click(cx.listener(move |this, _e, window, cx| {
                                        let leaving = this.cursor_name();
                                        if let Some(tab) = this.session.active_tab_mut() {
                                            tab.navigation.go(root.clone(), leaving.as_deref());
                                        }
                                        this.dismiss_overlay(window, cx);
                                        this.reload(cx);
                                    })),
                            )
                            .child(
                                ActionButton::new(("srv-kill", index))
                                    .glyph("\u{f00d}") // nf-fa-times
                                    .on_click(cx.listener(move |this, _e, _w, cx| {
                                        this.stop_server_at(index, cx);
                                    })),
                            )
                            .into_any_element()
                    })
                    .collect::<Vec<_>>();

                let body = if rows.is_empty() {
                    modal_inset(cx)
                        .text_size(px(caption))
                        .text_color(dim)
                        .child("nothing serving \u{2014} ^s starts one in the current directory")
                        .into_any_element()
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(row_gap))
                        .children(separated(rows))
                        .into_any_element()
                };

                Modal::new("servers", "HTTP servers")
                    .subtitle(format!("{count} running"))
                    .child(body)
                    .hint("esc", "close")
                    .on_dismiss(dismiss)
                    .into_any_element()
            }
            Overlay::Context {
                path,
                name,
                is_dir,
                position,
            } => {
                let (path, name, is_dir, position) =
                    (path.clone(), name.clone(), *is_dir, *position);
                let pin = is_dir.then(|| self.pin_state(&path));
                let can_paste_into = is_dir && self.clipboard.is_some();

                // The verbs, in the order the toolbars use. Each dismisses
                // first: the action may open its own modal (the agent
                // prompt), and it must not find this one standing.
                let mut rows: Vec<AnyElement> = Vec::new();
                let item = |id: &'static str,
                            glyph: &'static str,
                            label: String,
                            enabled: bool,
                            act: ContextAction,
                            cx: &mut Context<Explorer>| {
                    let theme = cx.theme();
                    let (dim, faded) =
                        (theme.dim_foreground(), theme.dim_foreground().opacity(0.5));
                    let mut row = Row::new(id)
                        .child(
                            div()
                                .w(px(theme.type_scale().caption() * 1.6))
                                .flex_shrink_0()
                                .text_color(if enabled { dim } else { faded })
                                .child(glyph),
                        )
                        .child(div().flex_1().child(label));
                    if enabled {
                        row = row.on_click(cx.listener(move |this, _e, window, cx| {
                            this.dismiss_overlay(window, cx);
                            act(this, window, cx);
                        }));
                    }
                    row.into_any_element()
                };

                rows.push(item(
                    "ctx-open",
                    "\u{f08e}",
                    "Open".to_string(),
                    true,
                    Box::new(|this, _w, cx| this.open_selected(cx)),
                    cx,
                ));
                rows.push(item(
                    "ctx-agent",
                    "\u{f06a9}",
                    "Ask the agent".to_string(),
                    true,
                    Box::new(|this, window, cx| this.ask_agent(window, cx)),
                    cx,
                ));
                rows.push(item(
                    "ctx-share",
                    "\u{f1e0}",
                    "Share via LocalSend".to_string(),
                    true,
                    Box::new(|this, _w, cx| this.share_selected(cx)),
                    cx,
                ));
                rows.push(item(
                    "ctx-copy",
                    "\u{f0c5}",
                    "Copy".to_string(),
                    true,
                    Box::new(|this, window, cx| this.copy_selected(window, cx)),
                    cx,
                ));
                rows.push(item(
                    "ctx-cut",
                    "\u{f0c4}", // nf-fa-scissors
                    "Cut".to_string(),
                    true,
                    Box::new(|this, _w, cx| this.cut_selected(cx)),
                    cx,
                ));
                rows.push(item(
                    "ctx-path",
                    "\u{f0c1}", // nf-fa-link
                    "Copy path".to_string(),
                    true,
                    Box::new(|this, _w, cx| this.copy_path_selected(cx)),
                    cx,
                ));
                rows.push(item(
                    "ctx-move",
                    "\u{f061}", // nf-fa-arrow_right
                    "Move to\u{2026}".to_string(),
                    true,
                    Box::new(|this, window, cx| this.move_selected(window, cx)),
                    cx,
                ));
                if can_paste_into {
                    let dest = path.clone();
                    rows.push(item(
                        "ctx-paste",
                        "\u{f0ea}", // nf-fa-clipboard
                        "Paste into".to_string(),
                        true,
                        Box::new(move |this, _w, cx| this.paste_into(dest.clone(), cx)),
                        cx,
                    ));
                }
                rows.push(item(
                    "ctx-zip",
                    "\u{f1c6}", // nf-fa-file_archive
                    "Compress to zip".to_string(),
                    true,
                    Box::new(|this, _w, cx| this.compress_selected(cx)),
                    cx,
                ));
                rows.push(item(
                    "ctx-delete",
                    "\u{f1f8}", // nf-fa-trash
                    "Move to trash".to_string(),
                    true,
                    Box::new(|this, window, cx| this.delete_selected(window, cx)),
                    cx,
                ));
                if let Some(state) = pin {
                    let target = path.clone();
                    rows.push(item(
                        "ctx-pin",
                        "\u{f08d}",
                        if state == PinState::Pinned {
                            "Unpin".to_string()
                        } else {
                            "Pin".to_string()
                        },
                        state != PinState::System,
                        Box::new(move |this, _w, cx| this.toggle_pin(&target, cx)),
                        cx,
                    ));
                }

                menu_surround(name.clone(), rows, position, viewport, dismiss, cx)
            }
            Overlay::Server { root: menu_root } => {
                let menu_root = menu_root.clone();
                let theme = cx.theme();
                let (caption, dim, bright, urgent, accent) = (
                    theme.type_scale().caption(),
                    theme.dim_foreground(),
                    theme.bright_foreground(),
                    theme.urgent(),
                    theme.accent(),
                );
                let (row_gap, gap) = (theme.space().xs(), theme.space().md());

                let here = self.server_index_for(&menu_root);
                match here.and_then(|index| self.servers.get(index)) {
                    // Stopped: the two ways to start one, and nothing else.
                    // Loopback is the plain row; the network bind carries its
                    // warning in the row itself, because §6.7 says that
                    // exposure must never happen by accident.
                    None => {
                        let here = display_path(&self.current_path());
                        Modal::new("server", "HTTP server")
                            .subtitle(format!("serve {here}"))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        Row::new("serve-local")
                                            .child(div().flex_1().child("Start server"))
                                            .child(
                                                div()
                                                    .text_size(px(caption))
                                                    .text_color(dim)
                                                    .child("this machine only"),
                                            )
                                            .on_click(cx.listener(|this, _e, _w, cx| {
                                                this.start_server(false, cx)
                                            })),
                                    )
                                    .child(
                                        Row::new("serve-lan")
                                            .child(div().flex_1().child("Start on the network"))
                                            .child(
                                                div()
                                                    .text_size(px(caption))
                                                    .text_color(urgent)
                                                    .child("visible to everyone on it"),
                                            )
                                            .on_click(cx.listener(|this, _e, _w, cx| {
                                                this.start_server(true, cx)
                                            })),
                                    ),
                            )
                            .hint("esc", "close")
                            .on_dismiss(dismiss)
                            .into_any_element()
                    }
                    // Running: the facts, the log, and the way out.
                    Some(handle) => {
                        let url = handle.url();
                        let root = display_path(&handle.root);
                        let hits = handle.hits;
                        let log = server::read_log(handle.pid);
                        let recent = log.len().saturating_sub(12);

                        Modal::new("server", "Serving")
                            .subtitle(format!("{root} \u{2014} {hits} requests"))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(gap))
                                    .child(modal_inset(cx).text_color(accent).child(url.clone()))
                                    .child(if log.is_empty() {
                                        modal_inset(cx)
                                            .text_size(px(caption))
                                            .text_color(dim)
                                            .child("no requests yet")
                                            .into_any_element()
                                    } else {
                                        modal_inset(cx)
                                            .flex()
                                            .flex_col()
                                            .gap(px(row_gap))
                                            .text_size(px(caption))
                                            .text_color(dim)
                                            .children(
                                                log[recent..]
                                                    .iter()
                                                    .map(|line| div().child(line.clone())),
                                            )
                                            .into_any_element()
                                    })
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(
                                                Row::new("server-open")
                                                    .child(div().flex_1().child("Open in browser"))
                                                    .on_click({
                                                        let url = url.clone();
                                                        move |_e, _w, cx| cx.open_url(&url)
                                                    }),
                                            )
                                            .child(Separator::horizontal().subtle())
                                            .child(
                                                Row::new("server-copy")
                                                    .child(div().flex_1().child("Copy URL"))
                                                    .on_click({
                                                        let url = url.clone();
                                                        move |_e, _w, cx| {
                                                            cx.write_to_clipboard(
                                                                gpui::ClipboardItem::new_string(
                                                                    url.clone(),
                                                                ),
                                                            )
                                                        }
                                                    }),
                                            )
                                            .child(Separator::horizontal().subtle())
                                            .child(
                                                Row::new("server-stop")
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .text_color(bright)
                                                            .child("Stop server"),
                                                    )
                                                    .on_click(cx.listener(
                                                        move |this, _e, _w, cx| {
                                                            let root = match &this.overlay {
                                                                Some(Overlay::Server { root }) => {
                                                                    root.clone()
                                                                }
                                                                _ => return,
                                                            };
                                                            if let Some(index) =
                                                                this.server_index_for(&root)
                                                            {
                                                                this.stop_server_at(index, cx);
                                                            }
                                                        },
                                                    )),
                                            ),
                                    ),
                            )
                            .hint("esc", "close")
                            .on_dismiss(dismiss)
                            .into_any_element()
                    }
                }
            }
            Overlay::Agent { cwd, agent } => {
                // The subtitle is the contract: which agent, and where it will
                // sit. Enter should never launch something the dialog did not
                // describe.
                Modal::new("agent-prompt", "Ask the agent")
                    .subtitle(format!("launches {agent} in {}", display_path(cwd)))
                    .child(modal_inset(cx).child(Input::new(&self.input)))
                    .hint("\u{23ce}", "launch")
                    .hint("esc", "cancel")
                    .on_dismiss(dismiss)
                    .into_any_element()
            }
            Overlay::Branches {
                results,
                cursor,
                current,
                error,
                all,
            } => {
                let theme = cx.theme();
                let caption = theme.type_scale().caption();
                let (dim, urgent) = (theme.dim_foreground(), theme.urgent());
                let row_height = theme.space().control_height();
                let gap = theme.space().sm();

                let rows = results
                    .iter()
                    .take(60)
                    .enumerate()
                    .map(|(i, branch)| {
                        Row::new(("branch", i))
                            .cursor(i == *cursor)
                            .focused(true)
                            // The branch we are on stays marked while the cursor
                            // moves off it — otherwise the list gives no answer
                            // to "which one am I on".
                            .selected(current.as_deref() == Some(branch.as_str()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .overflow_hidden()
                                    .child(branch.clone()),
                            )
                            .into_any_element()
                    })
                    .collect::<Vec<_>>();

                let body = if results.is_empty() {
                    modal_inset(cx)
                        .h(px(row_height))
                        .flex()
                        .items_center()
                        .text_size(px(caption))
                        .text_color(dim)
                        .child(if all.is_empty() {
                            "no branches yet"
                        } else {
                            "no branch matches"
                        })
                        .into_any_element()
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .max_h(px(row_height * 10.0))
                        .overflow_hidden()
                        .children(separated(rows))
                        .into_any_element()
                };

                Modal::new("branches", "Switch branch")
                    .subtitle(format!("{} local", all.len()))
                    .child(modal_inset(cx).child(Input::new(&self.input)))
                    .child(body)
                    // git's refusal, verbatim and in full. It is the reason the
                    // switch shells out at all, so paraphrasing or truncating it
                    // here would throw away the point.
                    .children(error.as_ref().map(|message| {
                        modal_inset(cx)
                            .flex()
                            .flex_col()
                            .gap(px(gap))
                            .text_size(px(caption))
                            .text_color(urgent)
                            .child("git refused, and nothing was changed:")
                            .child(div().text_color(dim).child(message.clone()))
                    }))
                    .hint("\u{23ce}", "switch")
                    .hint("\u{2193}\u{2191}", "select")
                    .hint("esc", "close")
                    .on_dismiss(dismiss)
                    .into_any_element()
            }
        };
        Some(modal)
    }

    /// The status bar: what is in this directory, and the way in to everything
    /// else. One affordance rather than a wall of hints — the full list is a
    /// keystroke away and does not cost permanent screen space.
    /// The preview body in place of the listing column and the panel (§6.5).
    ///
    /// The body and nothing else: the title and the facts stay behind with the
    /// panel they belong to. Not an overlay either — it is the centre pane
    /// rendered differently, so the sidebar stays put and stays usable, and it
    /// never takes focus, which is what leaves `j`/`k` moving the cursor
    /// underneath. Flicking through a folder of images is most of what this is
    /// for.
    fn expanded_pane(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (caption, dim, small_gap, gap) = (
            theme.type_scale().caption(),
            theme.dim_foreground(),
            theme.space().sm(),
            theme.space().xl(),
        );
        // Text-like bodies sit on the sunken ground, and the *pane* paints it
        // so it reaches the bottom edge even when the file does not. On the
        // scrolled child it would either stop at the last line or need a
        // min-height entangled with the scroll measurement; on the container
        // it is just paint.
        let ground = self
            .preview
            .as_ref()
            .filter(|loaded| {
                matches!(
                    loaded.preview.body,
                    Body::Text { .. } | Body::Binary { .. } | Body::Diff(_)
                )
            })
            .map(|_| theme.sunken());

        // The body alone — no title, no fact table. Those belong to the panel,
        // which is a description of a file; this is the file. Expanding to see
        // a picture larger should not also enlarge the words next to it.
        let content = match &self.preview {
            Some(loaded) => render_body(loaded, Target::Expanded, cx),
            None => div()
                .text_size(px(caption))
                .text_color(dim)
                .child("reading\u{2026}")
                .into_any_element(),
        };

        // The one fact that survives, and it goes in the footer rather than over
        // the body: with the title gone and the listing replaced, nothing on
        // screen would say which file `j`/`k` had landed on.
        let name = self
            .preview
            .as_ref()
            .map(|l| l.preview.name.clone())
            .unwrap_or_default();

        div()
            .id("preview-expanded")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .child(
                // The instruction bar rides on top (on request): the file's
                // name, the keys, and the way back — then the rule, then the
                // body filling everything below.
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(gap))
                    .p(px(small_gap))
                    .text_size(px(caption))
                    .text_color(dim)
                    // The name yields first: a long one must not push the way
                    // out off the edge of the window.
                    .child(div().flex_1().min_w(px(0.)).overflow_hidden().child(name))
                    .child("j / k  next \u{00b7} previous")
                    // From the keymap, not a literal: rebinding space (M11)
                    // must not leave this hint promising a key that no longer
                    // does it.
                    .child({
                        let mut keys = vec!["esc".to_string()];
                        keys.extend(self.keymap.keys_for("toggle_preview"));
                        format!("{}  collapse", keys.join(" / "))
                    })
                    .child(
                        ActionButton::new("preview-close")
                            .glyph("\u{f00d}") // nf-fa-times
                            .label("Close")
                            .on_click(cx.listener(|this, _e, _w, cx| this.toggle_preview(cx))),
                    ),
            )
            .child(Separator::horizontal())
            .child(
                // The body fills the area — no margin, no cap.
                {
                    let mut scroll = div()
                        .id("preview-expanded-scroll")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.))
                        .overflow_y_scroll();
                    if let Some(ground) = ground {
                        scroll = scroll.bg(ground);
                    }
                    scroll.child(content)
                },
            )
            .into_any_element()
    }

    fn status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let (dirs, files) = self
            .listing()
            .map(|l| l.counts(self.show_hidden))
            .unwrap_or((0, 0));
        // Copied out rather than held: `git_bar` needs the context back, and a
        // live `cx.theme()` borrow is what stops it.
        let (space, caption, dim, urgent) = {
            let theme = cx.theme();
            (
                theme.space().clone(),
                theme.type_scale().caption(),
                theme.dim_foreground(),
                theme.urgent(),
            )
        };
        let git = self.git_bar(cx);
        let marked = self.selected_count();
        let compact = !self.config.button_labels;

        // One value everywhere (on request): the same small inset on every
        // side and between items, so the bar's rhythm is a single number —
        // except on the left, where the summary gets the panel inset so it
        // does not hug the window edge (on request).
        let inset = space.sm();
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(inset))
            .p(px(inset))
            .pl(px(space.panel_padding()))
            .text_size(px(caption))
            .text_color(dim)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(inset))
                    .child(div().child(format!("{dirs} directories · {files} files")))
                    .children((marked > 0).then(|| div().child(format!("{marked} selected"))))
                    .children(git)
                    // What the last action had to say for itself (M9). One
                    // line, urgent, gone again in a few seconds.
                    .children(self.notice.as_ref().map(|(message, is_urgent)| {
                        div()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .text_color(if *is_urgent { urgent } else { dim })
                            .child(message.clone())
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(inset))
                    .children(self.show_hidden.then(|| div().child("hidden shown")))
                    // The directory's actions, for the mouse (M9). The same
                    // verbs as `t`/`a`/`s` plus `^p`, but scoped to the
                    // directory being looked at rather than the entry under
                    // the cursor — the cursor entry's actions sit in the
                    // detail panel beside its facts. The server badge rides
                    // with them: it is the directory-scoped action whose
                    // label happens to also be its state.
                    .child(self.server_badge(cx))
                    .child({
                        let path = self.current_path();
                        let state = self.pin_state(&path);
                        pin_button("act-pin", path, state, compact, cx)
                    })
                    .child(
                        ActionButton::new("act-terminal")
                            .glyph("\u{f120}") // nf-fa-terminal
                            .label("Terminal")
                            .compact(compact)
                            .on_click(cx.listener(|this, _e, _w, cx| this.open_terminal_here(cx))),
                    )
                    .child(
                        ActionButton::new("act-agent")
                            .glyph("\u{f06a9}") // nf-md-robot
                            .label("Agent")
                            .compact(compact)
                            .on_click(
                                cx.listener(|this, _e, window, cx| this.ask_agent_here(window, cx)),
                            ),
                    )
                    .child(
                        ActionButton::new("act-share")
                            .glyph("\u{f1e0}") // nf-fa-share_alt
                            .label("Share")
                            .compact(compact)
                            .on_click(cx.listener(|this, _e, _w, cx| this.share_here(cx))),
                    )
                    .child(
                        ActionButton::new("act-new")
                            .glyph("\u{f067}") // nf-fa-plus
                            .label("New")
                            .compact(compact)
                            .on_click(cx.listener(|this, _e, window, cx| {
                                this.create_file_here(window, cx)
                            })),
                    )
                    .child(
                        ActionButton::new("help")
                            .glyph("?")
                            .label("Help")
                            .compact(compact)
                            .on_click(
                                cx.listener(|this, _e, window, cx| this.show_help(window, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// The HTTP server's status, as a button (M10).
    ///
    /// Lives with the action cluster in the status bar's right corner: it is
    /// an action whose label happens to also be its state. Stopped it reads `http off`,
    /// dimly; serving it carries the accent and the port — and the served
    /// path when that is not the directory being looked at, because a badge
    /// reading `:8080` next to the *wrong* directory would imply it serves
    /// this one. Clicking opens the contextual menu either way.
    fn server_badge(&self, cx: &mut Context<Self>) -> AnyElement {
        let here = self.server_index_for(&self.current_path());
        let label = match here.and_then(|index| self.servers.get(index)) {
            None => "http off".to_string(),
            Some(handle) => format!(":{}", handle.port),
        };
        ActionButton::new("server")
            .glyph("\u{f0ac}") // nf-fa-globe
            .label(label)
            .accent(here.is_some())
            .on_click(cx.listener(|this, _e, window, cx| this.open_server_menu(window, cx)))
            .into_any_element()
    }

    /// The current directory's repository, in the status bar.
    ///
    /// §6.9 put the branch in the header. It lives here instead — the header is
    /// the *path*, and the branch is a fact about where you are rather than part
    /// of the address, so it belongs beside the directory summary and not in
    /// competition with the breadcrumb for width.
    ///
    /// Outside a repository this renders nothing at all. Most directories are
    /// not in one, and an empty branch label reads as a bug.
    fn git_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let git = self.git.as_ref()?;
        let label = git.head.label();
        // Empty until the background status lands, which is what keeps the
        // branch on screen immediately rather than waiting on the slow half.
        let counts = git
            .status
            .as_ref()
            .map(|status| status.counts.summary())
            .unwrap_or_default();

        // Everything the theme is needed for, resolved before `cx.listener`
        // wants the context back.
        let (bright, caption, marks) = {
            let theme = cx.theme();
            let marks: Vec<(String, gpui::Hsla)> = counts
                .into_iter()
                .map(|(state, count)| {
                    (
                        format!("{}{count}", state.marker()),
                        // From the palette by role, never a literal: a marker
                        // has to retint with the rest of the window.
                        omarchy_ui::color(theme.tokens.palette.get(state.role())),
                    )
                })
                .collect();
            (
                theme.bright_foreground(),
                theme.type_scale().caption(),
                marks,
            )
        };

        // The branch and the counts ride as children rather than the label:
        // the branch is *information* and keeps its emphasis, and each count
        // keeps its own palette role — an `ActionButton` label would flatten
        // both to the uniform secondary colour.
        Some(
            ActionButton::new("git")
                .glyph("\u{e0a0}") // nf-pl-branch
                .child(div().text_size(px(caption)).text_color(bright).child(label))
                .children(marks.into_iter().map(|(text, colour)| {
                    div().text_size(px(caption)).text_color(colour).child(text)
                }))
                .on_click(cx.listener(|this, _e, window, cx| this.open_branches(window, cx)))
                .into_any_element(),
        )
    }
}
/// One sidebar row.
///
/// No "you are here" highlight (revised on request): the tab list already
/// says where you are, and a place click now selects or opens a tab rather
/// than navigating one, so a lit place would repeat the tab row above it.
fn place_row(
    index: usize,
    place: &Place,
    is_cursor: bool,
    pane_focused: bool,
    cx: &mut App,
) -> Row {
    let theme = cx.theme();
    let caption = theme.type_scale().caption();
    let dim = theme.dim_foreground_on(theme.tokens.palette.lighter_background());

    let glyph = match place.origin {
        Origin::Home => "\u{f015}",   // nf-fa-home
        Origin::Config => "\u{f013}", // nf-fa-cog
        Origin::Xdg => "\u{f07b}",    // nf-fa-folder
        Origin::Pinned => "\u{f08d}", // nf-fa-thumb_tack
    };

    Row::new(("place", index))
        .cursor(is_cursor)
        .focused(pane_focused)
        .child(
            div()
                .w(px(caption * 1.6))
                .flex_shrink_0()
                .text_color(dim)
                .child(glyph),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .child(place.label.clone()),
        )
}

/// A row that is an affordance rather than an item — "New tab", "New
/// workspace": faint until the pointer is on it, so it reads as a place to
/// click and not as one more entry in the list above.
fn quiet_row(
    id: impl Into<gpui::ElementId>,
    glyph: &'static str,
    label: &'static str,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    cx: &mut Context<Explorer>,
) -> AnyElement {
    let caption = cx.theme().type_scale().caption();
    div()
        .opacity(0.3)
        .hover(|style| style.opacity(1.0))
        .child(
            Row::new(id)
                .focused(true)
                .child(
                    div()
                        .w(px(caption * ICON_COLUMN))
                        .flex_shrink_0()
                        .child(glyph),
                )
                .child(div().flex_1().min_w(px(0.)).truncate().child(label))
                .on_click(on_click),
        )
        .into_any_element()
}

/// The TABS section header, with the borderless "new workspace" button.
///
/// A grid with a plus, not the bare plus the "New tab" rows use: the two
/// verbs sit a few rows apart and must not read as the same one. Borderless
/// like a tab row's `×`, not an `ActionButton`: in a section header a
/// hairline outline reads as a form control.
fn tabs_header(cx: &mut Context<Explorer>) -> AnyElement {
    let theme = cx.theme();
    let pad_x = theme.space().row_padding_x();
    let dim = theme.dim_foreground_on(theme.tokens.palette.lighter_background());
    div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .pr(px(pad_x))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .child(SectionHeader::new("tabs")),
        )
        .child(quiet_button(
            "ws-new",
            "\u{f11da}", // nf-md-view_grid_plus_outline
            dim,
            cx.listener(|this, _e, window, cx| this.open_workspace_prompt(None, window, cx)),
            cx,
        ))
        .into_any_element()
}

/// A workspace header: its name, a drop target, and its actions.
///
/// The header is the drop target rather than the group's rows, because it stays
/// hittable when the group is empty — which is exactly when you most want to
/// drag something into it.
fn drop_header(
    workspace: usize,
    label: String,
    collapsed: bool,
    active: bool,
    cx: &mut Context<Explorer>,
) -> AnyElement {
    let theme = cx.theme();
    let space = theme.space();
    let (pad_x, gap, chevron_gap) = (space.row_padding_x(), space.xs(), space.md());
    let (pad_top, pad_bottom, caption) = (space.sm(), space.xxs(), theme.type_scale().caption());
    let dim = theme.dim_foreground_on(theme.tokens.palette.lighter_background());
    let bright = theme.bright_foreground();
    // A workspace holding the active tab is where you are, and says so the
    // way a place row does: in the plain foreground, not the dim one.
    let text = if active { theme.foreground() } else { dim };

    div()
        .id(("ws-drop", workspace))
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .pr(px(pad_x))
        .drag_over::<DraggedTab>(|style, _dragged, _window, cx| {
            style.bg(cx.theme().selected_fill())
        })
        .on_drop(cx.listener(move |this, dragged: &DraggedTab, _window, cx| {
            this.tab_drop = None;
            if this
                .session
                .move_tab(dragged.workspace, dragged.index, workspace)
            {
                this.after_tab_change(cx);
            }
            cx.notify();
        }))
        // The name is the collapse toggle: a chevron says which way it will
        // go, and a collapsed group keeps its header so the tabs inside are
        // one click from coming back. The state persists with the session.
        .child(
            div()
                .id(("ws-toggle", workspace))
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
                .hover(|style| style.text_color(bright))
                .on_click(cx.listener(move |this, _e, _w, cx| {
                    this.toggle_workspace_collapsed(workspace, cx);
                }))
                .child(div().flex_shrink_0().child(if collapsed {
                    "\u{f054}" // nf-fa-chevron_right
                } else {
                    "\u{f078}" // nf-fa-chevron_down
                }))
                // Plain case: a workspace is named by the user, unlike the
                // fixed section headers above it.
                .child(div().min_w(px(0.)).overflow_hidden().child(label)),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(gap))
                .child(quiet_button(
                    ("ws-menu", workspace),
                    "\u{f141}", // nf-fa-ellipsis_h
                    text,
                    cx.listener(move |this, _e, window, cx| {
                        this.overlay = Some(Overlay::WorkspaceMenu { workspace });
                        let owner = this.focus_handle_for(this.pane).clone();
                        window.focus(&owner, cx);
                        cx.notify();
                    }),
                    cx,
                )),
        )
        .into_any_element()
}

/// A borderless glyph button for a sidebar header — like a tab row's `×`,
/// not an `ActionButton`: in a header a hairline outline reads as a form
/// control.
fn quiet_button(
    id: impl Into<gpui::ElementId>,
    glyph: &'static str,
    color: gpui::Hsla,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    cx: &mut Context<Explorer>,
) -> AnyElement {
    let theme = cx.theme();
    let caption = theme.type_scale().caption();
    let (hover_fill, bright, radius) = (
        theme.hover_fill(),
        theme.bright_foreground(),
        theme.radius(),
    );
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .w(px(caption * ICON_COLUMN))
        .h(px(caption * ICON_COLUMN))
        .rounded(px(radius.min(2.0)))
        .text_size(px(caption))
        .text_color(color)
        .hover(|style| style.bg(hover_fill).text_color(bright))
        .on_click(on_click)
        .child(glyph)
        .into_any_element()
}

/// Where a dragged tab would be inserted: workspace, position in its tab
/// list (`tabs.len()` for "after the last"), and the bounds of the row the
/// pointer is over — kept so the sidebar can tell when the pointer has left
/// it, since gpui reports drag moves everywhere and drag leaves nowhere.
#[derive(Clone, Copy, Debug)]
struct TabDrop {
    workspace: usize,
    index: usize,
    bounds: Bounds<Pixels>,
}

/// The payload of a tab drag: where it came from.
///
/// A struct rather than a tuple so gpui's type-keyed drop routing cannot
/// confuse it with any other draggable thing added later.
#[derive(Clone, Debug)]
struct DraggedTab {
    workspace: usize,
    index: usize,
}

/// One tab row in the sidebar.
///
/// Clicking activates it; dragging moves it between workspaces (see the drop
/// targets on the section headers). `on_close` is the small `×` on the row's
/// right, revealed on hover — `None` for the last tab anywhere, which the
/// session refuses to close (§ `Session::close_tab`), so no dead control is
/// offered.
fn tab_row(
    workspace: usize,
    index: usize,
    tab: &Tab,
    active: bool,
    on_close: Option<impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static>,
    cx: &mut App,
) -> Row {
    let theme = cx.theme();
    let caption = theme.type_scale().caption();
    let surface = theme.tokens.palette.lighter_background();
    let dim = theme.dim_foreground_on(surface);
    let accent = theme.accent();
    let (hover_fill, bright, radius) = (
        theme.hover_fill(),
        theme.bright_foreground(),
        theme.radius(),
    );

    let label = tab.label();
    let mut row = Row::new(("tab", workspace * 1000 + index))
        .selected(active)
        .focused(true);
    if on_close.is_some() {
        // The close box carries its own inset; the row's would double it.
        row = row.padding_right(theme.space().sm());
    }
    row.draggable(DraggedTab { workspace, index }, {
        let label = label.clone();
        move |_payload, _position, _window, cx: &mut App| drag_preview(label.clone(), cx)
    })
    .child(
        div()
            .w(px(caption * 1.6))
            .flex_shrink_0()
            .text_color(if active { accent } else { dim })
            .child("\u{f114}"), // nf-fa-folder_o
    )
    .child(div().flex_1().min_w(px(0.)).truncate().child(label))
    .children(on_close.map(|on_close| {
        // Borderless on purpose, unlike an `ActionButton`: inside a row a
        // hairline outline reads as a form control. Invisible rather than
        // absent when the pointer is elsewhere, so revealing it never
        // shifts the label.
        div()
            .id("close")
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .w(px(caption * 1.6))
            .h(px(caption * 1.6))
            .rounded(px(radius.min(2.0)))
            .text_size(px(caption))
            .text_color(dim)
            .invisible()
            .group_hover(omarchy_ui::ROW_GROUP, |style| style.visible())
            .hover(|style| style.bg(hover_fill).text_color(bright))
            .on_click(move |event, window, cx| {
                // The row underneath activates on click; closing must not
                // also switch to the tab being removed.
                cx.stop_propagation();
                on_close(event, window, cx);
            })
            .child("\u{f00d}") // nf-fa-times
    }))
}

/// The payload of an entry drag: the files being carried.
///
/// Its own struct, like [`DraggedTab`], because gpui routes drops by type.
/// The items sit behind an `Arc`: every marked row offers the same set, and
/// the listing rebuilds its rows each frame, so the set is built once per
/// frame and shared rather than cloned per row.
#[derive(Clone, Debug)]
struct DraggedEntries {
    items: Arc<Vec<DragItem>>,
}

#[derive(Clone, Debug)]
struct DragItem {
    path: PathBuf,
    name: String,
    is_dir: bool,
}

impl DraggedEntries {
    /// What the drag preview and the notices call the load.
    fn label(&self) -> String {
        match self.items.as_slice() {
            [one] => format!("\u{201c}{}\u{201d}", one.name),
            many => format!("{} items", many.len()),
        }
    }
}

/// Why `items` cannot be dropped into `dest` — empty when they can.
///
/// Checked before anything moves, and all of them at once, so the refusal
/// names every problem rather than the first: a drop of five files that
/// stops at the third has done something, and nobody asked for that.
fn drop_refusals(items: &[DragItem], dest: &Path) -> Vec<String> {
    let mut reasons = Vec::new();
    if !dest.is_dir() {
        reasons.push(format!("{} is not a directory", display_path(dest)));
    }
    for item in items {
        let name = format!("\u{201c}{}\u{201d}", item.name);
        // `symlink_metadata`, not `exists`: a dangling link is still a thing
        // that can be moved.
        if item.path.symlink_metadata().is_err() {
            reasons.push(format!("{name} is no longer there"));
        } else if item.path.parent() == Some(dest) {
            reasons.push(format!("{name} is already there"));
        } else if item.is_dir && dest.starts_with(&item.path) {
            reasons.push(format!("{name} cannot be moved into itself"));
        }
    }
    reasons
}

/// The highlight a row takes while a dragged set of entries is over it: the
/// same fill a workspace header shows for a tab, so "you can drop here"
/// reads the same everywhere.
fn drop_highlight(
    style: StyleRefinement,
    _dragged: &DraggedEntries,
    _window: &mut Window,
    cx: &mut App,
) -> StyleRefinement {
    style.bg(cx.theme().selected_fill())
}

/// The floating label for a drag. Detached from the row, so it needs its own
/// background — otherwise it reads as text floating over the UI.
fn drag_preview(label: String, cx: &mut App) -> Entity<DragPreview> {
    let theme = cx.theme();
    let preview = DragPreview {
        label,
        background: theme.surface(),
        border: theme.border(),
        text: theme.bright_foreground(),
        radius: theme.radius(),
        padding: theme.space().row_padding_x(),
        height: theme.space().control_height(),
    };
    cx.new(|_| preview)
}

/// What follows the pointer while something is being dragged.
struct DragPreview {
    label: String,
    background: gpui::Hsla,
    border: gpui::Hsla,
    text: gpui::Hsla,
    radius: f32,
    padding: f32,
    height: f32,
}

impl Render for DragPreview {
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

/// Width of the listing's size column until the user drags it; a dragged
/// width is kept per directory in `views.toml`.
///
/// Read through `Explorer::column_widths` by both the header and the rows
/// rather than written at two call sites: the two have to agree to the pixel
/// or the labels stop sitting over the values they name, and that is the
/// kind of drift nobody sees in a diff.
const SIZE_COLUMN: f32 = 72.;

/// Width of the listing's age column. See [`SIZE_COLUMN`].
const AGE_COLUMN: f32 = 48.;

/// The icon column, as a multiple of the caption size — so it scales with
/// `omarchy display text size` like everything else.
const ICON_COLUMN: f32 = 1.6;

/// One listing row.
///
/// `git` is what the repository says about this entry, if the status has landed
/// and it has anything to say. It rides on the icon rather than taking a column
/// of its own: a marker column would cost width in every directory, and most
/// directories are not in a repository at all.
///
/// `columns` is the `(size, age)` width pair the header was drawn with.
fn entry_row(
    position: usize,
    entry: &Entry,
    is_cursor: bool,
    is_selected: bool,
    git: Option<git::State>,
    columns: (f32, f32),
    cx: &mut App,
) -> Row {
    let (size_width, age_width) = columns;
    let theme = cx.theme();
    let caption = theme.type_scale().caption();
    let dim = theme.dim_foreground();
    let accent = theme.accent();
    // By palette role, so the markers retint with the theme and no green or red
    // is ever written down (§6.9).
    let marker = git.map(|state| {
        (
            state.marker(),
            omarchy_ui::color(theme.tokens.palette.get(state.role())),
        )
    });

    let glyph = match entry.kind {
        Kind::Directory => "\u{f07b}",  // nf-fa-folder
        Kind::File => "\u{f15b}",       // nf-fa-file
        Kind::Unresolved => "\u{f127}", // nf-fa-chain_broken
    };

    let size = entry
        .size
        .map(format_size)
        .unwrap_or_else(|| "—".to_string());
    let age = entry.modified.map(format_age).unwrap_or_default();

    Row::new(("entry", position))
        .cursor(is_cursor)
        .selected(is_selected)
        .focused(true)
        .child(
            div()
                .relative()
                .w(px(caption * ICON_COLUMN))
                .flex_shrink_0()
                // Accent on the cursor row only, not on every directory. §5's
                // rule is that accent is scarce — one element per view — and
                // spending it on the row you are standing on says more than
                // spending it on half the listing. The folder and file glyphs
                // already differ, so nothing is lost by letting them share a
                // colour.
                .text_color(if is_cursor { accent } else { dim })
                .child(glyph)
                // Composited onto the icon's corner rather than replacing it:
                // the icon says what the entry is and the badge says what git
                // thinks of it, and both are worth knowing at once. It sits just
                // outside the glyph box, in the gap before the name, so it never
                // lands on top of a descender.
                .children(marker.map(|(glyph, colour)| {
                    div()
                        .absolute()
                        .right(px(-2.))
                        .bottom(px(-2.))
                        .text_size(px(caption * 0.85))
                        .text_color(colour)
                        .child(glyph)
                })),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .child(entry.name.clone()),
        )
        .children(entry.is_symlink.then(|| {
            div()
                .flex_shrink_0()
                .text_size(px(caption))
                .text_color(dim)
                .child("\u{f0c1}") // nf-fa-link
        }))
        .child(
            div()
                .w(px(size_width))
                .flex_shrink_0()
                .truncate()
                .text_size(px(caption))
                .text_color(dim)
                .child(size),
        )
        .child(
            div()
                .w(px(age_width))
                .flex_shrink_0()
                .truncate()
                .text_size(px(caption))
                .text_color(dim)
                .child(age),
        )
}

// -------------------------------------------------------------- the preview
//
// §6.5 asks for one renderer used at two sizes rather than a pane version and a
// fullscreen version, because two would drift and the second would be the one
// nobody notices is wrong. So `render_body` takes a `Target` and branches on it
// *inside* one arm per body kind, never by having two functions. What differs
// between the two sites is only what surrounds the body: the panel sets a title
// and a fact table above it, the expanded view shows it alone.

/// Lines of a text file shown in the detail panel.
///
/// The panel is a column a few hundred pixels wide; more than this is scrolling
/// nobody does there, and every line costs shaping. The expanded view shows the
/// file up to `preview::MAX_LINES`.
const PANE_LINES: usize = 120;

/// Hex rows in the panel. Expanded shows all of `preview::HEX_BYTES`.
const PANE_HEX_ROWS: usize = 12;

/// Bytes per hex row in the pane.
///
/// Measured, not guessed: the pane is a `dropdown_width` column, which fits
/// about 30 characters of caption type. A full 16-byte row is 56 characters and
/// an 8-byte row is 43; both wrap, and a wrapped hex row loses the column
/// alignment that is the only reason to show hex at all. Four bytes is 27 and
/// keeps the format intact — same offsets, same ascii column, just narrower.
const PANE_HEX_BYTES: usize = 4;

/// The panel's preview: a title, the facts, and the body under them.
///
/// Only the panel composes all three. The expanded view renders
/// [`render_body`] on its own, so the shared piece — and the only piece that
/// takes a [`Target`] — is the body.
fn render_info(loaded: &Loaded, cx: &mut App) -> AnyElement {
    let theme = cx.theme();
    let (subtitle, caption, dim, bright, gap, small_gap) = (
        theme.type_scale().subtitle(),
        theme.type_scale().caption(),
        theme.dim_foreground(),
        theme.bright_foreground(),
        theme.space().md(),
        theme.space().xs(),
    );

    let facts = preview_facts(loaded);

    div()
        .flex()
        .flex_col()
        .gap(px(small_gap))
        .min_h(px(0.))
        .child(
            div()
                .text_size(px(subtitle))
                .text_color(bright)
                .child(loaded.preview.name.clone()),
        )
        .children(facts.into_iter().map(|(label, value)| {
            div()
                .flex()
                .flex_row()
                .justify_between()
                .gap(px(gap))
                .text_size(px(caption))
                .child(div().text_color(dim).child(label))
                .child(div().child(value))
        }))
        .into_any_element()
}

/// The fact table under the title. Kind and size always; the rest per body.
fn preview_facts(loaded: &Loaded) -> Vec<(String, String)> {
    let preview = &loaded.preview;
    let mut facts = vec![("kind".to_string(), preview.body.label().to_string())];

    if preview.is_symlink {
        facts.push(("link".to_string(), "symlink".to_string()));
    }
    if let Some(size) = preview.size {
        facts.push(("size".to_string(), format_size(size)));
    }
    if let Some(modified) = preview.key.mtime {
        facts.push(("modified".to_string(), format_age(modified)));
    }

    match &preview.body {
        Body::Directory { entries: Some(n) } => {
            facts.push(("entries".to_string(), n.to_string()));
        }
        Body::Image {
            dimensions: Some((w, h)),
            ..
        } => {
            facts.push(("dimensions".to_string(), format!("{w} × {h}")));
        }
        Body::Text {
            lines, truncated, ..
        } => {
            // Saying "4000 lines" of a file that has 90,000 would be a lie, so
            // the cap is stated rather than hidden.
            let value = if *truncated {
                format!("first {lines}")
            } else {
                lines.to_string()
            };
            facts.push(("lines".to_string(), value));
        }
        Body::Diff(diff) => {
            facts.push((
                "hunks".to_string(),
                if diff.truncated {
                    format!("first {}", diff.hunks.len())
                } else {
                    diff.hunks.len().to_string()
                },
            ));
            facts.push((
                "changed".to_string(),
                format!("+{} \u{2212}{}", diff.added, diff.removed),
            ));
        }
        Body::Video { facts: probed, .. } => facts.extend(probed.iter().cloned()),
        Body::TooLarge { limit, .. } => {
            facts.push(("limit".to_string(), format_size(*limit)));
        }
        _ => {}
    }
    facts
}

/// The panel cover's height cap, in logical pixels.
const COVER_HEIGHT: f32 = 250.;

/// A picture laid out at its own shape, so no dead space frames it.
///
/// gpui's `img` takes the picture's natural pixel size when given none, and a
/// `max_w` then shrinks the width alone — the box keeps its natural height and
/// the picture floats in it with a band above and below. So the box is worked
/// out here instead. In the panel the width is known (the cover is flush to
/// the panel), so the box is exact: the cover's width, or the height cap with
/// the width to match. Expanded, the picture is contained in the whole pane,
/// centred, the way a viewer shows one. A poster keeps a width-bound box with
/// an aspect ratio, so the facts can follow underneath.
#[derive(Clone, Copy)]
enum ImageFit {
    /// The panel cover, at a known width.
    Cover { width: f32 },
    /// The whole expanded pane.
    Pane,
    /// One item in an expanded column of other things — a video's poster
    /// above its facts: width-bound, never taking the whole pane.
    Column,
}

impl ImageFit {
    fn of(target: Target) -> Self {
        match target {
            Target::Pane { width } => ImageFit::Cover { width },
            Target::Expanded => ImageFit::Pane,
        }
    }
}

fn fitted_image(
    image: Arc<gpui::Image>,
    dimensions: Option<(u32, u32)>,
    fit: ImageFit,
    radius: f32,
) -> AnyElement {
    let ratio = dimensions
        .filter(|(w, h)| *w > 0 && *h > 0)
        .map(|(w, h)| w as f32 / h as f32);
    match fit {
        ImageFit::Cover { width } => {
            let (w, h) = match ratio {
                Some(ratio) => {
                    let h = (width / ratio).min(COVER_HEIGHT);
                    ((h * ratio).min(width), h)
                }
                None => (width, COVER_HEIGHT),
            };
            div()
                .flex()
                .justify_center()
                .w_full()
                .child(
                    img(image)
                        .w(px(w))
                        .h(px(h))
                        .object_fit(ObjectFit::Contain)
                        .rounded(px(radius)),
                )
                .into_any_element()
        }
        ImageFit::Pane => div()
            .flex_1()
            .min_h(px(0.))
            .w_full()
            .child(img(image).size_full().object_fit(ObjectFit::Contain))
            .into_any_element(),
        ImageFit::Column => {
            let mut frame = div().w_full().max_h(px(1200.));
            if let Some(ratio) = ratio {
                frame = frame.aspect_ratio(ratio);
            }
            frame
                .child(img(image).size_full().object_fit(ObjectFit::Contain))
                .into_any_element()
        }
    }
}

fn render_body(loaded: &Loaded, target: Target, cx: &mut App) -> AnyElement {
    let theme = cx.theme();
    let (caption, body_size, dim, gap) = (
        theme.type_scale().caption(),
        theme.type_scale().body(),
        theme.dim_foreground(),
        theme.space().sm(),
    );
    let radius = theme.radius();
    let sunken = theme.sunken();

    // A note in dim caption type: every "there is nothing to show" case.
    let note = |text: String| -> AnyElement {
        div()
            .text_size(px(caption))
            .text_color(dim)
            .child(text)
            .into_any_element()
    };

    match &loaded.preview.body {
        Body::Directory { .. } => note("a directory has no preview".to_string()),
        Body::Unreadable(why) => note(why.clone()),
        Body::TooLarge { size, limit } => note(format!(
            "{} is over the {} preview limit",
            format_size(*size),
            format_size(*limit)
        )),

        Body::Image { image, dimensions } => {
            fitted_image(image.clone(), *dimensions, ImageFit::of(target), radius)
        }

        Body::Video {
            poster,
            poster_dimensions,
            facts,
        } => {
            let mut column = div().flex().flex_col().gap(px(gap));
            match poster {
                Some(image) => {
                    // The poster keeps its column even expanded: the facts
                    // sit under it, so it is width-bound, not pane-bound.
                    column = column.child(fitted_image(
                        image.clone(),
                        *poster_dimensions,
                        match target {
                            Target::Expanded => ImageFit::Column,
                            Target::Pane { width } => ImageFit::Cover { width },
                        },
                        radius,
                    ));
                }
                // ffmpeg missing is not this app's error to escalate, but a
                // silent blank would look like a bug.
                None if facts.is_empty() => {
                    column = column.child(note("install ffmpeg for video previews".to_string()));
                }
                None => {}
            }
            column.into_any_element()
        }

        Body::Markdown(source) => {
            // gpui-component's rich text: headings, lists, tables and fenced
            // code, the last of which is coloured by the same syntax table as a
            // source file (see `omarchy_ui::SyntaxPalette`).
            let source = match target {
                Target::Expanded => source.clone(),
                Target::Pane { .. } => truncate_for_pane(source),
            };
            TextView::markdown("preview-markdown", source).into_any_element()
        }

        Body::Text { text, .. } => {
            let (text, highlights) = match target {
                Target::Expanded => (text.clone(), loaded.highlights.clone()),
                Target::Pane { .. } => clip_to_lines(text, &loaded.highlights, PANE_LINES),
            };

            let block = div()
                .p(px(gap))
                .bg(sunken)
                .text_size(px(body_size))
                .child(StyledText::new(text).with_highlights(highlights));
            match target {
                Target::Pane { .. } => block.rounded(px(radius)),
                // Full-bleed, like an expanded image: square corners. The
                // pane itself paints the same ground to the bottom edge —
                // see `expanded_pane` — so a short file does not end in a
                // colour seam where its last line does.
                Target::Expanded => block,
            }
            .into_any_element()
        }

        Body::Diff(diff) => render_diff(&loaded.diff, diff, target, cx),

        Body::Binary { head, .. } => {
            // Narrower rows in the pane: 16 bytes wrap there, and a wrapped hex
            // row loses the column alignment that makes it readable at all.
            let (per_row, limit) = match target {
                Target::Pane { .. } => (PANE_HEX_BYTES, PANE_HEX_ROWS),
                Target::Expanded => (16, usize::MAX),
            };
            let rows: Vec<String> = hex_rows(head, per_row).into_iter().take(limit).collect();
            let block = div()
                .p(px(gap))
                .bg(sunken)
                .text_size(px(caption))
                .text_color(dim)
                .child(rows.join("\n"));
            match target {
                Target::Pane { .. } => block.rounded(px(radius)),
                Target::Expanded => block,
            }
            .into_any_element()
        }
    }
}

/// Rows of a diff shown in the detail panel. See [`PANE_LINES`].
const PANE_DIFF_ROWS: usize = 80;

/// The diff body: rows of the file under a wash, not a patch as text.
///
/// The three things that make it read as a diff rather than as a listing, and
/// all three are Omarchy tokens rather than choices:
///
/// - the **wash** is a palette colour at `[controls]`' selected alpha, so it is
///   the same kind of fill a selected row uses and it works on all 22 themes —
///   including `vantablack` and `white`, where a solid green would be violent
/// - the **sign** carries the same information as the colour, because a wash at
///   0.18 is not something to make a reader depend on
/// - the **line number** is the file's own, which is the fact `git diff`'s `@@`
///   arithmetic makes you compute yourself
fn render_diff(rows: &[DiffRow], diff: &git::Diff, target: Target, cx: &mut App) -> AnyElement {
    let theme = cx.theme();
    let space = theme.space();
    let (caption, body_size, dim) = (
        theme.type_scale().caption(),
        theme.type_scale().body(),
        theme.dim_foreground(),
    );
    let (pad, gap, radius, sunken) = (
        space.sm(),
        space.control_gap(),
        theme.radius(),
        theme.sunken(),
    );
    // The wash. `selected` because a diff row is a persistent state of the line
    // rather than a hover, and because at 0.04 nobody would see it.
    let alpha = theme.controls().selected.fill_alpha;
    let added = omarchy_ui::color(theme.tokens.palette.green());
    let removed = omarchy_ui::color(theme.tokens.palette.red());

    let limit = match target {
        Target::Pane { .. } => PANE_DIFF_ROWS,
        Target::Expanded => usize::MAX,
    };
    let cut = rows.len() > limit || diff.truncated;

    // Wide enough for the largest number actually present, so a 40-line file
    // does not reserve a column for five digits.
    let widest = rows
        .iter()
        .filter_map(|row| match row {
            DiffRow::Code { number, .. } => *number,
            DiffRow::Skip(_) => None,
        })
        .max()
        .unwrap_or(0)
        .to_string()
        .len()
        .max(2);
    // The panel is a `dropdown_width` column; a gutter of digits there is width
    // taken from the code, which is the part worth reading. Same trade as the
    // hex rows, and the same conclusion.
    let numbers = target == Target::Expanded;
    let gutter = caption * 0.62 * widest as f32;

    let rendered = rows.iter().take(limit).map(|row| match row {
        DiffRow::Skip(heading) => div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(gap))
            .px(px(pad))
            .py(px(space.xxs()))
            .text_size(px(caption))
            .text_color(dim)
            .child("\u{22ef}") // ⋯ — lines not shown
            .children(heading.clone())
            .into_any_element(),

        DiffRow::Code {
            kind,
            number,
            text,
            highlights,
        } => {
            let (sign, wash, tint) = match kind {
                git::LineKind::Added => ("+", added.opacity(alpha), added),
                git::LineKind::Removed => ("\u{2212}", removed.opacity(alpha), removed),
                git::LineKind::Context => (" ", gpui::transparent_black(), dim),
            };

            div()
                .flex()
                .flex_row()
                .items_start()
                .w_full()
                .gap(px(space.xxs()))
                .px(px(pad))
                .bg(wash)
                .text_size(px(body_size))
                // The column is reserved even where there is no number, or a
                // removed line's text would start where every other line's
                // number does.
                .children(numbers.then(|| {
                    div()
                        .w(px(gutter))
                        .flex_shrink_0()
                        .text_size(px(caption))
                        .text_color(dim)
                        .children(number.map(|number| number.to_string()))
                }))
                .child(
                    div()
                        .w(px(caption))
                        .flex_shrink_0()
                        .text_color(tint)
                        .child(sign),
                )
                // The content is a flex sibling of the gutter, so a line too
                // long for the column wraps *under itself* rather than under the
                // numbers — and the wash covers every wrapped row, since it is
                // one element per line and not one per visual row.
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .child(StyledText::new(text.clone()).with_highlights(highlights.clone())),
                )
                .into_any_element()
        }
    });

    let block = div().flex().flex_col().py(px(pad)).bg(sunken);
    match target {
        Target::Pane { .. } => block.rounded(px(radius)),
        // Same full-bleed treatment as an expanded text body: square
        // corners, with `expanded_pane` carrying the ground past the
        // last hunk.
        Target::Expanded => block,
    }
    .children(rendered)
    .children(cut.then(|| {
        div()
            .px(px(pad))
            .pt(px(pad))
            .text_size(px(caption))
            .text_color(dim)
            .child(match target {
                // Saying "80 rows" of a change with 900 would be the lie the
                // line cap on a text preview exists to avoid.
                Target::Pane { .. } => "\u{22ef} more — space to expand".to_string(),
                Target::Expanded => {
                    format!("\u{22ef} cut at {} lines", git::MAX_DIFF_LINES)
                }
            })
    }))
    .into_any_element()
}

/// The head of a markdown file, cut on a line boundary.
fn truncate_for_pane(source: &str) -> String {
    source
        .lines()
        .take(PANE_LINES)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The first `lines` of `text`, with the highlight ranges cut to match.
///
/// The ranges are byte offsets into the whole file, so they have to be clipped
/// alongside it — handing `StyledText` a range past the end of its own string
/// panics during shaping.
fn clip_to_lines(
    text: &str,
    highlights: &[(Range<usize>, HighlightStyle)],
    lines: usize,
) -> (String, Vec<(Range<usize>, HighlightStyle)>) {
    let end = text
        .match_indices('\n')
        .nth(lines - 1)
        .map(|(i, _)| i)
        .unwrap_or(text.len());

    let clipped = highlights
        .iter()
        .take_while(|(range, _)| range.start < end)
        .map(|(range, style)| (range.start..range.end.min(end), *style))
        .filter(|(range, _)| !range.is_empty())
        .collect();

    (text[..end].to_string(), clipped)
}

/// `00000000  de ad be ef  ….` — one row per 16 bytes.
fn hex_rows(bytes: &[u8], per_row: usize) -> Vec<String> {
    // Two hex digits and a space each, less the trailing space.
    let width = per_row * 3 - 1;
    bytes
        .chunks(per_row)
        .enumerate()
        .map(|(row, chunk)| {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
            let ascii: String = chunk
                .iter()
                .map(|b| {
                    if b.is_ascii_graphic() || *b == b' ' {
                        *b as char
                    } else {
                        '.'
                    }
                })
                .collect();
            // Padded so the ascii column lines up on a short final row.
            format!(
                "{:08x}  {:<width$}  {ascii}",
                row * per_row,
                hex.join(" "),
                width = width
            )
        })
        .collect()
}

#[cfg(test)]
mod chrome_tests {
    use super::*;

    fn drop_scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("omafiles-drop-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn item(path: &Path) -> DragItem {
        DragItem {
            path: path.to_path_buf(),
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            is_dir: path.is_dir(),
        }
    }

    #[test]
    fn a_clean_drop_has_nothing_to_refuse() {
        let dir = drop_scratch("clean");
        let (src, dest) = (dir.join("src"), dir.join("dest"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(src.join("a.txt"), "a").unwrap();
        std::fs::create_dir(src.join("sub")).unwrap();
        let items = [item(&src.join("a.txt")), item(&src.join("sub"))];
        assert!(drop_refusals(&items, &dest).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_bad_item_is_named_and_a_good_one_is_not() {
        let dir = drop_scratch("bad");
        let (src, dest) = (dir.join("src"), dir.join("dest"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(src.join("fine.txt"), "").unwrap();
        std::fs::create_dir_all(dest.join("parent")).unwrap();
        std::fs::write(dest.join("parent/here.txt"), "").unwrap();
        let gone = src.join("gone.txt");
        let items = [
            item(&src.join("fine.txt")),
            item(&dest.join("parent/here.txt")),
            DragItem {
                path: gone.clone(),
                name: "gone.txt".into(),
                is_dir: false,
            },
            item(&dest.join("parent")),
        ];
        // Dropping into a directory inside one of the dragged directories.
        let reasons = drop_refusals(&items, &dest.join("parent"));
        assert_eq!(reasons.len(), 3, "{reasons:?}");
        assert!(reasons[0].contains("here.txt") && reasons[0].contains("already there"));
        assert!(reasons[1].contains("gone.txt") && reasons[1].contains("no longer"));
        assert!(reasons[2].contains("parent") && reasons[2].contains("into itself"));
        assert!(!reasons.iter().any(|r| r.contains("fine.txt")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_destination_is_refused() {
        let dir = drop_scratch("nodest");
        std::fs::write(dir.join("a.txt"), "").unwrap();
        let reasons = drop_refusals(&[item(&dir.join("a.txt"))], &dir.join("nowhere"));
        assert_eq!(reasons.len(), 1);
        assert!(reasons[0].contains("not a directory"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn middle_truncate_keeps_the_start_and_the_end() {
        assert_eq!(middle_truncate("short", 44), "short");
        let long = "/tmp/some/very/long/path/that/keeps/going/until/it/ends/here";
        let cut = middle_truncate(long, 20);
        assert_eq!(cut.chars().count(), 20);
        assert!(cut.starts_with("/tmp"), "the beginning survives");
        assert!(cut.ends_with("here"), "the end survives");
        assert!(cut.contains('\u{2026}'));
        // Multi-byte input must cut on char boundaries, not bytes.
        let accented = "éèêë".repeat(20);
        let cut = middle_truncate(&accented, 10);
        assert_eq!(cut.chars().count(), 10);
    }
}

#[cfg(test)]
mod keymap_tests {
    use super::*;

    /// A name `keymap` accepts but `typed_binding` cannot resolve would be a
    /// silently dead binding — the exact failure the keymap module's docs
    /// promise cannot happen.
    #[test]
    fn every_keymap_action_binds() {
        for name in keymap::known_actions() {
            assert!(
                typed_binding("f24", name, None).is_some(),
                "keymap action {name:?} has no typed binding arm"
            );
        }
    }

    /// A palette hint looks up keys by the keymap name; a typo here would
    /// show a command with no keys and no error.
    #[test]
    fn every_palette_command_names_a_known_action() {
        for command in COMMANDS {
            assert!(
                keymap::known_actions().contains(&command.action),
                "palette command {:?} names unknown action {:?}",
                command.label,
                command.action
            );
        }
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    /// The live theme when Omarchy is installed, the built-in one otherwise —
    /// these tests are about the highlighting chain, not about which palette.
    fn test_palette() -> omarchy_tokens::Palette {
        omarchy_tokens::load()
            .map(|tokens| tokens.palette)
            .unwrap_or_else(|_| Theme::load().tokens.palette.clone())
    }

    fn styles(n: usize) -> Vec<(Range<usize>, HighlightStyle)> {
        (0..n)
            .map(|i| (i * 4..i * 4 + 3, HighlightStyle::default()))
            .collect()
    }

    #[test]
    fn clipping_keeps_the_highlights_inside_the_string() {
        // A range past the end of the string it styles panics during text
        // shaping, so this is not a cosmetic concern.
        let text = "aaa\nbbb\nccc\nddd\n";
        let (clipped, highlights) = clip_to_lines(text, &styles(4), 2);

        assert_eq!(clipped, "aaa\nbbb");
        assert!(
            highlights.iter().all(|(r, _)| r.end <= clipped.len()),
            "a range past the end would panic when shaped: {highlights:?}"
        );
        assert!(
            !highlights.is_empty(),
            "the surviving lines keep their colour"
        );
    }

    #[test]
    fn clipping_a_file_shorter_than_the_limit_changes_nothing() {
        let text = "one\ntwo";
        let (clipped, highlights) = clip_to_lines(text, &styles(2), 100);
        assert_eq!(clipped, text);
        assert_eq!(highlights.len(), 2);
    }

    #[test]
    fn clipping_truncates_a_range_that_straddles_the_cut() {
        // A block comment spanning the cut must be trimmed, not dropped and not
        // left overhanging.
        let text = "aa\nbb\ncc\n";
        let straddling = vec![(0..8, HighlightStyle::default())];
        let (clipped, highlights) = clip_to_lines(text, &straddling, 2);
        assert_eq!(clipped, "aa\nbb");
        assert_eq!(highlights[0].0, 0..5);
    }

    #[test]
    fn hex_rows_pad_so_the_ascii_column_lines_up() {
        let rows = hex_rows(b"hello", 16);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].starts_with("00000000  68 65 6c 6c 6f "));
        assert!(rows[0].ends_with("  hello"), "{}", rows[0]);

        // Non-printable bytes become dots rather than control characters.
        let rows = hex_rows(&[0x00, 0x1b, b'A'], 16);
        assert!(rows[0].ends_with("  ..A"), "{}", rows[0]);

        // The offset column counts bytes, and follows the row width.
        let rows = hex_rows(&[0u8; 40], 16);
        assert_eq!(rows.len(), 3);
        assert!(rows[1].starts_with("00000010"));
        assert!(rows[2].starts_with("00000020"));
    }

    #[test]
    fn narrow_hex_rows_stay_aligned_and_reoffset() {
        // The pane uses 8 bytes a row because 16 wrap there, and a wrapped row
        // loses the alignment that makes a hex dump readable.
        let rows = hex_rows(&[0u8; 20], 8);
        assert_eq!(rows.len(), 3);
        assert!(rows[1].starts_with("00000008"), "{}", rows[1]);
        assert!(rows[2].starts_with("00000010"), "{}", rows[2]);

        // What the padding buys is that the ascii column *starts* at the same
        // offset in every row, including the short final one. Total row length
        // still varies, and should: ascii is the last column, and nothing
        // follows it that could fall out of line.
        //
        // 8 offset digits, two spaces, `8 * 3 - 1` hex columns, two spaces.
        const ASCII_COLUMN: usize = 8 + 2 + (8 * 3 - 1) + 2;
        for row in &rows {
            assert!(row.len() > ASCII_COLUMN, "{row:?}");
            assert!(
                !row[ASCII_COLUMN..].starts_with(' ') && row.as_bytes()[ASCII_COLUMN - 1] == b' ',
                "the ascii column must begin exactly at {ASCII_COLUMN}: {row:?}"
            );
        }
    }

    #[test]
    fn a_rust_file_is_highlighted_by_the_omarchy_palette() {
        // The whole chain in one check: classification picks the grammar,
        // tree-sitter parses, and the capture names resolve against our table.
        // If any link breaks, code previews silently render as plain text.
        let dir = std::env::temp_dir().join(format!("omafiles-hl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.rs");
        std::fs::write(&path, "fn main() {\n    let x = \"hi\";\n}\n").unwrap();

        let preview = Preview::load(&Entry::from_path(path));
        let syntax = SyntaxPalette::new(&test_palette());
        let highlights = highlight(&preview, &syntax);

        assert!(
            !highlights.is_empty(),
            "a Rust file must come back highlighted"
        );
        let Body::Text { text, .. } = &preview.body else {
            panic!("expected text");
        };
        assert!(
            highlights.iter().all(|(r, _)| r.end <= text.len()),
            "every range must be inside the text it styles"
        );
        assert!(
            highlights.iter().any(|(_, s)| s.color.is_some()),
            "the ranges must carry palette colours, not empty styles"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_diff_row_borrows_its_line_from_the_whole_file() {
        // The point of the two-sided design: a hunk in the middle of a function
        // is coloured by a parser that has seen the function, not by one handed
        // three lines out of context.
        let source = "fn main() {\n    let x = \"hi\";\n}\n";
        let syntax = SyntaxPalette::new(&test_palette());
        let side = Side::new(source, Some("rust"), &syntax);

        let line = side.line(2, "    let x = \"hi\";");
        assert!(!line.is_empty(), "the middle line must carry colours");
        assert!(
            line.iter().all(|(range, _)| range.end <= 17),
            "ranges are rebased onto the line, not left as file offsets: {line:?}"
        );
        assert!(line.iter().any(|(_, style)| style.color.is_some()));
    }

    #[test]
    fn a_line_that_no_longer_matches_the_file_is_left_plain() {
        // The diff carries the text and the file carries the colours. When they
        // disagree — a file rewritten under a stale diff, or a preview truncated
        // before the hunk — plain is right and colours shifted by a few bytes
        // are not.
        let syntax = SyntaxPalette::new(&test_palette());
        let side = Side::new("fn main() {\n    let x = 1;\n}\n", Some("rust"), &syntax);

        assert!(side.line(2, "something else entirely").is_empty());
        assert!(side.line(99, "past the end").is_empty());
        assert!(side.line(0, "there is no line zero").is_empty());
    }

    #[test]
    fn plain_text_comes_back_unhighlighted_rather_than_failing() {
        let dir = std::env::temp_dir().join(format!("omafiles-plain-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notes.txt");
        std::fs::write(&path, "just words\n").unwrap();

        let preview = Preview::load(&Entry::from_path(path));
        let syntax = SyntaxPalette::new(&test_palette());
        assert!(highlight(&preview, &syntax).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ------------------------------------------------------------------ watching

/// A `notify` watcher plus a channel the UI polls without blocking the frame.
struct DirWatcher {
    path: PathBuf,
    events: EventStream,
    _inner: notify::RecommendedWatcher,
}

impl DirWatcher {
    fn new(path: PathBuf) -> anyhow::Result<Self> {
        use notify::{RecursiveMode, Watcher as _};

        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                // Only content changes. Reading a file emits an *access* event,
                // and since every wake-up re-reads, treating those as changes is
                // a self-sustaining loop that pegs a core — verified before this
                // filter existed. Nothing else here distinguishes our own reads
                // from someone else's writes.
                if let Ok(event) = event
                    && matches!(
                        event.kind,
                        notify::EventKind::Create(_)
                            | notify::EventKind::Modify(_)
                            | notify::EventKind::Remove(_)
                    )
                {
                    let _ = tx.send(());
                }
            })?;
        // Depth 1: we render one directory, and recursing into a large tree
        // would watch thousands of files to no purpose.
        watcher.watch(&path, RecursiveMode::NonRecursive)?;

        Ok(Self {
            path,
            events: EventStream::new(rx),
            _inner: watcher,
        })
    }
}

enum Wait {
    Changed,
    Idle,
    Closed,
}

/// Shared receiver, so the polling task and the watcher can both hold it.
#[derive(Clone)]
struct EventStream(Arc<Mutex<Receiver<()>>>);

impl EventStream {
    fn new(rx: Receiver<()>) -> Self {
        Self(Arc::new(Mutex::new(rx)))
    }

    /// Block for up to `timeout`, then drain the rest of the burst.
    ///
    /// Draining matters: copying a hundred files in produces a hundred events,
    /// and re-reading once per event would make the listing thrash.
    fn wait(&self, timeout: Duration) -> Wait {
        let Ok(rx) = self.0.lock() else {
            return Wait::Closed;
        };
        match rx.recv_timeout(timeout) {
            Ok(()) => {
                while rx.try_recv().is_ok() {}
                Wait::Changed
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Wait::Idle,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Wait::Closed,
        }
    }
}
