//! Reading a directory into a sorted, filtered list of entries.
//!
//! Pure and synchronous. The view calls [`Listing::read`] on a background
//! thread and swaps the result in when it lands, so nothing here needs to know
//! about gpui.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::entry::{Entry, Kind, natural_cmp};

/// The column a listing is ordered by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Age,
}

impl SortKey {
    /// The direction a first click on the column gives, as a file manager's
    /// users expect it: names A to Z, the largest file first, the newest
    /// first. Nobody clicks Size to find the smallest file.
    pub fn natural_descending(self) -> bool {
        matches!(self, SortKey::Size)
    }
}

/// How a listing is ordered: the key, and whether it runs backwards.
///
/// "Ascending" is in the column's own terms — the name column's ascent is
/// alphabetical, the size column's is small to large, and the *age* column's
/// is young to old, so ascending age puts the newest file on top.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sort {
    pub key: SortKey,
    pub descending: bool,
}

impl Sort {
    /// The order a click on `key`'s column asks for: a new column starts in
    /// its natural direction, the current one turns around.
    pub fn clicked(self, key: SortKey) -> Self {
        if self.key == key {
            Self {
                key,
                descending: !self.descending,
            }
        } else {
            Self {
                key,
                descending: key.natural_descending(),
            }
        }
    }

    /// Directories first whatever the key — they have no size, and a listing
    /// whose folders are scattered among the files by date is one you cannot
    /// navigate — then by the key, with the natural name order breaking ties
    /// so the order is total and two listings of one directory agree.
    fn cmp(self, a: &Entry, b: &Entry) -> Ordering {
        b.kind.is_dir().cmp(&a.kind.is_dir()).then_with(|| {
            let by_key = match self.key {
                SortKey::Name => Ordering::Equal,
                SortKey::Size => a.size.cmp(&b.size),
                // Later modification is a smaller age. An entry with no
                // date (an unresolved link) has no age, and sorts old.
                SortKey::Age => b.modified.cmp(&a.modified),
            };
            let within = by_key.then_with(|| natural_cmp(&a.name, &b.name));
            if self.descending {
                within.reverse()
            } else {
                within
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub path: PathBuf,
    /// Everything in the directory, in name order. Hidden entries included —
    /// filtering is [`Listing::visible`]'s job, so toggling "show hidden" does
    /// not need a re-read from disk. Never reordered once read: the cursor
    /// and the marks hold indices into it, and changing the sort must move
    /// the rows, not what the cursor is on.
    entries: Vec<Entry>,
    /// Indices into `entries` in display order, rebuilt by [`Listing::sort`].
    order: Vec<usize>,
    sort: Sort,
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
                    order: Vec::new(),
                    sort: Sort::default(),
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
            order: (0..entries.len()).collect(),
            entries,
            sort: Sort::default(),
            error: None,
        }
    }

    /// Read `path` and order it by `sort` in one go, for the background
    /// thread that has no view to ask afterwards.
    pub fn read_sorted(path: &Path, sort: Sort) -> Self {
        let mut listing = Self::read(path);
        listing.sort(sort);
        listing
    }

    pub fn empty(path: PathBuf) -> Self {
        Self {
            path,
            entries: Vec::new(),
            order: Vec::new(),
            sort: Sort::default(),
            error: None,
        }
    }

    /// Reorder the rows. Indices stay what they were, so a cursor or a mark
    /// held across the call is still on the same entry, in its new place.
    pub fn sort(&mut self, sort: Sort) {
        self.sort = sort;
        let entries = &self.entries;
        // Already in name order, and `sort_by` is stable, so the name key is
        // a no-op pass and every other key breaks its ties by name for free.
        self.order = (0..entries.len()).collect();
        if sort.key != SortKey::Name || sort.descending {
            self.order
                .sort_by(|&a, &b| sort.cmp(&entries[a], &entries[b]));
        }
    }

    pub fn sort_order(&self) -> Sort {
        self.sort
    }

    /// Indices into [`Self::all`] that should be shown, in display order.
    ///
    /// Returns indices rather than references so the caller can keep a cursor
    /// that survives a hidden-files toggle or a change of sort.
    pub fn visible(&self, show_hidden: bool) -> Vec<usize> {
        self.order
            .iter()
            .copied()
            .filter(|&i| show_hidden || !self.entries[i].is_hidden())
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

    fn names_in_order(listing: &Listing) -> Vec<&str> {
        listing
            .visible(true)
            .into_iter()
            .map(|i| listing.get(i).unwrap().name.as_str())
            .collect()
    }

    #[test]
    fn sorting_moves_the_rows_but_not_the_indices() {
        let t = Tree::new("sortkeys");
        t.file("small", b"x")
            .file("big", b"xxxxxxxx")
            .file("mid", b"xxx")
            .dir("folder");
        let mut listing = Listing::read(&t.0);
        assert_eq!(names_in_order(&listing), ["folder", "big", "mid", "small"]);
        let big = listing.index_of_name("big").unwrap();

        listing.sort(Sort {
            key: SortKey::Size,
            descending: true,
        });
        assert_eq!(
            names_in_order(&listing),
            ["folder", "big", "mid", "small"],
            "directories stay first, then largest first"
        );
        listing.sort(Sort {
            key: SortKey::Size,
            descending: false,
        });
        assert_eq!(names_in_order(&listing), ["folder", "small", "mid", "big"]);
        assert_eq!(
            listing.index_of_name("big"),
            Some(big),
            "an index still names the same entry after a re-sort"
        );

        listing.sort(Sort {
            key: SortKey::Name,
            descending: true,
        });
        assert_eq!(names_in_order(&listing), ["folder", "small", "mid", "big"]);
    }

    #[test]
    fn age_ascending_is_newest_first_and_dateless_last() {
        let t = Tree::new("sortage");
        t.file("old", b"").file("new", b"");
        let ancient = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        std::fs::File::options()
            .write(true)
            .open(t.0.join("old"))
            .unwrap()
            .set_modified(ancient)
            .unwrap();
        std::os::unix::fs::symlink(t.0.join("missing"), t.0.join("link")).unwrap();

        let mut listing = Listing::read(&t.0);
        listing.sort(Sort {
            key: SortKey::Age,
            descending: false,
        });
        assert_eq!(names_in_order(&listing), ["new", "old", "link"]);
        listing.sort(Sort {
            key: SortKey::Age,
            descending: true,
        });
        assert_eq!(names_in_order(&listing), ["link", "old", "new"]);
    }

    #[test]
    fn a_click_turns_the_current_column_and_starts_a_new_one_naturally() {
        let sort = Sort::default();
        let by_size = sort.clicked(SortKey::Size);
        assert_eq!(by_size.key, SortKey::Size);
        assert!(by_size.descending, "size starts largest first");
        assert!(!by_size.clicked(SortKey::Size).descending);
        let by_age = by_size.clicked(SortKey::Age);
        assert!(!by_age.descending, "age starts newest first");
        assert!(!by_age.clicked(SortKey::Name).descending);
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
