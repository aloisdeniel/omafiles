//! The sidebar's navigation shortcuts.
//!
//! Two groups. **Places** are derived from the system — home, the XDG user
//! directories, and `~/.config` — and are not editable. **Pins** are the user's
//! own, persisted to `~/.config/omafiles/places.toml`.
//!
//! A place is a shortcut, not a tab: activating one navigates the current view.
//! Tabs arrive in M5.

use std::path::{Path, PathBuf};

/// Where a shortcut came from, which decides whether it can be edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// `$HOME`.
    Home,
    /// An `XDG_*_DIR` from `user-dirs.dirs`.
    Xdg,
    /// `~/.config`, which is not an XDG user directory but is where an Omarchy
    /// user spends a lot of time.
    Config,
    /// The user pinned it. The only kind that can be reordered or removed.
    Pinned,
}

impl Origin {
    pub fn is_editable(self) -> bool {
        matches!(self, Origin::Pinned)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub label: String,
    pub path: PathBuf,
    pub origin: Origin,
}

/// The sidebar's contents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Places {
    system: Vec<Place>,
    pinned: Vec<Place>,
}

/// The order the well-known XDG directories are shown in.
///
/// Explicit rather than alphabetical, because alphabetical puts Videos above
/// Documents and nobody thinks of them that way. Anything not listed here is a
/// non-standard `XDG_*_DIR` and is appended after, sorted, rather than dropped —
/// this machine has an `XDG_PROJECTS_DIR`, and silently hiding it would be wrong.
const XDG_ORDER: &[&str] = &[
    "DESKTOP",
    "DOWNLOAD",
    "DOCUMENTS",
    "PICTURES",
    "MUSIC",
    "VIDEOS",
    "PUBLICSHARE",
    "TEMPLATES",
];

impl Places {
    /// Build the system half and load the user's pins.
    pub fn load(home: &Path, config_dir: &Path) -> Self {
        Self {
            system: system_places(home, config_dir),
            pinned: PinFile::load(&pins_path(config_dir)).into_places(),
        }
    }

    pub fn system(&self) -> &[Place] {
        &self.system
    }

    pub fn pinned(&self) -> &[Place] {
        &self.pinned
    }

    /// Every place, system then pinned, for cursor movement across the sidebar.
    pub fn all(&self) -> impl Iterator<Item = &Place> {
        self.system.iter().chain(self.pinned.iter())
    }

    pub fn len(&self) -> usize {
        self.system.len() + self.pinned.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, index: usize) -> Option<&Place> {
        self.all().nth(index)
    }

    /// The index of the place matching `path` exactly.
    ///
    /// Exact rather than "is an ancestor of": highlighting Home the entire time
    /// you are anywhere under it tells you nothing.
    pub fn index_of_path(&self, path: &Path) -> Option<usize> {
        self.all().position(|p| p.path == path)
    }

    /// Pin `path`. Returns false if it is already pinned or is not a directory.
    pub fn pin(&mut self, path: &Path) -> bool {
        if !path.is_dir() || self.pinned.iter().any(|p| p.path == path) {
            return false;
        }
        self.pinned.push(Place {
            label: label_for(path),
            path: path.to_path_buf(),
            origin: Origin::Pinned,
        });
        true
    }

    /// Remove the pin at `index` **within the pinned list**.
    pub fn unpin(&mut self, index: usize) -> bool {
        if index >= self.pinned.len() {
            return false;
        }
        self.pinned.remove(index);
        true
    }

    /// Move a pin up or down within the pinned list. Clamped, not wrapping —
    /// a reorder that silently jumps from top to bottom is disorienting.
    pub fn move_pin(&mut self, index: usize, delta: isize) -> Option<usize> {
        if index >= self.pinned.len() {
            return None;
        }
        let target = (index as isize + delta).clamp(0, self.pinned.len() as isize - 1) as usize;
        if target == index {
            return None;
        }
        let pin = self.pinned.remove(index);
        self.pinned.insert(target, pin);
        Some(target)
    }

    /// Whether `index` (into [`Self::all`]) refers to a pin, and its position
    /// within the pinned list.
    pub fn pinned_index(&self, index: usize) -> Option<usize> {
        index
            .checked_sub(self.system.len())
            .filter(|i| *i < self.pinned.len())
    }

    pub fn save(&self, config_dir: &Path) -> std::io::Result<()> {
        PinFile::from_places(&self.pinned).save(&pins_path(config_dir))
    }
}

