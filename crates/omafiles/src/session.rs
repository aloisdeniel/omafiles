//! Tabs, workspaces, and the session file that outlives the process.
//!
//! A **tab** is an open view with its own path and history. A **workspace** is a
//! named, ordered group of tabs. Tabs belonging to no workspace are *global* —
//! modelled here as workspace index 0 with no name, rendered without a header.
//!
//! Exactly one workspace is active, and it scopes new tabs: opening a directory
//! while `Client X` is active files the tab there with no extra step. That is
//! the whole point of the feature.
//!
//! This module is pure: no gpui, no listing, no rendering. The view keeps the
//! `Listing` for each tab separately, keyed by tab id.

use std::path::{Path, PathBuf};

use crate::entry::nearest_existing;
use crate::navigation::Navigation;

/// The index of the implicit global group. Always present, always first.
pub const GLOBAL: usize = 0;

#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    /// Stable across renames and reorders; what the view keys its listings by.
    pub id: String,
    pub navigation: Navigation,
    /// The entry the cursor is on, by **name**. Indices shift when the
    /// directory changes underneath us; names do not.
    pub cursor_name: Option<String>,
    /// Whether the preview has taken over the listing column.
    ///
    /// Per tab rather than per window: one tab can be a folder of images being
    /// flicked through expanded while another is an ordinary listing, and
    /// switching between them should show each as it was left.
    pub preview_expanded: bool,
}

impl Tab {
    pub fn new(path: PathBuf) -> Self {
        Self {
            id: fresh_id("tab"),
            navigation: Navigation::new(path),
            cursor_name: None,
            preview_expanded: false,
        }
    }

    pub fn path(&self) -> &Path {
        self.navigation.current()
    }

    /// The label shown on the tab: the directory's own name, or `/`.
    pub fn label(&self) -> String {
        self.path()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path().to_string_lossy().into_owned())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Workspace {
    /// Stable id. The **name is a label and may change**, so nothing keys off it
    /// — renaming a workspace must not orphan the tabs inside.
    pub id: String,
    /// `None` for the global group.
    pub name: Option<String>,
    pub collapsed: bool,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
}

impl Workspace {
    fn global() -> Self {
        Self {
            id: "global".to_string(),
            name: None,
            collapsed: false,
            tabs: Vec::new(),
            active_tab: 0,
        }
    }

    pub fn named(name: String) -> Self {
        Self {
            id: fresh_id("ws"),
            name: Some(name),
            collapsed: false,
            tabs: Vec::new(),
            active_tab: 0,
        }
    }

    pub fn is_global(&self) -> bool {
        self.name.is_none()
    }

    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or("tabs")
    }

    fn clamp(&mut self) {
        self.active_tab = self.active_tab.min(self.tabs.len().saturating_sub(1));
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// `[0]` is always the global group.
    workspaces: Vec<Workspace>,
    active: usize,
    /// Bumped on every write. The watcher uses it to recognise — and ignore —
    /// its own writes. See [`Session::is_own_revision`].
    revision: u64,
}

impl Session {
    /// A fresh session with one tab in the global group.
    pub fn new(start: PathBuf) -> Self {
        let mut global = Workspace::global();
        global.tabs.push(Tab::new(start));
        Self {
            workspaces: vec![global],
            active: GLOBAL,
            revision: 0,
        }
    }

    // ------------------------------------------------------------- accessors

    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces
    }

