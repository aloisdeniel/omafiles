//! Where we are, where we have been, and where the cursor was when we left.
//!
//! The behaviour worth naming: going *up* out of a directory puts the cursor on
//! the directory you just came from, not at the top of the list. Without that,
//! navigating up out of a deep folder loses your place and you have to hunt for
//! it — the single most common navigation in a file manager made annoying.
//!
//! Pure logic, no filesystem access, no gpui. M5 makes one of these per tab.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A back/forward stack plus per-directory cursor memory.
#[derive(Debug, Clone, PartialEq)]
pub struct Navigation {
    current: PathBuf,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    /// The entry name the cursor was on, per directory we have visited.
    ///
    /// Keyed by name rather than index because the listing changes underneath
    /// us — a file added or removed while we were away shifts every index after
    /// it, and restoring index 7 would land somewhere arbitrary.
    cursor_memory: HashMap<PathBuf, String>,
    /// Bound on `cursor_memory`, so a long session browsing thousands of
    /// directories does not grow without limit.
    memory_limit: usize,
}

impl Navigation {
    const DEFAULT_MEMORY_LIMIT: usize = 512;

    pub fn new(start: PathBuf) -> Self {
        Self {
            current: start,
            back: Vec::new(),
            forward: Vec::new(),
            cursor_memory: HashMap::new(),
            memory_limit: Self::DEFAULT_MEMORY_LIMIT,
        }
    }

    pub fn current(&self) -> &Path {
        &self.current
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// Navigate to `path`, remembering where the cursor was.
    ///
    /// Navigating somewhere new discards the forward stack, as in a browser.
    pub fn go(&mut self, path: PathBuf, leaving_cursor: Option<&str>) {
        if path == self.current {
            return;
        }
        self.remember(leaving_cursor);
        self.back.push(std::mem::replace(&mut self.current, path));
        self.forward.clear();
    }

    /// Go to the parent, leaving the cursor on the directory we came from.
    ///
    /// Returns `false` at the filesystem root.
    pub fn go_up(&mut self, leaving_cursor: Option<&str>) -> bool {
        let Some(parent) = self.current.parent().map(Path::to_path_buf) else {
            return false;
        };
        // The name we are leaving, so the parent's cursor lands on it.
        let leaving_name = self
            .current
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());

        self.remember(leaving_cursor);
        let child = std::mem::replace(&mut self.current, parent);
        self.back.push(child);
        self.forward.clear();

        if let Some(name) = leaving_name {
            self.cursor_memory.insert(self.current.clone(), name);
        }
        true
    }

    pub fn go_back(&mut self, leaving_cursor: Option<&str>) -> bool {
        let Some(previous) = self.back.pop() else {
            return false;
        };
        self.remember(leaving_cursor);
        self.forward
            .push(std::mem::replace(&mut self.current, previous));
        true
    }

    pub fn go_forward(&mut self, leaving_cursor: Option<&str>) -> bool {
        let Some(next) = self.forward.pop() else {
            return false;
        };
        self.remember(leaving_cursor);
        self.back.push(std::mem::replace(&mut self.current, next));
        true
    }

    /// The entry name the cursor should land on in the current directory.
    pub fn remembered_cursor(&self) -> Option<&str> {
        self.cursor_memory.get(&self.current).map(String::as_str)
    }

    /// Name the entry the cursor lands on *here*, overriding what was
    /// remembered when this directory was last left — for a caller that just
    /// made something and wants it under the cursor.
    pub fn land_on(&mut self, name: &str) {
        self.remember(Some(name));
    }

    /// Record the cursor for the directory we are about to leave.
    fn remember(&mut self, cursor: Option<&str>) {
        let Some(name) = cursor else { return };

        if self.cursor_memory.len() >= self.memory_limit
            && !self.cursor_memory.contains_key(&self.current)
        {
            // Crude but bounded: drop an arbitrary entry rather than track LRU
            // for something whose only cost is re-finding your place once.
            if let Some(victim) = self.cursor_memory.keys().next().cloned() {
                self.cursor_memory.remove(&victim);
            }
        }
        self.cursor_memory
            .insert(self.current.clone(), name.to_string());
    }