fn pins_path(config_dir: &Path) -> PathBuf {
    config_dir.join("omafiles/places.toml")
}

/// Home, the XDG user directories, and `~/.config`.
fn system_places(home: &Path, config_dir: &Path) -> Vec<Place> {
    let mut places = vec![Place {
        label: "Home".to_string(),
        path: home.to_path_buf(),
        origin: Origin::Home,
    }];

    let dirs = read_user_dirs(&config_dir.join("user-dirs.dirs"), home);

    let mut known: Vec<(usize, &str, PathBuf)> = Vec::new();
    let mut extra: Vec<(String, PathBuf)> = Vec::new();
    for (name, path) in dirs {
        match XDG_ORDER.iter().position(|k| *k == name) {
            Some(rank) => known.push((rank, XDG_ORDER[rank], path)),
            None => extra.push((name, path)),
        }
    }
    known.sort_by_key(|(rank, _, _)| *rank);
    extra.sort();

    for (_, _, path) in known {
        push_if_useful(&mut places, home, path, Origin::Xdg);
    }
    for (_, path) in extra {
        push_if_useful(&mut places, home, path, Origin::Xdg);
    }

    push_if_useful(&mut places, home, config_dir.to_path_buf(), Origin::Config);
    places
}

/// Add a place unless it is useless: missing, or `$HOME` again.
///
/// Both happen in practice. On this machine `XDG_DESKTOP_DIR` and
/// `XDG_TEMPLATES_DIR` both point at `$HOME`, so a naive sidebar shows
/// "Desktop" and "Templates" as two more copies of Home.
fn push_if_useful(places: &mut Vec<Place>, home: &Path, path: PathBuf, origin: Origin) {
    if path == home || !path.is_dir() || places.iter().any(|p| p.path == path) {
        return;
    }
    places.push(Place {
        label: label_for(&path),
        path,
        origin,
    });
}

/// A place's label is its directory's own name.
///
/// Derived from the path rather than from the `XDG_*` key so a localised setup
/// shows what is actually on disk — `Téléchargements`, not "Downloads".
fn label_for(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Parse `user-dirs.dirs`, which is shell syntax: `XDG_NAME_DIR="$HOME/x"`.
fn read_user_dirs(path: &Path, home: &Path) -> Vec<(String, PathBuf)> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };

    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            let name = key.trim().strip_prefix("XDG_")?.strip_suffix("_DIR")?;

            let value = value.trim().trim_matches('"');
            let expanded = match value.strip_prefix("$HOME") {
                Some(rest) => home.join(rest.trim_start_matches('/')),
                None => PathBuf::from(value),
            };
            (!name.is_empty()).then(|| (name.to_string(), expanded))
        })
        .collect()
}

// ------------------------------------------------------------------ pin file

/// The on-disk shape of `places.toml`.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct PinFile {
    #[serde(default = "one")]
    version: u32,
    #[serde(default, rename = "pin")]
    pins: Vec<PinEntry>,
}

