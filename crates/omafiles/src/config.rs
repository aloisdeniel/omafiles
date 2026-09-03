//! `~/.config/omafiles/config.toml`: the handful of settings that are not
//! keys (those live in `keymap.toml`) and not places.
//!
//! One flat table, every field optional, so an empty file — or none — is the
//! defaults. Written back only by the in-app toggles, and then in full.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Whether the action buttons in the detail panel and the status bar
    /// spell out their verb beside the glyph. Off by default: a bar of
    /// glyphs reads as chrome, and the verb is one hover away.
    pub button_labels: bool,
}

impl Config {
    /// Read `path`. Missing is the defaults; unreadable is the defaults too,
    /// with the reason on stderr — a bad settings file should not keep the
    /// window from opening.
    pub fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(err) => {
                eprintln!("omafiles: reading {}: {err}", path.display());
                return Self::default();
            }
        };
        match toml::from_str(&text) {
            Ok(config) => config,
            Err(err) => {
                eprintln!("omafiles: {}: {err}", path.display());
                Self::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("omafiles-config-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.toml")
    }

    #[test]
    fn a_missing_file_is_the_defaults() {
        let path = scratch("missing");
        assert_eq!(Config::load(&path), Config::default());
        assert!(!Config::default().button_labels);
    }

    #[test]
    fn a_saved_file_round_trips() {
        let path = scratch("roundtrip");
        let config = Config {
            button_labels: true,
        };
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path), config);
    }

    #[test]
    fn a_broken_file_is_the_defaults_not_a_crash() {
        let path = scratch("broken");
        std::fs::write(&path, "button_labels = maybe").unwrap();
        assert_eq!(Config::load(&path), Config::default());
    }
}
