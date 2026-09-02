//! One row in a directory listing.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Directory,
    File,
    /// A symlink we could not resolve — a broken link, or one pointing outside
    /// what we can stat. Shown rather than hidden: a broken link in a directory
    /// is information, and silently omitting entries is how a file manager
    /// loses someone's trust.
    Unresolved,
}

impl Kind {
    pub fn is_dir(self) -> bool {
        matches!(self, Kind::Directory)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    /// The file name alone. Stored rather than derived because it is read on
    /// every frame for every visible row.
    pub name: String,
    pub kind: Kind,
    /// `None` for directories — computing a directory's size means walking it,
    /// which is not something a listing should do eagerly.
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
    /// Whether this is a symlink, independent of what it resolves to.
    pub is_symlink: bool,
}

impl Entry {
    /// Read one directory entry. Never fails: a `stat` that errors yields an
    /// [`Kind::Unresolved`] entry rather than dropping the file from the view.
    pub fn from_path(path: PathBuf) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());

        // symlink_metadata does not follow; metadata does. We want both: the
        // former tells us it is a link, the latter what it points at.
        let is_symlink = path
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);

        match path.metadata() {
            Ok(meta) => Self {
                kind: if meta.is_dir() {
                    Kind::Directory
                } else {
                    Kind::File
                },
                size: (!meta.is_dir()).then_some(meta.len()),
                modified: meta.modified().ok(),
                name,
                path,
                is_symlink,
            },
            Err(_) => Self {
                kind: Kind::Unresolved,
                size: None,
                modified: None,
                name,
                path,
                is_symlink,
            },
        }
    }

    /// A leading dot, the Unix convention. Not the same as "should be hidden" —
    /// that is the view's decision, made in [`crate::listing::Listing`].
    pub fn is_hidden(&self) -> bool {
        self.name.starts_with('.')
    }

    /// Directories first, then natural order by name.
    ///
    /// Natural rather than lexicographic because a file manager full of
    /// `frame1.png … frame10.png` sorted as `1, 10, 2` is visibly wrong in a way
    /// people notice immediately.
    pub fn sort_key_cmp(&self, other: &Self) -> Ordering {
        other
            .kind
            .is_dir()
            .cmp(&self.kind.is_dir())
            .then_with(|| natural_cmp(&self.name, &other.name))
    }
}

/// Compare two names treating digit runs as numbers.
///
/// Case-insensitive, with a case-sensitive tiebreak so the order is total and
/// stable — otherwise `README` and `readme` could swap between listings.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut left = a.chars().peekable();
    let mut right = b.chars().peekable();

    loop {
        match (left.peek().copied(), right.peek().copied()) {
            (None, None) => break,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(l), Some(r)) => {
                if l.is_ascii_digit() && r.is_ascii_digit() {
                    let ln = take_number(&mut left);
                    let rn = take_number(&mut right);
                    match ln.cmp(&rn) {
                        Ordering::Equal => continue,
                        other => return other,
                    }
                } else {
                    left.next();
                    right.next();
                    let (lc, rc) = (l.to_ascii_lowercase(), r.to_ascii_lowercase());
                    match lc.cmp(&rc) {
                        Ordering::Equal => continue,
                        other => return other,
                    }
                }
            }
        }
    }

    // Equal ignoring case and digit grouping: fall back to raw bytes so the
    // ordering is total.
    a.cmp(b)
}

/// Consume a run of digits. Saturates rather than overflowing — a 30-digit
/// filename is not worth a panic.
fn take_number(chars: &mut std::iter::Peekable<std::str::Chars>) -> u128 {
    let mut value: u128 = 0;
    while let Some(c) = chars.peek().copied() {
        if !c.is_ascii_digit() {
            break;
        }
        chars.next();
        value = value
            .saturating_mul(10)
            .saturating_add((c as u8 - b'0') as u128);
    }
    value
}

