//! M11's rebindable keymap: defaults as data, `keymap.toml` layered on top.
//!
//! §6.8 asks for a keymap "loaded from `~/.config/omafiles/keymap.toml`,
//! merged over defaults, so it is rebindable without a rebuild". The merge
//! rule is the simplest one that cannot surprise: **naming an action in a
//! section replaces every default key that action had in that section.** A
//! string binds one key, a list binds several, an empty list unbinds. What a
//! user did not name keeps its defaults, so a one-line file changes one thing.
//!
//! ```toml
//! [listing]
//! toggle_preview = "p"          # replaces space
//! open_terminal = []            # unbinds t
//! [global]
//! new_tab = ["ctrl-t", "f7"]
//! ```
//!
//! The names are the actions' snake_case names, and the key syntax is gpui's
//! (`ctrl-shift-g`). This module is pure data — the one place that knows the
//! typed gpui actions is `bind_keys` in `main.rs`, which consumes
//! [`Keymap::bindings`] and is also where an unknown name would fail to
//! resolve; names are validated *here* so the error can carry the file's
//! words rather than a silent no-op.
//!
//! A file problem never stops the app: the defaults stand, and the problem is
//! reported in [`Keymap::problems`] for the status bar. A file manager that
//! refuses to start over a typo in a config file locks the user out of the
//! tool they would fix it with.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

/// Where a binding applies — gpui key contexts, plus "everywhere".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    Global,
    Listing,
    Sidebar,
}

impl Context {
    /// The `keymap.toml` section name.
    pub fn section(self) -> &'static str {
        match self {
            Context::Global => "global",
            Context::Listing => "listing",
            Context::Sidebar => "sidebar",
        }
    }
}

/// One effective binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub keys: String,
    pub action: &'static str,
    pub context: Context,
}

