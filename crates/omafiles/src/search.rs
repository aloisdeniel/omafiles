//! Fuzzy matching for the filter and the recursive search.
//!
//! Uses [`nucleo`] in-process rather than spawning `fzf`. Same algorithm
//! family, but fzf is a full-screen TUI — driving it from a GUI means a pty,
//! ANSI parsing and a process per keystroke-session, and the results could not
//! be drawn with our own design system. Zed uses `nucleo` for the same reason.

use std::path::{Path, PathBuf};

use nucleo::Matcher;
use nucleo::pattern::{CaseMatching, Normalization, Pattern};

/// What the search box is currently doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Match against the entries already loaded. Instant, no IO.
    Directory,
    /// Walk the tree below the current directory. Streams.
    Recursive,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    /// Index into the source list for [`Scope::Directory`]; meaningless for
    /// recursive results, which carry their own path.
    pub index: usize,
    pub path: PathBuf,
    /// What to show. For recursive matches this is the path relative to the
    /// search root, so the row says where the file actually is.
    pub label: String,
    pub score: u32,
}

/// A reusable matcher.
///
/// `nucleo::Matcher` holds scratch buffers and is explicitly designed to be
/// kept around rather than rebuilt per keystroke.
pub struct Search {
    matcher: Matcher,
}

impl Default for Search {
    fn default() -> Self {
        Self::new()
    }
}

impl Search {
    pub fn new() -> Self {
        Self {
            matcher: Matcher::new(nucleo::Config::DEFAULT),
        }
    }

    /// Rank `items` against `query`, best first.
    ///
    /// An empty query matches everything in the original order — the filter
    /// showing nothing until you type would be a worse default than showing the
    /// directory you are already looking at.
    pub fn rank<'a, I>(&mut self, query: &str, items: I) -> Vec<Match>
    where
        I: IntoIterator<Item = (usize, &'a str, PathBuf)>,
    {
        let items: Vec<(usize, &str, PathBuf)> = items.into_iter().collect();

        if query.trim().is_empty() {
            return items
                .into_iter()
                .map(|(index, label, path)| Match {
                    index,
                    path,
                    label: label.to_string(),
                    score: 0,
                })
                .collect();
        }

        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);

        let mut scored: Vec<Match> = items
            .into_iter()
            .filter_map(|(index, label, path)| {
                let mut buffer = Vec::new();
                let haystack = nucleo::Utf32Str::new(label, &mut buffer);
                let score = pattern.score(haystack, &mut self.matcher)?;
                Some(Match {
                    index,
                    path,
                    label: label.to_string(),
                    score,
                })
            })
            .collect();

        // Descending score, then by label so equal scores are stable rather
        // than reshuffling on every keystroke.
        scored.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.label.cmp(&b.label)));
        scored
    }
}

/// How many entries a recursive walk will collect before stopping.
///
/// A search under `/` or a home directory with a huge node_modules must not
/// grow without bound; the ranked top of a very large set is not more useful
/// than the ranked top of a large one.
pub const RECURSIVE_LIMIT: usize = 20_000;

/// The result of a recursive walk.
pub struct Walk {
    pub items: Vec<(PathBuf, String)>,
    /// True when [`RECURSIVE_LIMIT`] stopped us early, so the UI can say so
    /// rather than implying the results are complete.
    pub truncated: bool,
}