/// Human-readable size, for the listing's right-hand column.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "K", "M", "G", "T", "P"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// Compact relative age, e.g. `2m`, `4h`, `9d`.
pub fn format_age(modified: SystemTime) -> String {
    let Ok(elapsed) = modified.elapsed() else {
        // A file dated in the future. Clock skew and bad archives both produce
        // these; showing "now" beats showing a negative duration.
        return "now".to_string();
    };
    let secs = elapsed.as_secs();
    match secs {
        0..=59 => "now".to_string(),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        86_400..=2_591_999 => format!("{}d", secs / 86_400),
        2_592_000..=31_535_999 => format!("{}mo", secs / 2_592_000),
        _ => format!("{}y", secs / 31_536_000),
    }
}

/// The nearest ancestor of `path` that exists.
///
/// A tab pointing at an unmounted drive or a deleted directory must land
/// somewhere rather than failing — see `PLAN.md` M5.
pub fn nearest_existing(path: &Path) -> PathBuf {
    let mut candidate = path;
    loop {
        if candidate.is_dir() {
            return candidate.to_path_buf();
        }
        match candidate.parent() {
            Some(parent) => candidate = parent,
            None => return PathBuf::from("/"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_digit_runs_as_numbers() {
        let mut names = ["frame10.png", "frame2.png", "frame1.png"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["frame1.png", "frame2.png", "frame10.png"]);
    }

    #[test]
    fn sorting_is_case_insensitive_but_total() {
        let mut names = ["b.txt", "A.txt", "a.txt"];
        names.sort_by(|a, b| natural_cmp(a, b));
        // A and a adjacent (case-insensitively equal), b after both.
        assert_eq!(names[2], "b.txt");
        assert!(names[..2].contains(&"A.txt") && names[..2].contains(&"a.txt"));

        // Total: never returns Equal for different strings, or sorts become
        // unstable between listings.
        assert_ne!(natural_cmp("A.txt", "a.txt"), Ordering::Equal);
    }

    #[test]
    fn huge_digit_runs_do_not_panic() {
        let long = "9".repeat(64);
        assert_eq!(natural_cmp(&long, &long), Ordering::Equal);
        natural_cmp(&format!("f{long}"), "f1");
    }

    #[test]
    fn directories_sort_before_files() {
        let dir = Entry {
            path: "/x/zzz".into(),
            name: "zzz".into(),
            kind: Kind::Directory,
            size: None,
            modified: None,
            is_symlink: false,
        };
        let file = Entry {
            path: "/x/aaa".into(),
            name: "aaa".into(),
            kind: Kind::File,
            size: Some(0),
            modified: None,
            is_symlink: false,
        };
        assert_eq!(dir.sort_key_cmp(&file), Ordering::Less);
    }

    #[test]
    fn formats_sizes_at_the_unit_boundaries() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 K");
        assert_eq!(format_size(1536), "1.5 K");
        assert_eq!(format_size(1024 * 1024), "1.0 M");
        assert_eq!(format_size(15 * 1024 * 1024), "15 M");
    }

    #[test]
    fn formats_age_including_the_future() {
        use std::time::Duration;
        let now = SystemTime::now();
        assert_eq!(format_age(now), "now");
        assert_eq!(format_age(now - Duration::from_secs(300)), "5m");
        assert_eq!(format_age(now - Duration::from_secs(7200)), "2h");
        assert_eq!(format_age(now - Duration::from_secs(172_800)), "2d");
        // Clock skew must not produce a negative or a panic.
        assert_eq!(format_age(now + Duration::from_secs(3600)), "now");
    }

    #[test]
    fn nearest_existing_walks_up_to_something_real() {
        let tmp = std::env::temp_dir();
        let missing = tmp.join("omafiles-does-not-exist/nor/this");
        assert_eq!(nearest_existing(&missing), tmp);
        assert_eq!(nearest_existing(&tmp), tmp);
    }

    #[test]
    fn a_hidden_name_is_just_a_leading_dot() {
        let entry = |name: &str| Entry {
            path: format!("/x/{name}").into(),
            name: name.into(),
            kind: Kind::File,
            size: None,
            modified: None,
            is_symlink: false,
        };
        assert!(entry(".gitignore").is_hidden());
        assert!(!entry("gitignore").is_hidden());
    }
}