fn one() -> u32 {
    1
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct PinEntry {
    path: PathBuf,
    /// Optional: a pin the user renamed. Absent means "use the directory name",
    /// so a renamed directory updates its own label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
}

impl PinFile {
    fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match toml::from_str(&text) {
            Ok(parsed) => parsed,
            Err(err) => {
                // Never discard the user's file on a parse error — starting
                // empty looks identical to "your pins were deleted".
                eprintln!(
                    "omafiles: {} is not readable, ignoring: {err}",
                    path.display()
                );
                Self::default()
            }
        }
    }

    fn into_places(self) -> Vec<Place> {
        self.pins
            .into_iter()
            .map(|pin| Place {
                label: pin.label.unwrap_or_else(|| label_for(&pin.path)),
                path: pin.path,
                origin: Origin::Pinned,
            })
            .collect()
    }

    fn from_places(places: &[Place]) -> Self {
        Self {
            version: 1,
            pins: places
                .iter()
                .map(|p| PinEntry {
                    // Only persist a label that differs from the directory name,
                    // so renaming the directory keeps the sidebar honest.
                    label: (p.label != label_for(&p.path)).then(|| p.label.clone()),
                    path: p.path.clone(),
                })
                .collect(),
        }
    }

    /// Write atomically: a temp file in the same directory, then rename.
    ///
    /// A crash mid-write must leave the previous file intact rather than a
    /// truncated one — the same discipline `omarchy-theme-set` uses, and what
    /// M5's session file will need.
    fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write as _;

        let parent = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;

        let text = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let temp = parent.join(format!(".places.toml.{}.tmp", std::process::id()));
        {
            let mut file = std::fs::File::create(&temp)?;
            file.write_all(text.as_bytes())?;
            // Without this the rename can land before the bytes do, which on a
            // crash leaves an empty file where the pins were.
            file.sync_all()?;
        }
        std::fs::rename(&temp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "omafiles-places-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn dir(&self, rel: &str) -> PathBuf {
            let p = self.0.join(rel);
            std::fs::create_dir_all(&p).unwrap();
            p
        }
        fn write(&self, rel: &str, text: &str) -> PathBuf {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text).unwrap();
            p
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A fixture home with a realistic `user-dirs.dirs`, including the two traps
    /// this machine actually has.
    fn fixture(name: &str) -> (Tree, PathBuf, PathBuf) {
        let t = Tree::new(name);
        let home = t.dir("home");
        let config = t.dir("home/.config");
        t.dir("home/Downloads");
        t.dir("home/Documents");
        t.dir("home/Pictures");
        t.dir("home/Projects");

        t.write(
            "home/.config/user-dirs.dirs",
            "# a comment\n\
             XDG_DESKTOP_DIR=\"$HOME/\"\n\
             XDG_DOWNLOAD_DIR=\"$HOME/Downloads\"\n\
             XDG_DOCUMENTS_DIR=\"$HOME/Documents\"\n\
             XDG_PICTURES_DIR=\"$HOME/Pictures\"\n\
             XDG_MUSIC_DIR=\"$HOME/Music\"\n\
             XDG_PROJECTS_DIR=\"$HOME/Projects\"\n",
        );
        (t, home, config)
    }

    #[test]
    fn skips_xdg_dirs_that_are_just_home_again() {
        let (_t, home, config) = fixture("home-dupe");
        let places = Places::load(&home, &config);

        // XDG_DESKTOP_DIR="$HOME/" would otherwise appear as a second Home.
        assert_eq!(
            places.system().iter().filter(|p| p.path == home).count(),
            1,
            "Home must appear exactly once"
        );
    }

    #[test]
    fn skips_xdg_dirs_that_do_not_exist() {
        let (_t, home, config) = fixture("missing");
        let places = Places::load(&home, &config);
        // Music is declared but was never created.
        assert!(!places.system().iter().any(|p| p.label == "Music"));
    }

    #[test]
    fn keeps_non_standard_xdg_entries() {
        let (_t, home, config) = fixture("nonstandard");
        let places = Places::load(&home, &config);
        assert!(
            places.system().iter().any(|p| p.label == "Projects"),
            "XDG_PROJECTS_DIR is non-standard but real; dropping it would be wrong"
        );
    }

    #[test]
    fn orders_well_known_directories_deliberately() {
        let (_t, home, config) = fixture("order");
        let places = Places::load(&home, &config);
        let labels: Vec<&str> = places.system().iter().map(|p| p.label.as_str()).collect();

        assert_eq!(labels[0], "Home");
        let downloads = labels.iter().position(|l| *l == "Downloads").unwrap();
        let documents = labels.iter().position(|l| *l == "Documents").unwrap();
        let projects = labels.iter().position(|l| *l == "Projects").unwrap();
        assert!(downloads < documents, "declared order, not alphabetical");
        assert!(
            documents < projects,
            "non-standard entries come after known ones"
        );
        assert_eq!(labels.last().copied(), Some(".config"));
    }

    #[test]
    fn labels_come_from_the_directory_so_localisation_survives() {
        let t = Tree::new("localised");
        let home = t.dir("home");
        t.dir("home/.config");
        t.dir("home/Téléchargements");
        t.write(
            "home/.config/user-dirs.dirs",
            "XDG_DOWNLOAD_DIR=\"$HOME/Téléchargements\"\n",
        );

        let places = Places::load(&home, &t.0.join("home/.config"));
        assert!(
            places.system().iter().any(|p| p.label == "Téléchargements"),
            "the label must be what is on disk, not the XDG key"
        );
    }

    #[test]
    fn handles_absolute_paths_in_user_dirs() {
        let t = Tree::new("absolute");
        let home = t.dir("home");
        let config = t.dir("home/.config");
        let elsewhere = t.dir("mnt/media");
        t.write(
            "home/.config/user-dirs.dirs",
            &format!("XDG_VIDEOS_DIR=\"{}\"\n", elsewhere.display()),
        );

        let places = Places::load(&home, &config);
        assert!(places.system().iter().any(|p| p.path == elsewhere));
    }

    #[test]
    fn pins_round_trip_through_disk() {
        let (_t, home, config) = fixture("roundtrip");
        let mut places = Places::load(&home, &config);

        assert!(places.pin(&home.join("Downloads")));
        assert!(places.pin(&home.join("Projects")));
        assert!(!places.pin(&home.join("Downloads")), "no duplicate pins");
        assert!(
            !places.pin(&home.join("nope")),
            "cannot pin a missing directory"
        );
        places.save(&config).unwrap();

        let reloaded = Places::load(&home, &config);
        let labels: Vec<&str> = reloaded.pinned().iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["Downloads", "Projects"]);
    }

    #[test]
    fn reordering_is_clamped_not_wrapping() {
        let (_t, home, config) = fixture("reorder");
        let mut places = Places::load(&home, &config);
        places.pin(&home.join("Downloads"));
        places.pin(&home.join("Documents"));
        places.pin(&home.join("Projects"));

        assert_eq!(places.move_pin(2, -1), Some(1));
        let labels: Vec<&str> = places.pinned().iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["Downloads", "Projects", "Documents"]);

        // At the top, moving up does nothing rather than jumping to the bottom.
        assert_eq!(places.move_pin(0, -1), None);
        assert_eq!(places.move_pin(2, 1), None);
    }

    #[test]
    fn unpinning_leaves_the_rest_in_order() {
        let (_t, home, config) = fixture("unpin");
        let mut places = Places::load(&home, &config);
        places.pin(&home.join("Downloads"));
        places.pin(&home.join("Documents"));

        assert!(places.unpin(0));
        assert!(!places.unpin(5), "out of range must not panic");
        assert_eq!(places.pinned().len(), 1);
        assert_eq!(places.pinned()[0].label, "Documents");
    }

    #[test]
    fn a_corrupt_places_file_is_ignored_not_overwritten() {
        let (_t, home, config) = fixture("corrupt");
        let pins = pins_path(&config);
        std::fs::create_dir_all(pins.parent().unwrap()).unwrap();
        std::fs::write(&pins, "this is not [ valid toml").unwrap();

        let places = Places::load(&home, &config);
        assert!(
            places.pinned().is_empty(),
            "loads empty rather than failing"
        );
        // The bad file must still be on disk — silently replacing it would
        // destroy pins that a hand-edit typo could otherwise be fixed in.
        assert!(pins.exists());
    }

    #[test]
    fn only_a_renamed_label_is_persisted() {
        let (_t, home, config) = fixture("labels");
        let mut places = Places::load(&home, &config);
        places.pin(&home.join("Downloads"));
        places.save(&config).unwrap();

        let text = std::fs::read_to_string(pins_path(&config)).unwrap();
        // Match the TOML key, not the substring: the fixture path itself
        // contains "labels", which a naive `contains` would hit.
        assert!(
            !text.lines().any(|l| l.trim_start().starts_with("label")),
            "a label matching the directory name is redundant on disk:\n{text}"
        );

        // And a genuinely renamed pin *is* written.
        let mut renamed = Places::load(&home, &config);
        renamed.pinned[0].label = "Grabs".to_string();
        renamed.save(&config).unwrap();
        let text = std::fs::read_to_string(pins_path(&config)).unwrap();
        assert!(
            text.lines().any(|l| l.trim_start().starts_with("label")),
            "{text}"
        );
        assert_eq!(Places::load(&home, &config).pinned()[0].label, "Grabs");
    }

    #[test]
    fn pinned_index_maps_from_the_combined_list() {
        let (_t, home, config) = fixture("indexing");
        let mut places = Places::load(&home, &config);
        places.pin(&home.join("Downloads"));

        let system = places.system().len();
        assert_eq!(places.pinned_index(system), Some(0));
        assert_eq!(places.pinned_index(system.saturating_sub(1)), None);
        assert_eq!(places.pinned_index(system + 9), None);
    }

    #[test]
    fn the_current_directory_matches_exactly_not_by_ancestry() {
        let (_t, home, config) = fixture("match");
        let places = Places::load(&home, &config);

        assert_eq!(places.index_of_path(&home), Some(0));
        assert_eq!(
            places.index_of_path(&home.join("Downloads/deeper")),
            None,
            "a descendant must not light up its ancestor"
        );
    }
}