    /// Path components, for the breadcrumb.
    pub fn breadcrumb(&self) -> Vec<String> {
        let home = std::env::var_os("HOME").map(PathBuf::from);

        // Render under-home paths as `~/…`, which is both shorter and how
        // people actually refer to them.
        if let Some(relative) = home
            .as_ref()
            .and_then(|h| self.current.strip_prefix(h).ok())
        {
            let mut parts = vec!["~".to_string()];
            parts.extend(
                relative
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned()),
            );
            return parts;
        }

        let mut parts = vec!["/".to_string()];
        parts.extend(
            self.current
                .components()
                .skip(1)
                .map(|c| c.as_os_str().to_string_lossy().into_owned()),
        );
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nav(path: &str) -> Navigation {
        Navigation::new(PathBuf::from(path))
    }

    #[test]
    fn going_up_puts_the_cursor_on_the_directory_we_left() {
        // The behaviour this module exists for.
        let mut n = nav("/home/alois/Documents/Github");
        assert!(n.go_up(Some("omafiles")));
        assert_eq!(n.current(), Path::new("/home/alois/Documents"));
        assert_eq!(
            n.remembered_cursor(),
            Some("Github"),
            "cursor must land on the child we came out of, not on the old cursor"
        );
    }

    #[test]
    fn back_and_forward_behave_like_a_browser() {
        let mut n = nav("/a");
        n.go("/b".into(), None);
        n.go("/c".into(), None);
        assert_eq!(n.current(), Path::new("/c"));

        assert!(n.go_back(None));
        assert_eq!(n.current(), Path::new("/b"));
        assert!(n.go_forward(None));
        assert_eq!(n.current(), Path::new("/c"));

        // Navigating somewhere new discards the forward stack.
        assert!(n.go_back(None));
        n.go("/d".into(), None);
        assert!(!n.can_go_forward());
    }

    #[test]
    fn refuses_to_go_above_the_root_or_back_from_the_start() {
        let mut n = nav("/");
        assert!(!n.go_up(None));
        assert_eq!(n.current(), Path::new("/"));
        assert!(!n.go_back(None));
        assert!(!n.go_forward(None));
    }

    #[test]
    fn navigating_to_where_we_already_are_is_a_no_op() {
        let mut n = nav("/a");
        n.go("/a".into(), Some("x"));
        assert!(!n.can_go_back(), "must not push a duplicate history entry");
    }

    #[test]
    fn cursor_memory_is_per_directory_and_survives_a_round_trip() {
        let mut n = nav("/a");
        n.go("/b".into(), Some("file-in-a"));
        assert_eq!(n.remembered_cursor(), None, "never been to /b");

        n.go_back(Some("file-in-b"));
        assert_eq!(n.remembered_cursor(), Some("file-in-a"));

        n.go_forward(None);
        assert_eq!(n.remembered_cursor(), Some("file-in-b"));
    }

    #[test]
    fn cursor_memory_is_bounded() {
        let mut n = nav("/start");
        n.memory_limit = 4;
        for i in 0..50 {
            n.go(PathBuf::from(format!("/dir{i}")), Some("something"));
        }
        assert!(
            n.cursor_memory.len() <= 4,
            "unbounded memory in a long session: {}",
            n.cursor_memory.len()
        );
    }

    #[test]
    fn breadcrumb_abbreviates_home() {
        // SAFETY: single-threaded test, and the value is restored immediately.
        let previous = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", "/home/alois") };

        let n = nav("/home/alois/Documents/Github");
        assert_eq!(n.breadcrumb(), ["~", "Documents", "Github"]);

        let n = nav("/usr/share/omarchy");
        assert_eq!(n.breadcrumb(), ["/", "usr", "share", "omarchy"]);

        match previous {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}