/// Collect paths beneath `root` for recursive search.
///
/// Uses `ignore`, which respects `.gitignore` — correct *here*, unlike in the
/// listing: when you are searching a repository you almost never want its build
/// output, and `target/` alone can be 18 GB (measured on this machine).
/// Hidden files follow the same toggle as the listing.
pub fn walk(root: &Path, show_hidden: bool) -> Walk {
    let mut items = Vec::new();
    let mut truncated = false;

    let walker = ignore::WalkBuilder::new(root)
        .hidden(!show_hidden)
        .git_ignore(true)
        .git_global(true)
        .parents(false)
        // Symlinked directories can point back up the tree; `ignore` does not
        // follow by default and we keep it that way, or a search can loop.
        .follow_links(false)
        .build();

    for entry in walker.flatten() {
        // Skip the root itself; matching "." against the query is noise.
        if entry.path() == root {
            continue;
        }
        if items.len() >= RECURSIVE_LIMIT {
            truncated = true;
            break;
        }
        let label = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .into_owned();
        items.push((entry.path().to_path_buf(), label));
    }

    Walk { items, truncated }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(names: &[&str]) -> Vec<(usize, String, PathBuf)> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| (i, n.to_string(), PathBuf::from(format!("/x/{n}"))))
            .collect()
    }

    fn rank(search: &mut Search, query: &str, names: &[&str]) -> Vec<String> {
        let owned = items(names);
        let borrowed = owned.iter().map(|(i, n, p)| (*i, n.as_str(), p.clone()));
        search
            .rank(query, borrowed)
            .into_iter()
            .map(|m| m.label)
            .collect()
    }

    #[test]
    fn an_empty_query_keeps_everything_in_order() {
        let mut s = Search::new();
        // Showing nothing until you type would hide the directory you are
        // already looking at.
        assert_eq!(
            rank(&mut s, "", &["b", "a", "c"]),
            ["b", "a", "c"],
            "original order, not sorted"
        );
        assert_eq!(rank(&mut s, "   ", &["b", "a"]), ["b", "a"]);
    }

    #[test]
    fn matches_subsequences_not_just_prefixes() {
        let mut s = Search::new();
        let out = rank(&mut s, "crg", &["Cargo.toml", "README.md", "config"]);
        assert!(out.contains(&"Cargo.toml".to_string()));
        assert!(!out.contains(&"README.md".to_string()));
    }

    #[test]
    fn is_case_insensitive() {
        let mut s = Search::new();
        assert!(!rank(&mut s, "cargo", &["Cargo.toml"]).is_empty());
        assert!(!rank(&mut s, "CARGO", &["Cargo.toml"]).is_empty());
    }

    #[test]
    fn ranks_the_better_match_first() {
        let mut s = Search::new();
        let out = rank(&mut s, "main", &["domain_helper.rs", "main.rs"]);
        assert_eq!(out.first().map(String::as_str), Some("main.rs"));
    }

    #[test]
    fn equal_scores_are_ordered_stably() {
        let mut s = Search::new();
        // Without the label tiebreak these could reshuffle between keystrokes,
        // which makes the list jump under the cursor.
        let first = rank(&mut s, "a", &["ba", "ca", "aa"]);
        let second = rank(&mut s, "a", &["ba", "ca", "aa"]);
        assert_eq!(first, second);
    }

    #[test]
    fn a_query_matching_nothing_returns_nothing() {
        let mut s = Search::new();
        assert!(rank(&mut s, "zzzzqqq", &["Cargo.toml", "README.md"]).is_empty());
    }

    #[test]
    fn walk_finds_nested_files_and_labels_them_relatively() {
        let root = std::env::temp_dir().join(format!("omafiles-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/deep.txt"), b"x").unwrap();
        std::fs::write(root.join("top.txt"), b"x").unwrap();

        let walk = walk(&root, false);
        let labels: Vec<&str> = walk.items.iter().map(|(_, l)| l.as_str()).collect();
        assert!(labels.contains(&"top.txt"));
        assert!(
            labels.contains(&"a/b/deep.txt"),
            "labels are relative so a row says where the file is: {labels:?}"
        );
        assert!(!walk.truncated);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn walk_respects_the_hidden_toggle() {
        let root = std::env::temp_dir().join(format!("omafiles-walkhidden-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(".secret"), b"x").unwrap();
        std::fs::write(root.join("plain"), b"x").unwrap();

        let visible: Vec<String> = walk(&root, false)
            .items
            .into_iter()
            .map(|(_, l)| l)
            .collect();
        assert!(visible.contains(&"plain".to_string()));
        assert!(!visible.contains(&".secret".to_string()));

        let all: Vec<String> = walk(&root, true)
            .items
            .into_iter()
            .map(|(_, l)| l)
            .collect();
        assert!(all.contains(&".secret".to_string()));

        let _ = std::fs::remove_dir_all(&root);
    }
}