/// The default bindings, as data. **Order is meaningful**: gpui gives later
/// bindings precedence on a conflict, so this mirrors the order `bind_keys`
/// historically used.
///
/// Every action name here must have an arm in `main.rs`'s binder — the test
/// at the bottom of `main.rs` holds the two together.
#[rustfmt::skip]
pub const DEFAULTS: &[(&str, &str, Context)] = &[
    // Sidebar: the listing's movement verbs, rebound per context.
    ("move_down",         "j",              Context::Sidebar),
    ("move_down",         "down",           Context::Sidebar),
    ("move_up",           "k",              Context::Sidebar),
    ("move_up",           "up",             Context::Sidebar),
    ("move_first",        "g",              Context::Sidebar),
    ("move_last",         "shift-g",        Context::Sidebar),
    ("open",              "enter",          Context::Sidebar),
    ("open",              "l",              Context::Sidebar),
    ("open",              "right",          Context::Sidebar),
    ("unpin_selected",    "delete",         Context::Sidebar),
    ("move_pin_up",       "alt-up",         Context::Sidebar),
    ("move_pin_up",       "alt-k",          Context::Sidebar),
    ("move_pin_down",     "alt-down",       Context::Sidebar),
    ("move_pin_down",     "alt-j",          Context::Sidebar),
    ("quit",              "q",              Context::Sidebar),
    ("show_help",         "?",              Context::Sidebar),
    ("start_search",      "/",              Context::Sidebar),
    ("open_terminal",     "t",              Context::Sidebar),

    // Everywhere.
    ("focus_next",        "tab",            Context::Global),
    ("focus_previous",    "shift-tab",      Context::Global),
    ("pin_current",       "ctrl-p",         Context::Global),
    ("new_tab",           "ctrl-t",         Context::Global),
    ("close_tab",         "ctrl-w",         Context::Global),
    ("next_tab",          "ctrl-tab",       Context::Global),
    ("previous_tab",      "ctrl-shift-tab", Context::Global),
    ("new_workspace",     "ctrl-n",         Context::Global),
    ("delete_workspace",  "ctrl-shift-w",   Context::Global),
    ("move_tab_to_next_workspace", "ctrl-shift-m", Context::Global),
    ("rename_workspace",  "ctrl-shift-r",   Context::Global),
    ("dismiss",           "escape",         Context::Global),
    ("toggle_left_panel", "ctrl-b",         Context::Global),
    ("toggle_right_panel", "ctrl-shift-b",  Context::Global),
    ("edit_path",         "ctrl-l",         Context::Global),
    ("switch_branch",     "ctrl-shift-g",   Context::Global),
    ("server_menu",       "ctrl-s",         Context::Global),
    ("server_list",       "ctrl-shift-s",   Context::Global),
    ("add_network",       "ctrl-shift-n",   Context::Global),
    ("command_palette",   "ctrl-k",         Context::Global),
    ("quit",              "ctrl-q",         Context::Global),
    // Driving a modal's list from inside its text field — bound globally
    // because gpui-component's Input sets no key context (see bind_keys).
    ("move_down",         "down",           Context::Global),
    ("move_up",           "up",             Context::Global),

    // The listing.
    ("show_help",         "?",              Context::Listing),
    ("start_search",      "/",              Context::Listing),
    ("move_down",         "j",              Context::Listing),
    ("move_down",         "down",           Context::Listing),
    ("move_up",           "k",              Context::Listing),
    ("move_up",           "up",             Context::Listing),
    ("move_first",        "g",              Context::Listing),
    ("move_first",        "home",           Context::Listing),
    ("move_last",         "shift-g",        Context::Listing),
    ("move_last",         "end",            Context::Listing),
    ("page_down",         "ctrl-d",         Context::Listing),
    ("page_down",         "pagedown",       Context::Listing),
    ("page_up",           "ctrl-u",         Context::Listing),
    ("page_up",           "pageup",         Context::Listing),
    ("open",              "enter",          Context::Listing),
    ("open",              "l",              Context::Listing),
    ("open",              "right",          Context::Listing),
    ("go_up",             "backspace",      Context::Listing),
    ("go_up",             "h",              Context::Listing),
    ("go_up",             "left",           Context::Listing),
    ("go_back",           "alt-left",       Context::Listing),
    ("go_forward",        "alt-right",      Context::Listing),
    ("toggle_hidden",     "ctrl-h",         Context::Listing),
    ("refresh",           "f5",             Context::Listing),
    ("refresh",           "ctrl-r",         Context::Listing),
    ("quit",              "q",              Context::Listing),
    ("toggle_preview",    "space",          Context::Listing),
    ("open_terminal",     "t",              Context::Listing),
    ("ask_agent",         "a",              Context::Listing),
    ("share_entry",       "s",              Context::Listing),
    ("copy_entry",        "ctrl-c",         Context::Listing),
    ("cut_entry",         "ctrl-x",         Context::Listing),
    ("copy_path",         "ctrl-shift-c",   Context::Listing),
    ("paste_here",        "ctrl-v",         Context::Listing),
    ("delete_entry",      "delete",         Context::Listing),
    ("compress_entry",    "z",              Context::Listing),
    ("move_entry",        "m",              Context::Listing),
    ("create_file",       "n",              Context::Listing),
    // shift-f10, because the dedicated menu key never reaches gpui's
    // Wayland backend.
    ("entry_menu",        "shift-f10",      Context::Listing),
    // Selection: the sweep, the flip, the lot. What a drag then carries.
    ("extend_down",       "shift-down",     Context::Listing),
    ("extend_down",       "shift-j",        Context::Listing),
    ("extend_up",         "shift-up",       Context::Listing),
    ("extend_up",         "shift-k",        Context::Listing),
    ("toggle_select",     "insert",         Context::Listing),
    ("select_all",        "ctrl-a",         Context::Listing),
];

/// Actions that ship with no key: reachable from the palette, bindable from
/// `keymap.toml`. Settings toggles live here — a key for something flipped
/// once a year would only be a key to hit by accident.
pub const UNBOUND: &[&str] = &["toggle_button_labels"];

/// Every bindable action name — [`DEFAULTS`] plus any action that ships
/// unbound. Membership here is what makes a `keymap.toml` name valid.
pub fn known_actions() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = DEFAULTS.iter().map(|(name, ..)| *name).collect();
    names.extend_from_slice(UNBOUND);
    names.sort_unstable();
    names.dedup();
    names
}

/// The effective keymap: defaults with the user's file merged over them.
#[derive(Debug, Clone)]
pub struct Keymap {
    pub bindings: Vec<Binding>,
    /// What was wrong with the file, in the file's own words. Empty when the
    /// file is absent or clean.
    pub problems: Vec<String>,
}

