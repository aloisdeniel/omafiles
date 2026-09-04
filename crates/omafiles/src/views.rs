//! `~/.config/omafiles/views.toml`: how each directory's listing is laid
//! out — the column it is sorted by, and how wide the size and age columns
//! were dragged.
//!
//! Per directory rather than global because the two questions have
//! different answers in different places: `~/Downloads` wants newest first
//! and a wide age column, a source tree wants names. A directory is in the
//! file once its listing has been touched, and out again when it is put back
//! to the defaults, so the file only ever says what differs.
//!
//! One table per directory, keyed by its absolute path:
//!
//! ```toml
//! ["/home/me/Downloads"]
//! sort = "age"
//! descending = false
//! size_width = 96
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::listing::{Sort, SortKey};

/// One directory's listing settings. Every field optional in the file, so a
/// table naming only `sort` leaves the widths at the defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DirectoryView {
    pub sort: SortKey,
    pub descending: bool,
    /// The size column's width in logical pixels, as last dragged. Absent
    /// until then, so the built-in width stands in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_width: Option<u32>,
    /// The age column's width, on the same terms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_width: Option<u32>,
}

impl DirectoryView {
    pub fn sort(&self) -> Sort {
        Sort {
            key: self.sort,
            descending: self.descending,
        }
    }

    pub fn set_sort(&mut self, sort: Sort) {
        self.sort = sort.key;
        self.descending = sort.descending;
    }
}

/// Every directory that has been laid out by hand.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Views {
    directories: BTreeMap<String, DirectoryView>,
}

impl Views {
    /// Load the saved views. Missing file is no views; a corrupt one loads
    /// as none and is left on disk, the `places.toml` discipline.
    pub fn load(config_dir: &Path) -> Self {
        let path = config_dir.join("omafiles/views.toml");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match toml::from_str(&text) {
            Ok(directories) => Self { directories },
            Err(err) => {
                eprintln!("omafiles: {}: {err}", path.display());
                Self::default()
            }
        }
    }

    /// Persist atomically: temp + `sync_all` + rename, like `places.toml`.
    pub fn save(&self, config_dir: &Path) -> std::io::Result<()> {
        let body = toml::to_string_pretty(&self.directories).map_err(std::io::Error::other)?;
        let dir = config_dir.join("omafiles");
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join("views.toml.tmp");
        {
            let mut file = std::fs::File::create(&tmp)?;
            use std::io::Write as _;
            file.write_all(body.as_bytes())?;
            file.sync_all()?;
        }
        std::fs::rename(&tmp, dir.join("views.toml"))
    }

    /// How `path` is laid out: what was saved for it, or the defaults.
    pub fn get(&self, path: &Path) -> DirectoryView {
        self.directories
            .get(&key(path))
            .cloned()
            .unwrap_or_default()
    }

    /// Record `path`'s layout. Back at the defaults, the directory leaves
    /// the file rather than pinning a copy of them.
    pub fn set(&mut self, path: &Path, view: DirectoryView) {
        if view == DirectoryView::default() {
            self.directories.remove(&key(path));
        } else {
            self.directories.insert(key(path), view);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.directories.is_empty()
    }
}

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("omafiles-views-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_unknown_directory_is_the_defaults() {
        let views = Views::default();
        assert_eq!(views.get(Path::new("/nowhere")), DirectoryView::default());
        assert_eq!(views.get(Path::new("/nowhere")).sort(), Sort::default());
    }

    #[test]
    fn a_saved_file_round_trips_per_directory() {
        let dir = scratch("roundtrip");
        let mut views = Views::default();
        let downloads = DirectoryView {
            sort: SortKey::Age,
            descending: false,
            size_width: Some(96),
            age_width: None,
        };
        views.set(Path::new("/home/me/Downloads"), downloads.clone());
        views.set(
            Path::new("/home/me/src"),
            DirectoryView {
                sort: SortKey::Name,
                descending: true,
                ..Default::default()
            },
        );
        views.save(&dir).unwrap();

        let text = std::fs::read_to_string(dir.join("omafiles/views.toml")).unwrap();
        assert!(text.contains("[\"/home/me/Downloads\"]"), "{text}");
        assert!(text.contains("sort = \"age\""), "{text}");
        assert!(
            !text.contains("age_width"),
            "an unset width is not written: {text}"
        );

        let loaded = Views::load(&dir);
        assert_eq!(loaded, views);
        assert_eq!(loaded.get(Path::new("/home/me/Downloads")), downloads);
        assert_eq!(
            loaded.get(Path::new("/home/me/other")),
            DirectoryView::default()
        );
    }

    #[test]
    fn back_at_the_defaults_a_directory_leaves_the_file() {
        let mut views = Views::default();
        let path = Path::new("/x");
        views.set(
            path,
            DirectoryView {
                sort: SortKey::Size,
                descending: true,
                ..Default::default()
            },
        );
        assert!(!views.is_empty());
        views.set(path, DirectoryView::default());
        assert!(views.is_empty());
    }

    #[test]
    fn a_broken_file_is_no_views_not_a_crash() {
        let dir = scratch("broken");
        std::fs::create_dir_all(dir.join("omafiles")).unwrap();
        std::fs::write(dir.join("omafiles/views.toml"), "not [ toml").unwrap();
        assert_eq!(Views::load(&dir), Views::default());
        assert!(
            dir.join("omafiles/views.toml").exists(),
            "left for the user to fix"
        );
    }

    #[test]
    fn a_table_naming_only_the_sort_keeps_default_widths() {
        let dir = scratch("partial");
        std::fs::create_dir_all(dir.join("omafiles")).unwrap();
        std::fs::write(
            dir.join("omafiles/views.toml"),
            "[\"/a\"]\nsort = \"size\"\n",
        )
        .unwrap();
        let view = Views::load(&dir).get(Path::new("/a"));
        assert_eq!(view.sort, SortKey::Size);
        assert!(!view.descending);
        assert_eq!(view.size_width, None);
    }
}
