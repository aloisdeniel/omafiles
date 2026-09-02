//! Reads the live Omarchy design tokens.
//!
//! A faithful port of `omarchy-shell`'s token model: the palette derivation
//! chain from `omarchy-theme-color`, and the typography, spacing, interaction
//! states and per-surface colours from `Commons/Style.qml` + `Commons/Color.qml`.
//! An app built on this reads as part of the same desktop as the bar and the
//! menu rather than merely themed to match.
//!
//! [`load`] takes one snapshot; [`watch`] keeps one current. Everything is
//! rooted at [`Paths`] rather than hardcoded, so the whole crate is testable
//! against fixture trees.

use std::path::Path;

use anyhow::{Context as _, Result};

mod color;
mod paths;
mod shell;
mod watch;

pub use color::{Mode, Palette, Rgb};
pub use paths::Paths;
pub use shell::{
    ControlStates, DEFAULT_BASE_SIZE, ShellValues, Spacing, StateStyle, Surface, Surfaces,
    Typography,
};
pub use watch::{Watcher, watch, watch_from};

/// A complete snapshot of the system's design tokens.
///
/// Always replace this value wholesale; never mutate one in place. A theme
/// switch rewrites several files, and a partially-applied theme rendered
/// mid-swap is the failure mode this type exists to prevent.
#[derive(Debug, Clone, PartialEq)]
pub struct Tokens {
    /// Slug as written by `omarchy-theme-set`, e.g. `tokyo-night`.
    pub theme_name: String,
    pub palette: Palette,
    pub typography: Typography,
    pub spacing: Spacing,
    pub controls: ControlStates,
    pub surfaces: Surfaces,
    pub geometry: Geometry,
    /// The merged `shell.toml`, for keys without a typed accessor.
    pub shell: ShellValues,
}

impl Tokens {
    pub fn mode(&self) -> Mode {
        self.palette.mode()
    }
}

/// Corner radius and edge gap, both owned by Hyprland rather than by the theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    /// Mirrors `decoration:rounding`.
    pub corner_radius: f32,
    /// **Half** of `general:gaps_out`, matching `Style.qml`. Hyprland's value is
    /// tuned as a window-to-window distance and reads as cavernous when used as
    /// a panel-to-edge inset.
    pub gaps_out: f32,
}

impl Default for Geometry {
    fn default() -> Self {
        // Style.qml's own fallbacks for when hyprctl is unavailable.
        Self {
            corner_radius: 0.0,
            gaps_out: 5.0,
        }
    }
}

/// Read every token once, from the real system paths.
pub fn load() -> Result<Tokens> {
    load_from(&Paths::system())
}

/// Read every token once, rooted at `paths`. Use this in tests.
pub fn load_from(paths: &Paths) -> Result<Tokens> {
    let colors = paths.colors_toml();
    let palette = Palette::from_toml_file(&colors)
        .with_context(|| format!("reading palette from {}", colors.display()))?;

    // The theme's generated file first, the user's machine-level overrides on
    // top — user keys win, as in `Color.qml`.
    let shell = ShellValues::from_toml_file(&paths.theme_shell_toml())
        .overlay(ShellValues::from_toml_file(&paths.user_shell_toml()));

    let typography = Typography::new(resolve_monospace_family(), &shell);
    let spacing = Spacing::new(&shell, &typography);
    let controls = ControlStates::new(&shell, &palette);
    let surfaces = Surfaces::new(&shell, &palette);

    Ok(Tokens {
        theme_name: read_theme_name(&paths.theme_name()),
        palette,
        typography,
        spacing,
        controls,
        surfaces,
        geometry: read_geometry(),
        shell,
    })
}

fn read_theme_name(path: &Path) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

/// fontconfig is the source of truth — `omarchy-font-set` writes a
/// `prepend_first` match on the `monospace` alias and every Qt/GTK/shell
/// consumer resolves through it. `fc-match` returns a comma-separated alias
/// list, so take the first entry.
fn resolve_monospace_family() -> String {
    const FALLBACK: &str = "monospace";

    let output = std::process::Command::new("fc-match")
        .args(["monospace", "-f", "%{family}"])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let raw = String::from_utf8_lossy(&out.stdout);
            raw.split(',')
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(FALLBACK)
                .to_string()
        }
        _ => FALLBACK.to_string(),
    }
}

/// Query Hyprland for the *effective* values. Omarchy 4's Hyprland config is
/// Lua, so parsing the config file is both harder and less correct than asking
/// the compositor.
fn read_geometry() -> Geometry {
    let mut geometry = Geometry::default();

    if let Some(v) = hyprctl_option("decoration:rounding").and_then(|j| first_number(&j, "int")) {
        geometry.corner_radius = v.max(0.0);
    }

    if let Some(v) = hyprctl_option("general:gaps_out").and_then(|j| first_number(&j, "css")) {
        geometry.gaps_out = (v / 2.0).round().max(0.0);
    }

    geometry
}