impl Keymap {
    pub fn defaults() -> Self {
        Self {
            bindings: DEFAULTS
                .iter()
                .map(|(action, keys, context)| Binding {
                    keys: (*keys).to_string(),
                    action,
                    context: *context,
                })
                .collect(),
            problems: Vec::new(),
        }
    }

    /// Read `path` over the defaults. A missing file is simply the defaults;
    /// a broken one is the defaults plus a problem to show.
    pub fn load(path: &Path) -> Self {
        let mut keymap = Self::defaults();
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return keymap,
            Err(err) => {
                keymap.problems.push(format!("keymap.toml: {err}"));
                return keymap;
            }
        };
        keymap.merge(&source);
        keymap
    }

    /// Apply one file's worth of overrides.
    fn merge(&mut self, source: &str) {
        let table: BTreeMap<String, BTreeMap<String, toml::Value>> = match toml::from_str(source) {
            Ok(table) => table,
            Err(err) => {
                // toml's message spans lines; the notice wants one.
                let mut message = String::from("keymap.toml: ");
                for (i, line) in err.message().lines().enumerate() {
                    if i > 0 {
                        message.push(' ');
                    }
                    message.push_str(line.trim());
                }
                self.problems.push(message);
                return;
            }
        };

        for (section, actions) in table {
            let context = match section.as_str() {
                "global" => Context::Global,
                "listing" => Context::Listing,
                "sidebar" => Context::Sidebar,
                other => {
                    self.problems
                        .push(format!("keymap.toml: unknown section [{other}]"));
                    continue;
                }
            };
            for (name, value) in actions {
                let Some(action) = known_actions()
                    .iter()
                    .copied()
                    .find(|known| *known == name.as_str())
                else {
                    self.problems
                        .push(format!("keymap.toml: unknown action \"{name}\""));
                    continue;
                };
                let keys = match keys_of_value(&value) {
                    Some(keys) => keys,
                    None => {
                        self.problems.push(format!(
                            "keymap.toml: \"{name}\" wants a key string or a list of them"
                        ));
                        continue;
                    }
                };
                // Replace: every default this action had in this section goes,
                // then the file's keys come in — in the file's order, at the
                // end, which also gives them precedence on a conflict.
                self.bindings
                    .retain(|b| !(b.action == action && b.context == context));
                self.bindings.extend(keys.into_iter().map(|keys| Binding {
                    keys,
                    action,
                    context,
                }));
            }
        }
    }

    /// The effective keys for an action, prettified, for the palette's hint.
    pub fn keys_for(&self, action: &str) -> Vec<String> {
        let mut keys: Vec<String> = self
            .bindings
            .iter()
            .filter(|b| b.action == action)
            .map(|b| pretty_keys(&b.keys))
            .collect();
        keys.dedup();
        keys
    }
}

/// A string is one key, a list is several, an empty list unbinds.
fn keys_of_value(value: &toml::Value) -> Option<Vec<String>> {
    match value {
        toml::Value::String(keys) => Some(vec![keys.clone()]),
        toml::Value::Array(items) => items
            .iter()
            .map(|item| item.as_str().map(str::to_string))
            .collect(),
        _ => None,
    }
}

