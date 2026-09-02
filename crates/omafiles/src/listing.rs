//! Reading a directory into a sorted, filtered list of entries.
//!
//! Pure and synchronous. The view calls [`Listing::read`] on a background
//! thread and swaps the result in when it lands, so nothing here needs to know
//! about gpui.

use std::path::{Path, PathBuf};

use crate::entry::{Entry, Kind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub path: PathBuf,
    /// Everything in the directory, sorted. Hidden entries included — filtering
    /// is [`Listing::visible`]'s job, so toggling "show hidden" does not need a
    /// re-read from disk.
    entries: Vec<Entry>,
    /// Set when the directory could not be read at all. The path still shows,
    /// so the user can see *where* they are and navigate out.
    pub error: Option<String>,
}

impl Listing {
    /// Read `path`. Never fails — an unreadable directory yields an empty
    /// listing carrying the error, because a file manager that shows nothing
    /// and says nothing is worse than one that says "permission denied".
    pub fn read(path: &Path) -> Self {
        let path = path.to_path_buf();

        // std::fs rather than `ignore`: at depth 1 there is nothing to gain,
        // and `ignore`'s gitignore filtering is actively wrong here — a file
        // manager must show the files that are on disk. The gitignore-aware
        // walker belongs in search (M6), not in the listing.
        let read = match std::fs::read_dir(&path) {
            Ok(read) => read,
            Err(err) => {
                return Self {
                    path,
                    entries: Vec::new(),
                    error: Some(err.to_string()),
                };
            }
        };

        // A single unreadable entry must not lose the whole directory, so
        // per-entry errors are skipped rather than propagated.
        let mut entries: Vec<Entry> = read
            .filter_map(Result::ok)
            .map(|e| Entry::from_path(e.path()))
            .collect();
        entries.sort_by(Entry::sort_key_cmp);

        Self {
            path,
            entries,
            error: None,
        }
    }

    pub fn empty(path: PathBuf) -> Self {
        Self {
            path,
            entries: Vec::new(),
            error: None,
        }
    }

    /// Indices into [`Self::all`] that should be shown.
    ///
    /// Returns indices rather than references so the caller can keep a cursor
    /// that survives a hidden-files toggle.
    pub fn visible(&self, show_hidden: bool) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| show_hidden || !e.is_hidden())
            .map(|(i, _)| i)
            .collect()
    }

    pub fn all(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, index: usize) -> Option<&Entry> {
        self.entries.get(index)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Find an entry by file name. Used to restore the cursor after a refresh,
    /// where indices have shifted but the name has not.
    pub fn index_of_name(&self, name: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.name == name)
    }

    /// Counts for the status line: `(directories, files)`, visible only.
    pub fn counts(&self, show_hidden: bool) -> (usize, usize) {
        self.visible(show_hidden)
            .iter()
            .filter_map(|&i| self.entries.get(i))
            .fold((0, 0), |(dirs, files), e| {
                if e.kind.is_dir() {
                    (dirs + 1, files)
                } else {
                    (dirs, files + 1)
                }
            })
    }

    /// The parent directory, unless we are at the root.
    pub fn parent(&self) -> Option<PathBuf> {
        self.path.parent().map(Path::to_path_buf)
    }
}

/// Where a listing came from, for the empty state.
pub fn describe_empty(listing: &Listing, show_hidden: bool) -> &'static str {
    if listing.error.is_some() {
        "cannot read this directory"
    } else if listing.is_empty() {
        "empty directory"
    } else if listing.visible(show_hidden).is_empty() {
        "only hidden files here"
    } else {
        ""
    }
}

/// Whether `entry` can be entered.
pub fn is_navigable(entry: &Entry) -> bool {
    matches!(entry.kind, Kind::Directory)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "omafiles-listing-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn file(&self, name: &str, bytes: &[u8]) -> &Self {
            std::fs::write(self.0.join(name), bytes).unwrap();
            self
        }
        fn dir(&self, name: &str) -> &Self {
            std::fs::create_dir_all(self.0.join(name)).unwrap();
            self
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reads_and_sorts_directories_before_files() {
        let t = Tree::new("sort");
        t.file("b.txt", b"xx").dir("a_dir").file("a.txt", b"x");

        let listing = Listing::read(&t.0);
        let names: Vec<&str> = listing.all().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a_dir", "a.txt", "b.txt"]);
        assert_eq!(listing.all()[2].size, Some(2));
        assert_eq!(listing.all()[0].size, None, "directories carry no size");
    }

    #[test]
    fn hidden_entries_are_read_but_filtered_at_the_view() {
        let t = Tree::new("hidden");
        t.file(".secret", b"").file("visible", b"");

        let listing = Listing::read(&t.0);
        assert_eq!(listing.all().len(), 2, "both are read");
        assert_eq!(listing.visible(false).len(), 1);
        assert_eq!(listing.visible(true).len(), 2);
        // Toggling must not require another disk read.
        assert_eq!(listing.counts(false), (0, 1));
        assert_eq!(listing.counts(true), (0, 2));
    }

    #[test]
    fn an_unreadable_directory_yields_an_error_not_a_panic() {
        let listing = Listing::read(Path::new("/omafiles-definitely-not-here"));
        assert!(listing.error.is_some());
        assert!(listing.is_empty());
        assert_eq!(
            describe_empty(&listing, false),
            "cannot read this directory"
        );
    }

    #[test]
    fn distinguishes_empty_from_all_hidden() {
        let empty = Tree::new("empty");
        assert_eq!(
            describe_empty(&Listing::read(&empty.0), false),
            "empty directory"
        );

        let hidden = Tree::new("allhidden");
        hidden.file(".only", b"");
        assert_eq!(
            describe_empty(&Listing::read(&hidden.0), false),
            "only hidden files here"
        );
        assert_eq!(describe_empty(&Listing::read(&hidden.0), true), "");
    }

    #[test]
    fn finds_an_entry_by_name_for_cursor_restore() {
        let t = Tree::new("byname");
        t.file("a", b"").file("b", b"").file("c", b"");
        let listing = Listing::read(&t.0);
        assert_eq!(listing.index_of_name("b"), Some(1));
        assert_eq!(listing.index_of_name("nope"), None);
    }

    #[test]
    fn a_broken_symlink_is_listed_not_dropped() {
        let t = Tree::new("brokenlink");
        std::os::unix::fs::symlink(t.0.join("missing"), t.0.join("link")).unwrap();

        let listing = Listing::read(&t.0);
        assert_eq!(listing.all().len(), 1, "a broken link is still information");
        let entry = &listing.all()[0];
        assert_eq!(entry.kind, Kind::Unresolved);
        assert!(entry.is_symlink);
        assert!(!is_navigable(entry));
    }
}
