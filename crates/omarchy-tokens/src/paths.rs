use std::path::PathBuf;

/// Every filesystem location the crate reads.
///
/// Constructed rather than hardcoded so tests can point the whole crate at a
/// fixture tree, and so `OMARCHY_ROOT` can redirect it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `~/.local/state/omarchy/current` — the directory holding the active
    /// theme. Watch **this**, never the `theme` directory inside it: a theme
    /// switch replaces that inode (`rm -rf` + `mv`) and kills any watch placed
    /// on it. See `plan/PLAN.md` §2.5.
    pub state_current: PathBuf,
    /// `~/.config/omarchy`
    pub config: PathBuf,
}

impl Paths {
    /// The real system locations, honouring `OMARCHY_ROOT`, `XDG_STATE_HOME`
    /// and `XDG_CONFIG_HOME`.
    pub fn system() -> Self {
        if let Some(root) = std::env::var_os("OMARCHY_ROOT") {
            return Self::rooted(PathBuf::from(root));
        }

        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));

        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/state"));

        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));

        Self {
            state_current: state.join("omarchy/current"),
            config: config.join("omarchy"),
        }
    }

    /// A fixture tree laid out as `<root>/state/current` and `<root>/config`.
    pub fn rooted(root: PathBuf) -> Self {
        Self {
            state_current: root.join("state/current"),
            config: root.join("config"),
        }
    }

    /// The active theme directory. Read through this; never watch it.
    pub fn theme_dir(&self) -> PathBuf {
        self.state_current.join("theme")
    }

    pub fn colors_toml(&self) -> PathBuf {
        self.theme_dir().join("colors.toml")
    }

    /// The theme's generated structural tokens. Layered *under*
    /// [`Self::user_shell_toml`].
    pub fn theme_shell_toml(&self) -> PathBuf {
        self.theme_dir().join("shell.toml")
    }

    /// The machine-level override. User keys win.
    pub fn user_shell_toml(&self) -> PathBuf {
        self.config.join("shell.toml")
    }

    pub fn theme_name(&self) -> PathBuf {
        self.state_current.join("theme.name")
    }

    /// Symlink to the current wallpaper.
    pub fn background(&self) -> PathBuf {
        self.state_current.join("background")
    }

    /// `~/.config/fontconfig` — `omarchy-font-set` writes `fonts.conf` here,
    /// and fontconfig is the source of truth for the monospace family.
    pub fn fontconfig_dir(&self) -> PathBuf {
        self.config
            .parent()
            .map(|c| c.join("fontconfig"))
            .unwrap_or_else(|| self.config.join("../fontconfig"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooted_paths_compose_correctly() {
        let paths = Paths::rooted(PathBuf::from("/fixtures/a"));
        assert_eq!(
            paths.colors_toml(),
            PathBuf::from("/fixtures/a/state/current/theme/colors.toml")
        );
        assert_eq!(
            paths.user_shell_toml(),
            PathBuf::from("/fixtures/a/config/shell.toml")
        );
    }
}