/// gpui key syntax, rendered the way the help sheet writes keys: `ctrl-shift-g`
/// becomes `^⇧g`.
pub fn pretty_keys(keys: &str) -> String {
    let mut out = String::new();
    let parts: Vec<&str> = keys.split('-').collect();
    for (i, part) in parts.iter().enumerate() {
        let is_last = i + 1 == parts.len();
        let _ = match (*part, is_last) {
            // A trailing empty part means the key itself was `-`.
            ("", true) => write!(out, "-"),
            ("ctrl", false) => write!(out, "^"),
            ("alt", false) => write!(out, "\u{2325}"),
            ("shift", false) => write!(out, "\u{21e7}"),
            ("cmd" | "super", false) => write!(out, "\u{2318}"),
            ("enter", _) => write!(out, "\u{23ce}"),
            ("escape", _) => write!(out, "esc"),
            ("backspace", _) => write!(out, "\u{232b}"),
            ("delete", _) => write!(out, "\u{2326}"),
            ("space", _) => write!(out, "space"),
            ("tab", _) => write!(out, "\u{21e5}"),
            ("up", _) => write!(out, "\u{2191}"),
            ("down", _) => write!(out, "\u{2193}"),
            ("left", _) => write!(out, "\u{2190}"),
            ("right", _) => write!(out, "\u{2192}"),
            (other, _) => write!(out, "{other}"),
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_load_without_a_file() {
        let keymap = Keymap::load(Path::new("/definitely/not/a/keymap.toml"));
        assert!(keymap.problems.is_empty());
        assert_eq!(keymap.bindings.len(), DEFAULTS.len());
    }

    #[test]
    fn an_override_replaces_only_its_own_action_and_section() {
        let mut keymap = Keymap::defaults();
        keymap.merge("[listing]\ntoggle_preview = \"p\"\n");
        assert!(keymap.problems.is_empty());

        let previews: Vec<_> = keymap
            .bindings
            .iter()
            .filter(|b| b.action == "toggle_preview")
            .collect();
        assert_eq!(previews.len(), 1);
        assert_eq!(previews[0].keys, "p");
        assert_eq!(previews[0].context, Context::Listing);

        // Everything else keeps its defaults — including `t` in the sidebar.
        assert!(
            keymap
                .bindings
                .iter()
                .any(|b| b.action == "open_terminal" && b.context == Context::Sidebar)
        );
    }

    #[test]
    fn a_list_binds_several_and_an_empty_list_unbinds() {
        let mut keymap = Keymap::defaults();
        keymap.merge("[global]\nnew_tab = [\"ctrl-t\", \"f7\"]\n[listing]\nopen_terminal = []\n");
        assert!(keymap.problems.is_empty());

        let tabs: Vec<_> = keymap
            .bindings
            .iter()
            .filter(|b| b.action == "new_tab")
            .map(|b| b.keys.as_str())
            .collect();
        assert_eq!(tabs, ["ctrl-t", "f7"]);
        assert!(
            !keymap
                .bindings
                .iter()
                .any(|b| b.action == "open_terminal" && b.context == Context::Listing),
            "an empty list unbinds"
        );
    }

    #[test]
    fn problems_name_what_is_wrong_and_leave_defaults_standing() {
        let mut keymap = Keymap::defaults();
        keymap.merge("[listing]\nfly_to_the_moon = \"m\"\nrefresh = 7\n[cockpit]\nx = \"y\"\n");
        assert_eq!(keymap.problems.len(), 3);
        let all = keymap.problems.join("\n");
        assert!(all.contains("fly_to_the_moon"));
        assert!(all.contains("refresh"));
        assert!(all.contains("[cockpit]"));
        // The broken refresh override left the default alone.
        assert!(
            keymap
                .bindings
                .iter()
                .any(|b| b.action == "refresh" && b.keys == "f5")
        );
    }

    #[test]
    fn a_syntax_error_is_one_problem_not_a_crash() {
        let mut keymap = Keymap::defaults();
        keymap.merge("[listing\nbroken");
        assert_eq!(keymap.problems.len(), 1);
        assert!(keymap.problems[0].starts_with("keymap.toml: "));
        assert_eq!(keymap.bindings.len(), DEFAULTS.len());
    }

    #[test]
    fn pretty_keys_reads_like_the_help_sheet() {
        assert_eq!(pretty_keys("ctrl-shift-g"), "^\u{21e7}g");
        assert_eq!(pretty_keys("alt-left"), "\u{2325}\u{2190}");
        assert_eq!(pretty_keys("enter"), "\u{23ce}");
        assert_eq!(pretty_keys("escape"), "esc");
        assert_eq!(pretty_keys("space"), "space");
        assert_eq!(pretty_keys("q"), "q");
        assert_eq!(pretty_keys("shift-tab"), "\u{21e7}\u{21e5}");
    }

    #[test]
    fn keys_for_prettifies_and_deduplicates() {
        let keymap = Keymap::defaults();
        let keys = keymap.keys_for("quit");
        // q (sidebar), ^q (global), q (listing) — the doubled q collapses
        // only when adjacent, so assert on content rather than order.
        assert!(keys.contains(&"q".to_string()));
        assert!(keys.contains(&"^q".to_string()));
    }

    #[test]
    fn every_default_action_is_known() {
        for (name, ..) in DEFAULTS {
            assert!(known_actions().contains(name));
        }
    }
}