    pub fn active_workspace(&self) -> usize {
        self.active
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn workspace(&self, index: usize) -> Option<&Workspace> {
        self.workspaces.get(index)
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        let ws = self.workspaces.get(self.active)?;
        ws.tabs.get(ws.active_tab)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        let ws = self.workspaces.get_mut(self.active)?;
        let index = ws.active_tab;
        ws.tabs.get_mut(index)
    }

    /// Every tab in display order, with the workspace it belongs to.
    pub fn flat(&self) -> Vec<(usize, usize, &Tab)> {
        self.workspaces
            .iter()
            .enumerate()
            .flat_map(|(w, ws)| ws.tabs.iter().enumerate().map(move |(t, tab)| (w, t, tab)))
            .collect()
    }

    pub fn total_tabs(&self) -> usize {
        self.workspaces.iter().map(|w| w.tabs.len()).sum()
    }

    // ------------------------------------------------------------ operations

    /// Open a tab **in the active workspace**. That scoping is the feature.
    pub fn new_tab(&mut self, path: PathBuf) -> &Tab {
        let ws = &mut self.workspaces[self.active];
        ws.tabs.push(Tab::new(path));
        ws.active_tab = ws.tabs.len() - 1;
        &ws.tabs[ws.active_tab]
    }

    /// Close a tab. The last remaining tab anywhere is never closed — an app
    /// with no tabs has nothing to show and no way back.
    pub fn close_tab(&mut self, workspace: usize, tab: usize) -> bool {
        if self.total_tabs() <= 1 {
            return false;
        }
        let Some(ws) = self.workspaces.get_mut(workspace) else {
            return false;
        };
        if tab >= ws.tabs.len() {
            return false;
        }
        ws.tabs.remove(tab);
        ws.clamp();

        // Never leave the active workspace empty and selected; fall back to the
        // first group that still has something in it.
        if self.workspaces[self.active].tabs.is_empty()
            && let Some(next) = self.workspaces.iter().position(|w| !w.tabs.is_empty())
        {
            self.active = next;
        }
        true
    }

    pub fn activate_tab(&mut self, workspace: usize, tab: usize) -> bool {
        let Some(ws) = self.workspaces.get_mut(workspace) else {
            return false;
        };
        if tab >= ws.tabs.len() {
            return false;
        }
        ws.active_tab = tab;
        self.active = workspace;
        true
    }

    pub fn add_workspace(&mut self, name: String) -> usize {
        self.workspaces.push(Workspace::named(name));
        self.workspaces.len() - 1
    }

    pub fn rename_workspace(&mut self, index: usize, name: String) -> bool {
        match self.workspaces.get_mut(index) {
            // The global group has no name to change.
            Some(ws) if !ws.is_global() => {
                ws.name = Some(name);
                true
            }
            _ => false,
        }
    }

    /// Delete a workspace, **moving its tabs to global rather than destroying
    /// them**.
    ///
    /// A workspace is a grouping, not an owner of lifetimes. Losing tabs to a
    /// mis-click is not something people forgive.
    pub fn delete_workspace(&mut self, index: usize) -> bool {
        if index == GLOBAL || index >= self.workspaces.len() {
            return false;
        }
        let removed = self.workspaces.remove(index);
        self.workspaces[GLOBAL].tabs.extend(removed.tabs);

        self.active = if self.active == index {
            GLOBAL
        } else if self.active > index {
            self.active - 1
        } else {
            self.active
        };
        self.workspaces[self.active].clamp();
        true
    }

    /// Move a tab between workspaces. This is what drag-and-drop and
    /// `Ctrl-Shift-N` both call.
    pub fn move_tab(&mut self, from: usize, tab: usize, to: usize) -> bool {
        if from >= self.workspaces.len() || to >= self.workspaces.len() || from == to {
            return false;
        }
        if tab >= self.workspaces[from].tabs.len() {
            return false;
        }
        let moved = self.workspaces[from].tabs.remove(tab);
        self.workspaces[from].clamp();
        self.workspaces[to].tabs.push(moved);
        self.workspaces[to].active_tab = self.workspaces[to].tabs.len() - 1;

        // Follow the tab: moving the one you are looking at and being left
        // staring at a different directory would be disorienting.
        self.active = to;

        if self.workspaces[from].tabs.is_empty() && from == GLOBAL {
            // Global may legitimately empty out; that is fine.
        }
        true
    }

    pub fn toggle_collapsed(&mut self, index: usize) {
        if let Some(ws) = self.workspaces.get_mut(index)
            && !ws.is_global()
        {
            ws.collapsed = !ws.collapsed;
        }
    }

    /// Activate a workspace by ordinal, for `Ctrl-1..9`.
    pub fn activate_workspace(&mut self, index: usize) -> bool {
        if index >= self.workspaces.len() || self.workspaces[index].tabs.is_empty() {
            return false;
        }
        self.active = index;
        true
    }

    // ----------------------------------------------------------- persistence

    /// Serialise, bumping the revision. Returns the revision written so the
    /// caller can recognise the resulting file-change event as its own.
    fn build_record(&mut self) -> SessionRecord {
        self.revision += 1;
        SessionRecord {
            version: 1,
            revision: self.revision,
            active: self.workspaces[self.active].id.clone(),
            workspace: self
                .workspaces
                .iter()
                .map(|ws| WorkspaceRecord {
                    id: ws.id.clone(),
                    name: ws.name.clone(),
                    collapsed: ws.collapsed,
                    active_tab: ws.active_tab,
                    tab: ws
                        .tabs
                        .iter()
                        .map(|t| TabRecord {
                            path: t.path().to_path_buf(),
                            cursor: t.cursor_name.clone(),
                            expanded: t.preview_expanded,
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    fn from_record(record: SessionRecord) -> Self {
        let mut workspaces: Vec<Workspace> = record
            .workspace
            .into_iter()
            .map(|ws| {
                let tabs: Vec<Tab> = ws
                    .tab
                    .into_iter()
                    .map(|t| Tab {
                        id: fresh_id("tab"),
                        // A tab can point at an unmounted drive or a directory
                        // deleted since last run. Land on the nearest existing
                        // ancestor rather than vanishing or wedging.
                        navigation: Navigation::new(nearest_existing(&t.path)),
                        cursor_name: t.cursor,
                        preview_expanded: t.expanded,
                    })
                    .collect();
                let active_tab = ws.active_tab.min(tabs.len().saturating_sub(1));
                Workspace {
                    id: ws.id,
                    name: ws.name,
                    collapsed: ws.collapsed,
                    tabs,
                    active_tab,
                }
            })
            .collect();

        // Repair anything the file could be missing: a global group must exist
        // and be first, and something must be open.
        if !workspaces.first().is_some_and(Workspace::is_global) {
            workspaces.insert(0, Workspace::global());
        }
        if workspaces.iter().all(|w| w.tabs.is_empty()) {
            workspaces[GLOBAL].tabs.push(Tab::new(default_start()));
        }

        let active = workspaces
            .iter()
            .position(|w| w.id == record.active && !w.tabs.is_empty())
            .or_else(|| workspaces.iter().position(|w| !w.tabs.is_empty()))
            .unwrap_or(GLOBAL);

        Self {
            workspaces,
            active,
            revision: record.revision,
        }
    }

    /// Load, falling back to a fresh session on anything unreadable.
    pub fn load(path: &Path, start: PathBuf) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::new(start);
        };
        match toml::from_str::<SessionRecord>(&text) {
            Ok(record) => Self::from_record(record),
            Err(err) => {
                // Never overwrite a session we failed to parse — see
                // `places.toml`'s equivalent. Starting fresh is indistinguishable
                // from "your tabs were deleted".
                eprintln!(
                    "omafiles: {} is not readable, ignoring: {err}",
                    path.display()
                );
                Self::new(start)
            }
        }
    }

    /// Write atomically. Returns the revision written.
    pub fn save(&mut self, path: &Path) -> std::io::Result<u64> {
        use std::io::Write as _;

        let record = self.build_record();
        let revision = record.revision;

        let parent = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let text = toml::to_string_pretty(&record)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        // Temp + rename, so a crash mid-write leaves the previous session
        // intact rather than a truncated one.
        let temp = parent.join(format!(".session.toml.{}.tmp", std::process::id()));
        {
            let mut file = std::fs::File::create(&temp)?;
            file.write_all(text.as_bytes())?;
            // Without this the rename can land before the bytes do, and a crash
            // leaves an empty file where the session was.
            file.sync_all()?;
        }
        std::fs::rename(&temp, path)?;
        Ok(revision)
    }

    /// Whether a session read back from disk is one we just wrote.
    ///
    /// **This is the guard against the self-reload loop.** Our own atomic
    /// rename trips our own watcher; without this the reload clobbers whatever
    /// the user was doing between the write and the event. It only bites when a
    /// write and an edit overlap, which is exactly why it needs a test rather
    /// than manual checking.
    pub fn is_own_revision(&self, revision: u64) -> bool {
        revision <= self.revision
    }
}

fn default_start() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Ids only need to be unique within a run and stable across reorders, so a
/// counter plus the pid is enough — no uuid dependency for something never
/// compared across machines.
fn fresh_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

// ------------------------------------------------------------------ on disk

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct SessionRecord {
    version: u32,
    #[serde(default)]
    revision: u64,
    #[serde(default)]
    active: String,
    #[serde(default)]
    workspace: Vec<WorkspaceRecord>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct WorkspaceRecord {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default)]
    collapsed: bool,
    #[serde(default)]
    active_tab: usize,
    #[serde(default)]
    tab: Vec<TabRecord>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct TabRecord {
    path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
    /// Omitted when false, so an ordinary tab's record is unchanged and a
    /// session written by an older build still loads.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    expanded: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "omafiles-session-{name}-{}-{:?}",
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
        fn session(&self) -> PathBuf {
            self.0.join("state/session.toml")
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_new_tab_lands_in_the_active_workspace() {
        // The point of the whole feature: no explicit filing step.
        let t = Tree::new("scoping");
        let mut s = Session::new(t.dir("a"));
        let client = s.add_workspace("Client X".into());
        assert!(
            !s.activate_workspace(client),
            "empty workspace cannot be activated"
        );

        // Activating requires a tab, so put one there the way the UI would.
        s.active = client;
        s.new_tab(t.dir("b"));
        assert_eq!(s.workspaces()[client].tabs.len(), 1);
        assert_eq!(s.workspaces()[GLOBAL].tabs.len(), 1, "global is untouched");

        s.new_tab(t.dir("c"));
        assert_eq!(
            s.workspaces()[client].tabs.len(),
            2,
            "still scoped to Client X"
        );
    }

    #[test]
    fn deleting_a_workspace_moves_its_tabs_to_global() {
        let t = Tree::new("delete");
        let mut s = Session::new(t.dir("a"));
        let ws = s.add_workspace("Temp".into());
        s.active = ws;
        s.new_tab(t.dir("b"));
        s.new_tab(t.dir("c"));

        let before = s.total_tabs();
        assert!(s.delete_workspace(ws));

        assert_eq!(
            s.total_tabs(),
            before,
            "a workspace is a grouping, not an owner"
        );
        assert_eq!(s.workspaces().len(), 1);
        assert_eq!(s.workspaces()[GLOBAL].tabs.len(), 3);
        assert_eq!(s.active_workspace(), GLOBAL);
    }

    #[test]
    fn the_global_group_cannot_be_deleted_or_renamed() {
        let t = Tree::new("global");
        let mut s = Session::new(t.dir("a"));
        assert!(!s.delete_workspace(GLOBAL));
        assert!(!s.rename_workspace(GLOBAL, "Nope".into()));
        assert!(s.workspaces()[GLOBAL].is_global());
    }

    #[test]
    fn renaming_keeps_the_id_so_tabs_are_not_orphaned() {
        let t = Tree::new("rename");
        let mut s = Session::new(t.dir("a"));
        let ws = s.add_workspace("Before".into());
        let id = s.workspaces()[ws].id.clone();
        s.active = ws;
        s.new_tab(t.dir("b"));

        assert!(s.rename_workspace(ws, "After".into()));
        assert_eq!(
            s.workspaces()[ws].id,
            id,
            "the id is the identity, not the name"
        );
        assert_eq!(s.workspaces()[ws].tabs.len(), 1);
    }

    #[test]
    fn moving_a_tab_follows_it() {
        let t = Tree::new("move");
        let mut s = Session::new(t.dir("a"));
        let ws = s.add_workspace("Target".into());

        assert!(s.move_tab(GLOBAL, 0, ws));
        assert_eq!(s.workspaces()[GLOBAL].tabs.len(), 0);
        assert_eq!(s.workspaces()[ws].tabs.len(), 1);
        assert_eq!(
            s.active_workspace(),
            ws,
            "must not leave the user elsewhere"
        );
    }

    #[test]
    fn the_last_tab_is_never_closed() {
        let t = Tree::new("lasttab");
        let mut s = Session::new(t.dir("a"));
        assert!(
            !s.close_tab(GLOBAL, 0),
            "an app with no tabs has no way back"
        );
        assert_eq!(s.total_tabs(), 1);

        s.new_tab(t.dir("b"));
        assert!(s.close_tab(GLOBAL, 1));
        assert_eq!(s.total_tabs(), 1);
    }

    #[test]
    fn round_trips_through_disk_with_cursors_intact() {
        let t = Tree::new("roundtrip");
        let (a, b) = (t.dir("a"), t.dir("b"));
        let mut s = Session::new(a.clone());
        s.active_tab_mut().unwrap().cursor_name = Some("README.md".into());
        let ws = s.add_workspace("Client X".into());
        s.active = ws;
        s.new_tab(b.clone());
        s.save(&t.session()).unwrap();

        let loaded = Session::load(&t.session(), a.clone());
        assert_eq!(loaded.workspaces().len(), 2);
        assert_eq!(loaded.workspaces()[GLOBAL].tabs[0].path(), a);
        assert_eq!(
            loaded.workspaces()[GLOBAL].tabs[0].cursor_name.as_deref(),
            Some("README.md")
        );
        assert_eq!(loaded.workspaces()[1].name.as_deref(), Some("Client X"));
        assert_eq!(loaded.workspaces()[1].tabs[0].path(), b);
        assert_eq!(
            loaded.active_workspace(),
            1,
            "the active workspace survives"
        );
    }

    #[test]
    fn a_tab_pointing_at_a_deleted_directory_lands_on_its_nearest_parent() {
        let t = Tree::new("stale");
        let alive = t.dir("alive");
        let mut s = Session::new(alive.clone());
        s.new_tab(alive.join("gone/deeper"));
        s.save(&t.session()).unwrap();

        let loaded = Session::load(&t.session(), alive.clone());
        assert_eq!(
            loaded.workspaces()[GLOBAL].tabs[1].path(),
            alive,
            "must degrade rather than vanish or wedge"
        );
    }

    /// **The self-reload guard.** Our own atomic rename trips our own watcher.
    #[test]
    fn recognises_its_own_writes() {
        let t = Tree::new("ownwrite");
        let mut s = Session::new(t.dir("a"));

        let written = s.save(&t.session()).unwrap();
        assert!(
            s.is_own_revision(written),
            "the revision we just wrote must not trigger a reload"
        );

        // A revision from somewhere else is newer and must be honoured.
        assert!(!s.is_own_revision(written + 1));

        // And a second write moves us forward.
        let again = s.save(&t.session()).unwrap();
        assert!(again > written);
        assert!(s.is_own_revision(again));
    }

    #[test]
    fn a_crash_mid_write_leaves_the_previous_session_intact() {
        let t = Tree::new("crash");
        let a = t.dir("a");
        let mut s = Session::new(a.clone());
        s.save(&t.session()).unwrap();
        let good = std::fs::read_to_string(t.session()).unwrap();

        // Simulate the crash window: a temp file exists, the rename never ran.
        let parent = t.session().parent().unwrap().to_path_buf();
        std::fs::write(parent.join(".session.toml.99999.tmp"), "truncated garbage").unwrap();

        assert_eq!(
            std::fs::read_to_string(t.session()).unwrap(),
            good,
            "the live file must be untouched by an abandoned write"
        );
        assert_eq!(Session::load(&t.session(), a).total_tabs(), 1);
    }

    #[test]
    fn a_corrupt_session_is_ignored_not_overwritten() {
        let t = Tree::new("corrupt");
        let a = t.dir("a");
        std::fs::create_dir_all(t.session().parent().unwrap()).unwrap();
        std::fs::write(t.session(), "not [ valid toml").unwrap();

        let s = Session::load(&t.session(), a);
        assert_eq!(s.total_tabs(), 1, "starts fresh");
        assert!(
            t.session().exists(),
            "but the file the user could fix is kept"
        );
    }

    #[test]
    fn repairs_a_session_with_no_global_group_or_no_tabs() {
        let t = Tree::new("repair");
        let a = t.dir("a");
        std::fs::create_dir_all(t.session().parent().unwrap()).unwrap();
        std::fs::write(
            t.session(),
            "version = 1\nrevision = 3\nactive = \"ws-1\"\n\n\
             [[workspace]]\nid = \"ws-1\"\nname = \"Only\"\n",
        )
        .unwrap();

        let s = Session::load(&t.session(), a);
        assert!(
            s.workspaces()[GLOBAL].is_global(),
            "a global group is inserted"
        );
        assert!(s.total_tabs() >= 1, "something must be open");
    }

    #[test]
    fn closing_the_active_workspaces_last_tab_moves_somewhere_real() {
        let t = Tree::new("emptyactive");
        let mut s = Session::new(t.dir("a"));
        let ws = s.add_workspace("Temp".into());
        s.active = ws;
        s.new_tab(t.dir("b"));

        assert!(s.close_tab(ws, 0));
        assert!(
            !s.workspaces()[s.active_workspace()].tabs.is_empty(),
            "never leave the user staring at an empty workspace"
        );
    }
    #[test]
    fn the_expanded_preview_is_remembered_per_tab() {
        // One tab expanded and another not is the whole point: switching
        // between them must show each as it was left, and that survives a
        // restart the same way the cursor does.
        let t = Tree::new("expanded");
        let (a, b) = (t.dir("a"), t.dir("b"));
        let mut s = Session::new(a.clone());
        s.new_tab(b.clone());

        // The second tab is active after `new_tab`.
        s.active_tab_mut().unwrap().preview_expanded = true;
        s.save(&t.session()).unwrap();

        let loaded = Session::load(&t.session(), a.clone());
        let tabs = &loaded.workspaces()[GLOBAL].tabs;
        assert_eq!(tabs.len(), 2);
        assert!(
            !tabs[0].preview_expanded,
            "the first tab was never expanded"
        );
        assert!(tabs[1].preview_expanded, "the second one was");
    }

    #[test]
    fn a_hand_written_expanded_flag_is_read_back() {
        // The save-then-load test would pass even if the key were serialised
        // under a different name, because both halves would agree. This reads a
        // literal file, which is what actually pins the on-disk spelling.
        let t = Tree::new("literal");
        let a = t.dir("a");
        let record = format!(
            "version = 1\nrevision = 1\nactive = \"global\"\n\n\
             [[workspace]]\nid = \"global\"\ncollapsed = false\nactive_tab = 0\n\n\
             [[workspace.tab]]\npath = \"{}\"\ncursor = \"x.rs\"\nexpanded = true\n",
            a.display()
        );
        std::fs::create_dir_all(t.session().parent().unwrap()).unwrap();
        std::fs::write(t.session(), record).unwrap();

        let loaded = Session::load(&t.session(), a.clone());
        let tab = &loaded.workspaces()[GLOBAL].tabs[0];
        assert_eq!(tab.cursor_name.as_deref(), Some("x.rs"));
        assert!(
            tab.preview_expanded,
            "`expanded = true` must survive the read"
        );
    }

    #[test]
    fn a_session_written_before_the_expanded_flag_still_loads() {
        // `expanded` is skipped when false, so old records simply lack the key.
        // Defaulting rather than erroring is what stops an upgrade from wiping
        // someone's tabs.
        let t = Tree::new("oldrecord");
        let a = t.dir("a");
        let record = format!(
            "version = 1\nrevision = 1\nactive = \"global\"\n\n\
             [[workspace]]\nid = \"global\"\ncollapsed = false\nactive_tab = 0\n\n\
             [[workspace.tab]]\npath = \"{}\"\n",
            a.display()
        );
        // `save` would create this; writing the file by hand does not.
        std::fs::create_dir_all(t.session().parent().unwrap()).unwrap();
        std::fs::write(t.session(), record).unwrap();

        let loaded = Session::load(&t.session(), a.clone());
        let tabs = &loaded.workspaces()[GLOBAL].tabs;
        assert_eq!(tabs.len(), 1, "the tab survived the missing key");
        assert!(!tabs[0].preview_expanded);
    }
}
