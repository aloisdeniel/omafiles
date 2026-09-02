//! The most recently modified files under a root — the sidebar's "Recent".
//!
//! A walk, not a database: the `ignore` walker sweeps the tree the same way
//! the recursive search does (gitignore honoured, hidden entries skipped —
//! `~/.cache` churns constantly and answers no question anyone asked), and a
//! bounded heap keeps only the newest [`LIMIT`] files, so memory stays flat
//! however large the tree is. **Blocking** — background executors only.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Files kept. Past this, "recent" stops meaning anything.
pub const LIMIT: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentFile {
    pub path: PathBuf,
    pub modified: SystemTime,
}

/// Walk `root` and return the newest files, newest first.
pub fn scan(root: &Path) -> Vec<RecentFile> {
    // A min-heap of the newest LIMIT: the root of the heap is the *oldest*
    // of the keepers, so each candidate compares against exactly one entry.
    let mut keep: BinaryHeap<Reverse<(SystemTime, PathBuf)>> = BinaryHeap::with_capacity(LIMIT);

    for entry in ignore::WalkBuilder::new(root).build().flatten() {
        let Some(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_file() {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|meta| {
            meta.modified()
                .map_err(|err| ignore::Error::Io(std::io::Error::other(err)))
        }) else {
            continue;
        };

        if keep.len() < LIMIT {
            keep.push(Reverse((modified, entry.into_path())));
        } else if keep
            .peek()
            .is_some_and(|Reverse((oldest, _))| modified > *oldest)
        {
            keep.pop();
            keep.push(Reverse((modified, entry.into_path())));
        }
    }

    let mut files: Vec<RecentFile> = keep
        .into_iter()
        .map(|Reverse((modified, path))| RecentFile { path, modified })
        .collect();
    files.sort_by_key(|file| Reverse(file.modified));
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn touch(path: &Path, age: Duration) {
        std::fs::write(path, "x").unwrap();
        let time = SystemTime::now() - age;
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(time).unwrap();
    }

    #[test]
    fn newest_first_and_files_only() {
        let dir = std::env::temp_dir().join(format!("omafiles-recent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        touch(&dir.join("old.txt"), Duration::from_secs(3600));
        touch(&dir.join("sub/newest.txt"), Duration::from_secs(10));
        touch(&dir.join("middle.txt"), Duration::from_secs(600));
        // Hidden stays out, matching the walker the recursive search uses.
        touch(&dir.join(".secret"), Duration::from_secs(1));

        let files = scan(&dir);
        let names: Vec<_> = files
            .iter()
            .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["newest.txt", "middle.txt", "old.txt"]);
    }

    #[test]
    fn the_limit_keeps_the_newest_hundred() {
        let dir = std::env::temp_dir().join(format!("omafiles-recent-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 0 is the newest; anything numbered LIMIT or later must fall out.
        for i in 0..(LIMIT + 20) {
            touch(
                &dir.join(format!("f{i:03}.txt")),
                Duration::from_secs(i as u64),
            );
        }

        let files = scan(&dir);
        assert_eq!(files.len(), LIMIT);
        assert!(files[0].path.ends_with("f000.txt"), "newest survives");
        assert!(
            files.iter().all(|f| !f.path.ends_with("f119.txt")),
            "the oldest fell out"
        );
        // Strictly newest-first throughout.
        assert!(files.windows(2).all(|w| w[0].modified >= w[1].modified));
    }
}