fn hyprctl_option(option: &str) -> Option<String> {
    let out = std::process::Command::new("hyprctl")
        .args(["getoption", option, "-j"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Pull the first number out of `"key": <value>` in hyprctl's JSON. `gaps_out`
/// answers with a CSS-ish string (`"5 5 5 5"`), `rounding` with a bare integer,
/// so both go through one path rather than pulling in a JSON dependency for two
/// fields.
fn first_number(json: &str, key: &str) -> Option<f32> {
    let after = json.split(&format!("\"{key}\"")).nth(1)?;
    let after = after.trim_start().strip_prefix(':')?;

    let mut seen_digit = false;
    let mut buffer = String::new();
    for ch in after.chars() {
        if ch.is_ascii_digit() || (ch == '-' && buffer.is_empty()) || (ch == '.' && seen_digit) {
            seen_digit |= ch.is_ascii_digit();
            buffer.push(ch);
        } else if seen_digit {
            break;
        } else if !buffer.is_empty() {
            buffer.clear();
        }
    }

    buffer.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hyprctl_integer_options() {
        let json = r#"{"option": "decoration:rounding", "int": 4, "set": true }"#;
        assert_eq!(first_number(json, "int"), Some(4.0));
    }

    #[test]
    fn parses_hyprctl_css_options() {
        let json = r#"{"option": "general:gaps_out", "css": "10 10 10 10", "set": true }"#;
        assert_eq!(first_number(json, "css"), Some(10.0));
    }

    #[test]
    fn missing_hyprctl_key_is_none() {
        let json = r#"{"option": "general:gaps_out", "set": true }"#;
        assert_eq!(first_number(json, "css"), None);
    }

    /// A fixture tree with a theme and (optionally) a user override.
    fn fixture(name: &str, user_shell: Option<&str>) -> (tempdir::TempTree, Paths) {
        let tree = tempdir::TempTree::new(name);
        let paths = Paths::rooted(tree.path().to_path_buf());
        std::fs::create_dir_all(paths.theme_dir()).unwrap();
        std::fs::create_dir_all(&paths.config).unwrap();

        std::fs::write(
            paths.colors_toml(),
            "mode = \"dark\"\nbackground = \"#1a1b26\"\nforeground = \"#a9b1d6\"\n\
             accent = \"#7aa2f7\"\nred = \"#f7768e\"\nyellow = \"#e0af68\"\n\
             green = \"#9ece6a\"\ncyan = \"#449dab\"\nblue = \"#7aa2f7\"\n\
             magenta = \"#ad8ee6\"\nmuted = \"#414868\"\n",
        )
        .unwrap();
        std::fs::write(paths.theme_name(), "fixture\n").unwrap();
        std::fs::write(paths.theme_shell_toml(), "[font]\nbase-size = 12\n").unwrap();
        if let Some(text) = user_shell {
            std::fs::write(paths.user_shell_toml(), text).unwrap();
        }
        (tree, paths)
    }

    #[test]
    fn loads_a_complete_token_set() {
        let (_tree, paths) = fixture("load-complete", None);
        let tokens = load_from(&paths).unwrap();

        assert_eq!(tokens.theme_name, "fixture");
        assert_eq!(tokens.mode(), Mode::Dark);
        assert_eq!(tokens.palette.background().to_hex(), "#1a1b26");
        assert_eq!(tokens.typography.base_size, 12.0);
        assert_eq!(tokens.typography.body(), 12.0);
        assert_eq!(tokens.spacing.lg(), 8.0);
        assert_eq!(tokens.controls.normal.fill_alpha, 0.04);
    }

    /// Regression: the user's `~/.config/omarchy/shell.toml` layers on top of
    /// the theme's, and this silently regressed once already when a discarded
    /// parse error made base-size fall back to the default.
    #[test]
    fn user_shell_toml_overrides_the_theme() {
        let (_tree, paths) = fixture("load-override", Some("[font]\nbase-size = 14\n"));
        let tokens = load_from(&paths).unwrap();

        assert_eq!(tokens.typography.base_size, 14.0);
        assert_eq!(tokens.typography.body(), 14.0);
        assert_eq!(tokens.typography.heading(), 19.0);
        assert_eq!(tokens.spacing.lg(), 9.0, "spacing tracks the font scale");
    }

    #[test]
    fn a_missing_theme_is_an_error_not_a_silent_default() {
        let paths = Paths::rooted(std::env::temp_dir().join("omarchy-tokens-absent"));
        assert!(load_from(&paths).is_err());
    }

    #[test]
    fn theme_name_falls_back_when_unreadable() {
        let (_tree, paths) = fixture("load-noname", None);
        std::fs::remove_file(paths.theme_name()).unwrap();
        assert_eq!(load_from(&paths).unwrap().theme_name, "unknown");
    }
}

/// A self-deleting temporary directory. Small enough not to justify a
/// dev-dependency, and keeps the crate's dependency list honest.
#[cfg(test)]
pub(crate) mod tempdir {
    use std::path::{Path, PathBuf};

    pub struct TempTree(PathBuf);

    impl TempTree {
        pub fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "omarchy-tokens-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create temp tree");
            Self(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
