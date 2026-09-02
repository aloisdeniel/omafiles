//! Conformance against the real theme corpus.
//!
//! `omarchy-theme-color --file <path> --all` is the oracle: it is the resolver
//! every other Omarchy consumer goes through. If our derivation drifts from it,
//! omafiles renders colours the rest of the desktop does not agree with.
//!
//! Skipped when Omarchy is not installed, so the suite still runs in CI.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use omarchy_tokens::Palette;

const THEMES_DIR: &str = "/usr/share/omarchy/themes";

fn theme_dirs() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(THEMES_DIR) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("colors.toml").is_file())
        .collect();
    dirs.sort();
    dirs
}

fn skip_unless_installed(themes: &[PathBuf]) -> bool {
    if themes.is_empty() {
        eprintln!("skipping: no themes under {THEMES_DIR} (Omarchy not installed?)");
        return true;
    }
    false
}

/// Ask Omarchy to resolve a palette. `None` when the CLI is unavailable.
fn oracle(colors_toml: &Path) -> Option<HashMap<String, String>> {
    let out = Command::new("omarchy-theme-color")
        .arg("--file")
        .arg(colors_toml)
        .arg("--all")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }

    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| {
                let (key, value) = line.split_once('\t')?;
                Some((key.to_string(), value.to_string()))
            })
            .collect(),
    )
}

#[test]
fn every_stock_theme_parses() {
    let themes = theme_dirs();
    if skip_unless_installed(&themes) {
        return;
    }

    for dir in &themes {
        let name = dir.file_name().unwrap().to_string_lossy();
        let palette = Palette::from_toml_file(&dir.join("colors.toml"))
            .unwrap_or_else(|e| panic!("{name} failed to parse: {e:#}"));

        // Every role an accessor exposes must resolve, whether the theme wrote
        // it or the derivation chain filled it in.
        for role in [
            "accent",
            "selection",
            "muted",
            "background",
            "dark_background",
            "darker_background",
            "lighter_background",
            "foreground",
            "dark_foreground",
            "light_foreground",
            "bright_foreground",
            "red",
            "yellow",
            "orange",
            "green",
            "cyan",
            "blue",
            "magenta",
            "brown",
        ] {
            assert!(
                palette.try_get(role).is_some(),
                "{name}: role {role:?} did not resolve"
            );
        }
    }

    eprintln!("checked {} themes", themes.len());
}

#[test]
fn derivation_agrees_with_omarchy_theme_color() {
    let themes = theme_dirs();
    if skip_unless_installed(&themes) {
        return;
    }

    let colors_toml = themes[0].join("colors.toml");
    if oracle(&colors_toml).is_none() {
        eprintln!("skipping: omarchy-theme-color unavailable");
        return;
    }

    // Every key we claim to derive. If Omarchy adds a rule we have not ported,
    // this is where it shows up.
    const CHECKED: &[&str] = &[
        "accent",
        "selection",
        "selection_background",
        "selection_foreground",
        "muted",
        "background",
        "dark_background",
        "darker_background",
        "lighter_background",
        "foreground",
        "dark_foreground",
        "light_foreground",
        "bright_foreground",
        "cursor",
        "red",
        "yellow",
        "orange",
        "green",
        "cyan",
        "blue",
        "magenta",
        "brown",
        "purple",
        "bright_red",
        "bright_yellow",
        "bright_green",
        "bright_cyan",
        "bright_blue",
        "bright_magenta",
        "bright_purple",
        "bg",
        "fg",
        "dark_bg",
        "darker_bg",
        "lighter_bg",
        "dark_fg",
        "light_fg",
        "bright_fg",
    ];

    let mut mismatches: Vec<String> = Vec::new();

    for dir in &themes {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let colors_toml = dir.join("colors.toml");

        let palette = Palette::from_toml_file(&colors_toml)
            .unwrap_or_else(|e| panic!("{name} failed to parse: {e:#}"));
        let expected =
            oracle(&colors_toml).unwrap_or_else(|| panic!("{name}: omarchy-theme-color failed"));

        for key in CHECKED {
            let Some(want) = expected.get(*key) else {
                continue; // the oracle does not define it for this theme
            };
            match palette.try_get(key) {
                Some(got) if got.to_hex().eq_ignore_ascii_case(want) => {}
                Some(got) => mismatches.push(format!(
                    "{name}: {key} = {} but omarchy says {want}",
                    got.to_hex()
                )),
                None => mismatches.push(format!("{name}: {key} missing, omarchy says {want}")),
            }
        }
    }

    assert!(
        mismatches.is_empty(),
        "{} mismatches against omarchy-theme-color:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );

    eprintln!(
        "{} themes × {} keys agree with omarchy-theme-color",
        themes.len(),
        CHECKED.len()
    );
}
