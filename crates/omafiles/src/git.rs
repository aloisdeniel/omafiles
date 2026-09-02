//! What git knows about the directory we are looking at.
//!
//! `PLAN.md` §6.9's three things: the branch, a status marker per entry, and the
//! diff of a changed file. Pure model, no gpui — the expensive half runs on a
//! background task and the cheap half runs inline, and both are testable
//! headless.
//!
//! # Deviation from §8: reads go through `git`, not `gix`
//!
//! The plan chose `gix` for reads and the `git` binary only for switching. This
//! module uses the binary for both, deliberately:
//!
//! - **The binary is already a hard requirement.** Switching shells out because
//!   clobbering uncommitted work is unrecoverable, so a machine without `git`
//!   has no working M8 either way. `gix` would be a second implementation of
//!   something we still cannot do without.
//! - **`--porcelain=v2 -z` is plumbing.** It is a documented, stable,
//!   machine-readable format that reports the staged and worktree halves of
//!   every path in one pass. Getting the same picture out of `gix` means
//!   composing a tree-index diff with an index-worktree walk and reconciling
//!   them by hand.
//! - **The diff view wants hunks.** `git diff` produces them, with git's own
//!   rename detection, its section-heading heuristic and whatever the user has
//!   configured. `gix` has no unified-diff formatter, so we would be writing the
//!   hunk extraction ourselves and getting none of that.
//! - **§6.9's measurements are of this implementation.** 4 ms on this repo and
//!   400 ms on a 4,288-file checkout were timed with `git status --porcelain=v2`,
//!   so the background-task-and-cache architecture is validated against exactly
//!   what runs here.
//!
//! What that costs is a fork per status read. It is bounded: repo discovery and
//! the branch label are pure filesystem reads with no process at all, status is
//! one fork per repo per change, and the diff is one fork for the previewed file
//! only. Nothing forks on the main thread.
//!
//! # `--no-optional-locks`, and why it is not optional here
//!
//! `git status` normally rewrites `.git/index` to refresh its stat cache. We
//! watch `.git` to notice commits made in a terminal, so that write comes back
//! as an event, which would re-run status, which would write the index again —
//! the same self-sustaining watcher loop M5 hit with the session file, in a new
//! costume. `--no-optional-locks` is git's own answer for exactly this and every
//! read here passes it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Lines of diff kept before it is cut short.
///
/// §6.9: a huge diff is not previewable. The cut is reported rather than hidden,
/// the same way an over-long file preview is.
///
/// Lower than the 4,000-line file cap on purpose. A diff renders one element per
/// line — that is the price of a full-width wash behind each one — where a file
/// preview is a single laid-out text run, so the ceiling that keeps a frame cheap
/// is a different number. Six hundred lines is also well past the point where
/// something stops being a preview and starts being a review.
pub const MAX_DIFF_LINES: usize = 600;

/// Bytes of the `HEAD` version read to colour removed lines.
///
/// The blob is only ever fed to a syntax parser, so the ceiling is the same kind
/// of thing as `preview::MAX_PREVIEW_BYTES` — kept here rather than imported so
/// this module stays readable on its own.
const MAX_BLOB_BYTES: usize = 2 * 1024 * 1024;

/// A repository, found by walking up from a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// The working tree root — the directory holding `.git`.
    pub root: PathBuf,
    /// `.git` itself, or where a `.git` *file* points for a linked worktree or
    /// a submodule. This is what holds `HEAD`, and what we watch.
    pub git_dir: PathBuf,
    /// Where refs live. The same as `git_dir` in the ordinary case, and the
    /// main repository's `.git` for a linked worktree.
    common_dir: PathBuf,
}

/// Where `HEAD` is, in words.
///
/// §6.9: a repo mid-rebase or with a detached HEAD has no branch name, and an
/// empty label there reads as a bug rather than as a state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Branch(String),
    /// A branch that exists only in `HEAD` — a fresh `git init`, before the
    /// first commit.
    Unborn(String),
    /// Short object id.
    Detached(String),
    /// The branch being rebased, when git recorded one.
    Rebasing(Option<String>),
    Merging(Option<String>),
}

impl Head {
    /// What the status bar shows.
    pub fn label(&self) -> String {
        match self {
            Head::Branch(name) => name.clone(),
            Head::Unborn(name) => format!("{name} · no commits"),
            Head::Detached(oid) => format!("detached at {oid}"),
            Head::Rebasing(Some(name)) => format!("rebasing {name}"),
            Head::Rebasing(None) => "rebasing".to_string(),
            Head::Merging(Some(name)) => format!("merging {name}"),
            Head::Merging(None) => "merging".to_string(),
        }
    }

    /// The branch to put the cursor on in the switcher, if there is one.
    pub fn branch(&self) -> Option<&str> {
        match self {
            Head::Branch(name) | Head::Unborn(name) => Some(name),
            _ => None,
        }
    }
}

/// What git says about one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Added,
    Modified,
    Deleted,
    Untracked,
    Conflicted,
}

impl State {
    /// The badge glyph.
    ///
    /// §6.9 asks for conflicted to be *visually* distinct from deleted, and both
    /// are `urgent` — so the distinction has to be the shape, not the colour.
    pub fn marker(&self) -> &'static str {
        match self {
            State::Added => "+",
            State::Modified => "\u{2022}", // •
            State::Deleted => "\u{2212}",  // −
            State::Untracked => "?",
            State::Conflicted => "!",
        }
    }

    /// The palette key to draw it in. Never a literal colour, so a marker
    /// retints with everything else.
    pub fn role(&self) -> &'static str {
        match self {
            State::Added => "green",
            State::Modified => "yellow",
            // `urgent` is Omarchy's name for it; the palette key is `red`.
            State::Deleted | State::Conflicted => "red",
            State::Untracked => "muted",
        }
    }

    /// Which state a directory shows when several are inside it.
    ///
    /// Conflicted and deleted come first because they are the two you must not
    /// miss. Modified beats added below them: a folder that already existed and
    /// changed is better described as modified than as new.
    fn severity(&self) -> u8 {
        match self {
            State::Conflicted => 4,
            State::Deleted => 3,
            State::Modified => 2,
            State::Added => 1,
            State::Untracked => 0,
        }
    }
}

/// How many paths are in each state. The status bar's summary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
    pub untracked: usize,
    pub conflicted: usize,
}

impl Counts {
    pub fn total(&self) -> usize {
        self.added + self.modified + self.deleted + self.untracked + self.conflicted
    }

    pub fn is_clean(&self) -> bool {
        self.total() == 0
    }

    /// Non-zero groups, in marker order, for the status bar.
    pub fn summary(&self) -> Vec<(State, usize)> {
        [
            (State::Conflicted, self.conflicted),
            (State::Added, self.added),
            (State::Modified, self.modified),
            (State::Deleted, self.deleted),
            (State::Untracked, self.untracked),
        ]
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .collect()
    }
}

/// Every changed path in a repository, indexed for per-row lookup.
///
/// §6.9: the rollup is computed **once** from the single status call and indexed
/// by path prefix. Walking per row would make every frame O(entries × changes).
#[derive(Debug, Clone, Default)]
pub struct Status {
    files: HashMap<PathBuf, State>,
    /// Directories that contain something changed, rolled up.
    dirs: HashMap<PathBuf, State>,
    /// `git status` collapses an untracked directory to one entry, so anything
    /// inside it is absent from `files`. Kept so a lookup can walk ancestors and
    /// still mark those.
    untracked_dirs: HashSet<PathBuf>,
    pub counts: Counts,
}

impl Status {
    /// The marker for one entry, or `None` if git has nothing to say about it.
    pub fn of(&self, path: &Path) -> Option<State> {
        if let Some(state) = self.files.get(path) {
            return Some(*state);
        }
        if let Some(state) = self.dirs.get(path) {
            return Some(*state);
        }
        // Inside a collapsed untracked directory.
        path.ancestors()
            .skip(1)
            .any(|a| self.untracked_dirs.contains(a))
            .then_some(State::Untracked)
    }

    fn insert(&mut self, path: PathBuf, state: State, root: &Path) {
        match state {
            State::Added => self.counts.added += 1,
            State::Modified => self.counts.modified += 1,
            State::Deleted => self.counts.deleted += 1,
            State::Untracked => self.counts.untracked += 1,
            State::Conflicted => self.counts.conflicted += 1,
        }

        // Roll up into every directory between the path and the root — never
        // above it, or a marker would leak onto the user's home directory.
        for ancestor in path.ancestors().skip(1) {
            if !ancestor.starts_with(root) {
                break;
            }
            let entry = self.dirs.entry(ancestor.to_path_buf()).or_insert(state);
            if state.severity() > entry.severity() {
                *entry = state;
            }
            if ancestor == root {
                break;
            }
        }
        self.files.insert(path, state);
    }
}

/// Repo discovery, with the negative results kept.
///
/// §6.9: most directories are not in a repository, and detection walks up
/// looking for `.git`. Without this cache every navigation stat-walks to `/`.
#[derive(Debug, Default)]
pub struct Cache {
    known: HashMap<PathBuf, Option<Repo>>,
    /// How many times we actually walked the filesystem. Not debug-only: it is
    /// what the "one cached negative lookup" test asserts on, and a counter that
    /// only exists under `cfg(test)` proves nothing about the shipped path.
    walks: u64,
}

impl Cache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The repository containing `dir`, if any.
    pub fn repo_for(&mut self, dir: &Path) -> Option<Repo> {
        if let Some(found) = self.known.get(dir) {
            return found.clone();
        }
        self.walks += 1;
        let found = discover(dir);
        self.known.insert(dir.to_path_buf(), found.clone());
        found
    }

    /// Drop what we know. Called when `.git` changes, since a `git init` or a
    /// `rm -rf .git` both turn a cached answer into a wrong one.
    pub fn clear(&mut self) {
        self.known.clear();
    }

    pub fn walks(&self) -> u64 {
        self.walks
    }
}

/// Walk up from `dir` looking for `.git`.
pub fn discover(dir: &Path) -> Option<Repo> {
    for root in dir.ancestors() {
        let dot_git = root.join(".git");
        let metadata = std::fs::symlink_metadata(&dot_git).ok();
        let Some(metadata) = metadata else {
            continue;
        };

        let git_dir = if metadata.is_dir() {
            dot_git
        } else {
            // A linked worktree or a submodule: `.git` is a file holding
            // `gitdir: <path>`, which may be relative to the working tree.
            let text = std::fs::read_to_string(&dot_git).ok()?;
            let target = text.strip_prefix("gitdir:")?.trim();
            let target = PathBuf::from(target);
            if target.is_absolute() {
                target
            } else {
                root.join(target)
            }
        };

        // Linked worktrees keep their refs in the main repository's `.git`.
        let common_dir = std::fs::read_to_string(git_dir.join("commondir"))
            .ok()
            .map(|text| {
                let target = PathBuf::from(text.trim());
                if target.is_absolute() {
                    target
                } else {
                    git_dir.join(target)
                }
            })
            .unwrap_or_else(|| git_dir.clone());

        return Some(Repo {
            root: root.to_path_buf(),
            git_dir,
            common_dir,
        });
    }
    None
}

/// Where `HEAD` is. Pure filesystem reads — no process, so this is cheap enough
/// to run inline on navigation and the status bar never waits for it.
pub fn head(repo: &Repo) -> Head {
    // An interrupted operation is the state, and it outranks the branch name:
    // `develop` is a lie while a rebase is half-applied.
    if repo.git_dir.join("rebase-merge").is_dir() || repo.git_dir.join("rebase-apply").is_dir() {
        return Head::Rebasing(rebase_branch(repo));
    }
    if repo.git_dir.join("MERGE_HEAD").is_file() {
        return Head::Merging(symbolic_head(repo));
    }

    match symbolic_head(repo) {
        Some(name) => {
            if ref_exists(repo, &name) {
                Head::Branch(name)
            } else {
                // `git init` writes HEAD before there is anything to point at.
                Head::Unborn(name)
            }
        }
        None => {
            let oid = std::fs::read_to_string(repo.git_dir.join("HEAD"))
                .map(|text| text.trim().chars().take(7).collect::<String>())
                .unwrap_or_default();
            Head::Detached(oid)
        }
    }
}

/// The branch name in `HEAD`, or `None` when it holds a raw object id.
fn symbolic_head(repo: &Repo) -> Option<String> {
    let text = std::fs::read_to_string(repo.git_dir.join("HEAD")).ok()?;
    let reference = text.trim().strip_prefix("ref:")?.trim();
    Some(reference.strip_prefix("refs/heads/")?.to_string())
}

fn rebase_branch(repo: &Repo) -> Option<String> {
    for dir in ["rebase-merge", "rebase-apply"] {
        if let Ok(text) = std::fs::read_to_string(repo.git_dir.join(dir).join("head-name")) {
            let name = text.trim();
            return Some(name.strip_prefix("refs/heads/").unwrap_or(name).to_string());
        }
    }
    None
}

/// Whether `refs/heads/<name>` resolves — loose, then packed.
fn ref_exists(repo: &Repo, name: &str) -> bool {
    if repo
        .common_dir
        .join("refs/heads")
        .join(name)
        .try_exists()
        .unwrap_or(false)
    {
        return true;
    }
    let suffix = format!(" refs/heads/{name}");
    std::fs::read_to_string(repo.common_dir.join("packed-refs"))
        .map(|text| text.lines().any(|line| line.ends_with(&suffix)))
        .unwrap_or(false)
}

/// Every changed path in the repository. **Blocking** — background threads only.
///
/// §6.9 measured 400 ms on a 4,288-file checkout, which is four dropped frames
/// if this is ever called inline.
pub fn status(repo: &Repo) -> Status {
    let output = git(repo)
        .args([
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=normal",
            // Ignored files are deliberately not requested: the only treatment
            // §6.9 gives them is "no marker", and asking for them means walking
            // every ignored path — 18 GB of `target/` on this machine alone.
            "--ignored=no",
        ])
        .output();

    let Ok(output) = output else {
        return Status::default();
    };
    parse_status(&String::from_utf8_lossy(&output.stdout), &repo.root)
}

/// Parse `--porcelain=v2 -z`.
///
/// Split apart from [`status`] so the format — which is where the bugs live —
/// is testable without a repository on disk.
fn parse_status(raw: &str, root: &Path) -> Status {
    let mut status = Status::default();
    // `-z` makes every record NUL-terminated, which is the only way a path
    // containing a newline (legal, and hostile) parses correctly.
    let mut records = raw.split('\0').filter(|r| !r.is_empty());

    while let Some(record) = records.next() {
        let Some((tag, rest)) = record.split_once(' ') else {
            continue;
        };
        match tag {
            // `1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>`
            "1" => {
                if let Some((xy, path)) = field_and_path(rest, 7) {
                    status.insert(root.join(path), from_xy(xy), root);
                }
            }
            // `2 <XY> … <X><score> <path>`, then the original path as the next
            // record. Consuming it here is what keeps the two in step.
            "2" => {
                if let Some((xy, path)) = field_and_path(rest, 8) {
                    status.insert(root.join(path), from_xy(xy), root);
                }
                records.next();
            }
            // `u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>`
            "u" => {
                if let Some((_, path)) = field_and_path(rest, 9) {
                    status.insert(root.join(path), State::Conflicted, root);
                }
            }
            "?" => {
                // A directory arrives with a trailing slash, standing in for
                // everything inside it.
                let path = rest.trim_end_matches('/');
                let absolute = root.join(path);
                if rest.ends_with('/') {
                    status.untracked_dirs.insert(absolute.clone());
                }
                status.insert(absolute, State::Untracked, root);
            }
            // `#` headers and `!` ignored entries carry nothing we render.
            _ => {}
        }
    }
    status
}

/// Split `rest` into its first field and the path that follows `skip` fields.
///
/// The path is everything after the last space-delimited field, taken whole:
/// file names contain spaces routinely, so splitting the record fully would cut
/// half of them in two.
fn field_and_path(rest: &str, skip: usize) -> Option<(&str, &str)> {
    let mut parts = rest.splitn(skip + 1, ' ');
    let first = parts.next()?;
    let path = parts.nth(skip - 1)?;
    (!path.is_empty()).then_some((first, path))
}

/// The two-letter `XY` code: staged, then worktree.
fn from_xy(xy: &str) -> State {
    let mut chars = xy.chars();
    let staged = chars.next().unwrap_or('.');
    let worktree = chars.next().unwrap_or('.');

    // Checked in this order because a file can be several things at once: added
    // then edited is `AM`, and it is new rather than modified.
    if staged == 'A' || worktree == 'A' {
        State::Added
    } else if staged == 'D' || worktree == 'D' {
        State::Deleted
    } else {
        State::Modified
    }
}

/// Local branches, for the switcher.
pub fn branches(repo: &Repo) -> Vec<String> {
    let Ok(output) = git(repo)
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "--sort=-committerdate",
            "refs/heads/",
        ])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Switch branches, and **never force**.
///
/// §6.9's one genuinely dangerous operation, which is why it is the one thing
/// here that is not reimplemented: `git switch` already knows when a checkout
/// would clobber uncommitted work, and handles submodules, sparse-checkout and
/// hooks. On refusal we surface git's own message verbatim rather than
/// paraphrasing it, and the repository is left exactly as it was.
pub fn switch(repo: &Repo, branch: &str) -> Result<(), String> {
    if branch.starts_with('-') {
        return Err("a branch name cannot begin with a dash".to_string());
    }
    let output = git(repo)
        .args(["switch", branch])
        .output()
        .map_err(|err| format!("could not run git: {err}"))?;

    if output.status.success() {
        return Ok(());
    }
    // git writes the refusal to stderr; stdout carries the "Switched to" line.
    let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if message.is_empty() {
        "git refused the switch and said nothing".to_string()
    } else {
        message
    })
}

/// The unified diff of one path against `HEAD`, parsed and capped.
///
/// Against `HEAD` rather than the index, so a file that is half staged shows
/// everything that differs from the last commit — which is what "the diff of a
/// changed file" means to someone looking at a file manager.
///
/// **Blocking** — background threads only.
pub fn diff(repo: &Repo, path: &Path) -> Option<Diff> {
    let output = git(repo)
        .args(["diff", "HEAD", "--no-color", "-U3", "--"])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None; // unborn HEAD, or a path git will not diff
    }

    let raw = String::from_utf8(output.stdout).ok()?;
    let diff = parse_diff(&raw, MAX_DIFF_LINES);
    // A binary file produces "Binary files … differ" and no hunks at all, which
    // is a fact about the file rather than a preview of it — the caller falls
    // back to showing the file itself.
    (!diff.hunks.is_empty()).then_some(diff)
}

/// The `HEAD` version of a path.
///
/// Only ever fed to the syntax parser, so removed lines can be coloured by the
/// same grammar as the rest. A diff carries the *text* of what was removed but
/// not the file it came out of, and highlighting a fragment on its own is where
/// tree-sitter is weakest — it hits an `ERROR` node a line or two in and stops
/// producing captures.
///
/// **Blocking** — background threads only.
pub fn blob_at_head(repo: &Repo, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(&repo.root).ok()?;
    let output = git(repo)
        .args(["show", &format!("HEAD:{}", relative.to_str()?)])
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.len() > MAX_BLOB_BYTES {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// A parsed unified diff: hunks of tagged lines, with real line numbers.
///
/// The raw text is deliberately not kept. Rendering it as *text* — which is what
/// the first version of this did, colouring it with the `diff` grammar — means
/// the `+`/`-` prefixes, the `diff --git` and `index` preamble and the `@@`
/// arithmetic all end up on screen, and the code inside loses its own colours.
/// Zed renders a diff as rows of the file, syntax-highlighted, under a
/// full-width wash; this is the structure that lets us do the same.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
    pub hunks: Vec<Hunk>,
    /// True when [`MAX_DIFF_LINES`] cut it short.
    pub truncated: bool,
    pub added: usize,
    pub removed: usize,
}

/// One run of changed lines with its surrounding context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// What git guessed this hunk is inside — the function or section name it
    /// prints after the second `@@`. Worth keeping: it is the one piece of the
    /// header that says something the line numbers do not.
    pub heading: Option<String>,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    /// The line's number on the side it belongs to — the new file for context
    /// and additions, the old one for removals.
    pub number: u32,
    /// The content, with the `+`/`-`/space prefix already off.
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// Parse `git diff`'s unified output.
///
/// Split out from [`diff`] because the format is where the bugs are and this way
/// they are testable without a repository.
fn parse_diff(raw: &str, limit: usize) -> Diff {
    let mut diff = Diff::default();
    let (mut old_no, mut new_no) = (0u32, 0u32);
    let mut kept = 0usize;
    // Whether we are inside a hunk body. Not derived from "is there a hunk yet",
    // because a second file's `---`/`+++` header would then be read as a removed
    // and an added line — they start with the same characters.
    let mut in_hunk = false;

    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("@@") {
            let Some((old, new, heading)) = parse_hunk_header(rest) else {
                continue;
            };
            old_no = old;
            new_no = new;
            in_hunk = true;
            diff.hunks.push(Hunk {
                heading,
                lines: Vec::new(),
            });
            continue;
        }
        if line.starts_with("diff --git") {
            in_hunk = false;
            continue;
        }
        if !in_hunk {
            continue;
        }
        if kept >= limit {
            diff.truncated = true;
            break;
        }

        let (kind, number) = match line.as_bytes().first() {
            Some(b'+') => (LineKind::Added, &mut new_no),
            Some(b'-') => (LineKind::Removed, &mut old_no),
            Some(b' ') => (LineKind::Context, &mut new_no),
            // "\ No newline at end of file" annotates the line above rather than
            // being one of its own.
            _ => continue,
        };
        let at = *number;
        *number += 1;
        if kind == LineKind::Context {
            old_no += 1;
        }
        match kind {
            LineKind::Added => diff.added += 1,
            LineKind::Removed => diff.removed += 1,
            LineKind::Context => {}
        }

        kept += 1;
        if let Some(hunk) = diff.hunks.last_mut() {
            hunk.lines.push(Line {
                kind,
                number: at,
                text: line[1..].to_string(),
            });
        }
    }

    // A cut that lands between hunks leaves an empty one on the end, which would
    // render as a heading introducing nothing.
    if diff.hunks.last().is_some_and(|hunk| hunk.lines.is_empty()) {
        diff.hunks.pop();
    }
    diff
}

/// `@@ -12,7 +12,9 @@ fn main()` → the two starts and the section name.
fn parse_hunk_header(rest: &str) -> Option<(u32, u32, Option<String>)> {
    let (ranges, heading) = rest.split_once("@@")?;
    let mut parts = ranges.split_whitespace();
    let start = |field: &str, sign: char| -> Option<u32> {
        field
            .strip_prefix(sign)?
            .split(',')
            .next()?
            .parse::<u32>()
            .ok()
    };
    let old = start(parts.next()?, '-')?;
    let new = start(parts.next()?, '+')?;

    let heading = heading.trim();
    Some((old, new, (!heading.is_empty()).then(|| heading.to_string())))
}

/// A `git` invocation rooted in the repository.
///
/// `--no-optional-locks` on every read: see the module docs. It is one flag, and
/// leaving it off any single call site is enough to restart the watcher loop.
fn git(repo: &Repo) -> Command {
    let mut command = Command::new("git");
    command.arg("--no-optional-locks").arg("-C").arg(&repo.root);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch repository. Returns `None` when git is missing, so a machine
    /// without it skips rather than fails — a red suite people learn to ignore
    /// is worse than a skipped one.
    fn repo(name: &str) -> Option<(PathBuf, Repo)> {
        let dir = std::env::temp_dir().join(format!("omafiles-git-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;

        let ok = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&dir)
            .status()
            .ok()?
            .success();
        if !ok {
            eprintln!("skipping: git unavailable");
            return None;
        }
        for (key, value) in [("user.email", "t@example.invalid"), ("user.name", "t")] {
            let _ = Command::new("git")
                .args(["config", key, value])
                .current_dir(&dir)
                .status();
        }
        // macOS puts the temp dir behind a symlink, and so do some Linux
        // setups; canonicalising keeps the paths we build comparable to the
        // ones discovery returns.
        let dir = dir.canonicalize().ok()?;
        let found = discover(&dir)?;
        Some((dir, found))
    }

    fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    fn write(dir: &Path, name: &str, contents: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn discovery_finds_the_root_from_a_nested_directory() {
        let Some((dir, found)) = repo("discover") else {
            return;
        };
        let deep = dir.join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();

        let from_deep = discover(&deep).expect("a repo above");
        assert_eq!(from_deep.root, found.root);
        assert_eq!(from_deep.git_dir, dir.join(".git"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_outside_any_repo_costs_one_cached_lookup() {
        // §6.9: without the negative cache, every navigation stat-walks to `/`.
        let dir = std::env::temp_dir().join(format!("omafiles-norepo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let mut cache = Cache::new();
        assert!(cache.repo_for(&dir).is_none());
        assert_eq!(cache.walks(), 1);

        for _ in 0..50 {
            assert!(cache.repo_for(&dir).is_none());
        }
        assert_eq!(
            cache.walks(),
            1,
            "a negative answer must be remembered, not re-walked"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn head_reads_a_branch_an_unborn_branch_and_a_detached_one() {
        let Some((dir, found)) = repo("head") else {
            return;
        };

        // Fresh `git init`: HEAD names a branch that does not exist yet.
        assert_eq!(head(&found), Head::Unborn("main".to_string()));
        assert_eq!(head(&found).label(), "main · no commits");

        write(&dir, "a.txt", "one\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-qm", "first"]);
        assert_eq!(head(&found), Head::Branch("main".to_string()));

        // §6.9: a detached HEAD must render its state, not an empty label.
        let oid = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&dir)
            .output()
            .unwrap();
        let oid = String::from_utf8_lossy(&oid.stdout).trim().to_string();
        run(&dir, &["checkout", "-q", "--detach", &oid]);

        match head(&found) {
            Head::Detached(short) => {
                assert_eq!(short, oid[..7].to_string());
                assert!(head(&found).label().starts_with("detached at "));
            }
            other => panic!("expected a detached head, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_repository_mid_rebase_says_so() {
        let Some((dir, found)) = repo("rebase") else {
            return;
        };
        write(&dir, "a.txt", "one\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-qm", "base"]);
        run(&dir, &["switch", "-q", "-c", "topic"]);
        write(&dir, "a.txt", "topic\n");
        run(&dir, &["commit", "-qam", "topic"]);
        run(&dir, &["switch", "-q", "main"]);
        write(&dir, "a.txt", "main\n");
        run(&dir, &["commit", "-qam", "main"]);

        // Deliberately conflicting, so the rebase stops and leaves the state on
        // disk rather than completing.
        let _ = Command::new("git")
            .args(["rebase", "main", "topic"])
            .current_dir(&dir)
            .output();

        match head(&found) {
            Head::Rebasing(branch) => {
                assert_eq!(branch.as_deref(), Some("topic"));
                assert!(head(&found).label().contains("rebasing"));
            }
            // A git that resolved it without stopping leaves nothing to assert;
            // failing here would make the suite depend on merge heuristics.
            other => eprintln!("skipping: this git did not stop the rebase ({other:?})"),
        }

        let _ = Command::new("git")
            .args(["rebase", "--abort"])
            .current_dir(&dir)
            .output();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_reports_every_state_and_rolls_directories_up() {
        let Some((dir, found)) = repo("status") else {
            return;
        };
        write(&dir, "keep.txt", "one\n");
        write(&dir, "sub/deep/tracked.txt", "one\n");
        write(&dir, "gone.txt", "one\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-qm", "base"]);

        write(&dir, "sub/deep/tracked.txt", "one\ntwo\n");
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        write(&dir, "fresh.txt", "new\n");
        run(&dir, &["add", "fresh.txt"]);
        write(&dir, "stray.txt", "untracked\n");
        write(&dir, "strays/inside.txt", "untracked\n");

        let status = status(&found);
        assert_eq!(status.of(&dir.join("keep.txt")), None, "an unchanged file");
        assert_eq!(
            status.of(&dir.join("sub/deep/tracked.txt")),
            Some(State::Modified)
        );
        assert_eq!(status.of(&dir.join("gone.txt")), Some(State::Deleted));
        assert_eq!(status.of(&dir.join("fresh.txt")), Some(State::Added));
        assert_eq!(status.of(&dir.join("stray.txt")), Some(State::Untracked));

        // The rollup: a folder is marked when anything inside it changed.
        assert_eq!(status.of(&dir.join("sub")), Some(State::Modified));
        assert_eq!(status.of(&dir.join("sub/deep")), Some(State::Modified));

        // git collapses an untracked directory to one record, so the file
        // inside it is only reachable by walking ancestors.
        assert_eq!(status.of(&dir.join("strays")), Some(State::Untracked));
        assert_eq!(
            status.of(&dir.join("strays/inside.txt")),
            Some(State::Untracked),
            "a collapsed untracked directory still marks what is inside it"
        );

        assert_eq!(status.counts.modified, 1);
        assert_eq!(status.counts.deleted, 1);
        assert_eq!(status.counts.added, 1);
        assert!(!status.counts.is_clean());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_rollup_stops_at_the_repository_root() {
        // A marker escaping upward would light up the user's home directory.
        let root = Path::new("/tmp/repo");
        let mut status = Status::default();
        status.insert(root.join("a/b.txt"), State::Modified, root);

        assert_eq!(status.of(&root.join("a")), Some(State::Modified));
        assert_eq!(status.of(root), Some(State::Modified));
        assert_eq!(status.of(Path::new("/tmp")), None);
        assert_eq!(status.of(Path::new("/")), None);
    }

    #[test]
    fn the_worst_state_inside_a_directory_is_the_one_it_shows() {
        let root = Path::new("/tmp/repo");
        let mut status = Status::default();
        status.insert(root.join("a/new.txt"), State::Added, root);
        status.insert(root.join("a/edited.txt"), State::Modified, root);
        status.insert(root.join("a/stray.txt"), State::Untracked, root);

        assert_eq!(status.of(&root.join("a")), Some(State::Modified));

        status.insert(root.join("a/clash.txt"), State::Conflicted, root);
        assert_eq!(status.of(&root.join("a")), Some(State::Conflicted));
    }

    #[test]
    fn the_porcelain_format_survives_spaces_and_renames() {
        let root = Path::new("/tmp/repo");
        let raw = concat!(
            "# branch.head main\0",
            "1 .M N... 100644 100644 100644 aaa bbb my notes.txt\0",
            "2 R. N... 100644 100644 100644 aaa bbb R100 new name.txt\0old name.txt\0",
            "u UU N... 100644 100644 100644 100644 aaa bbb ccc clash.txt\0",
            "? stray file.txt\0",
        );
        let status = parse_status(raw, root);

        assert_eq!(
            status.of(&root.join("my notes.txt")),
            Some(State::Modified),
            "a path with spaces is one field, not several"
        );
        assert_eq!(
            status.of(&root.join("new name.txt")),
            Some(State::Modified),
            "a rename is reported at its new path"
        );
        assert_eq!(
            status.of(&root.join("old name.txt")),
            None,
            "the rename's second record is consumed, not parsed as an entry"
        );
        assert_eq!(status.of(&root.join("clash.txt")), Some(State::Conflicted));
        assert_eq!(
            status.of(&root.join("stray file.txt")),
            Some(State::Untracked)
        );
        assert_eq!(status.counts.total(), 4);
    }

    #[test]
    fn xy_codes_prefer_the_state_that_says_the_most() {
        assert_eq!(from_xy("A."), State::Added);
        assert_eq!(
            from_xy("AM"),
            State::Added,
            "added then edited is still new"
        );
        assert_eq!(from_xy(".D"), State::Deleted);
        assert_eq!(from_xy("M."), State::Modified);
        assert_eq!(from_xy(".M"), State::Modified);
        assert_eq!(from_xy("R."), State::Modified);
    }

    #[test]
    fn a_refused_switch_surfaces_gits_own_message_and_changes_nothing() {
        let Some((dir, found)) = repo("switch") else {
            return;
        };
        write(&dir, "a.txt", "one\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-qm", "base"]);
        run(&dir, &["switch", "-q", "-c", "topic"]);
        write(&dir, "a.txt", "topic\n");
        run(&dir, &["commit", "-qam", "topic"]);
        run(&dir, &["switch", "-q", "main"]);

        assert_eq!(branches(&found).len(), 2);

        // Uncommitted work that a checkout would clobber. git refuses; we must
        // report why and leave it alone.
        write(&dir, "a.txt", "precious, uncommitted\n");
        let refused = switch(&found, "topic").expect_err("git must refuse this");
        assert!(
            refused.contains("overwritten") || refused.contains("local changes"),
            "git's own words, not ours: {refused:?}"
        );
        assert_eq!(head(&found), Head::Branch("main".to_string()));
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "precious, uncommitted\n",
            "the work must still be there"
        );

        // And a switch git is happy with does move.
        run(&dir, &["checkout", "-q", "--", "a.txt"]);
        switch(&found, "topic").expect("a clean switch");
        assert_eq!(head(&found), Head::Branch("topic".to_string()));

        // A name that could be read as a flag never reaches git.
        assert!(switch(&found, "--force").is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_changed_file_diffs_and_an_unchanged_one_does_not() {
        let Some((dir, found)) = repo("diff") else {
            return;
        };
        write(&dir, "a.txt", "one\ntwo\n");
        write(&dir, "b.txt", "steady\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-qm", "base"]);
        write(&dir, "a.txt", "one\nTWO\n");

        let diff = diff(&found, &dir.join("a.txt")).expect("a diff");
        assert!(!diff.truncated);
        assert_eq!((diff.added, diff.removed), (1, 1));

        let lines: Vec<&Line> = diff.hunks.iter().flat_map(|h| &h.lines).collect();
        // The prefixes are gone: the rows carry their own kind, and a `+` glued
        // to the front of the text would misalign every line against its
        // neighbours and confuse the syntax parser.
        assert!(
            lines
                .iter()
                .any(|l| l.kind == LineKind::Removed && l.text == "two")
        );
        assert!(
            lines
                .iter()
                .any(|l| l.kind == LineKind::Added && l.text == "TWO")
        );
        assert!(
            lines
                .iter()
                .any(|l| l.kind == LineKind::Context && l.text == "one"),
            "context is kept, or the change has nothing to sit against"
        );

        // The `HEAD` side is what colours removed lines.
        assert_eq!(
            blob_at_head(&found, &dir.join("a.txt")).as_deref(),
            Some("one\ntwo\n")
        );

        assert!(
            super::diff(&found, &dir.join("b.txt")).is_none(),
            "an unchanged file has no diff, and must fall back to its content"
        );

        // Staged but uncommitted still differs from HEAD, which is the whole
        // reason the diff is taken against HEAD rather than the index.
        run(&dir, &["add", "a.txt"]);
        assert!(super::diff(&found, &dir.join("a.txt")).is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_huge_diff_is_cut_and_says_so() {
        let Some((dir, found)) = repo("bigdiff") else {
            return;
        };
        write(&dir, "big.txt", "seed\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-qm", "base"]);

        let long: String = (0..MAX_DIFF_LINES * 2)
            .map(|i| format!("line {i}\n"))
            .collect();
        write(&dir, "big.txt", &long);

        let diff = diff(&found, &dir.join("big.txt")).expect("a diff");
        assert!(diff.truncated, "§6.9: cap it, and say so");
        assert_eq!(
            diff.hunks.iter().map(|h| h.lines.len()).sum::<usize>(),
            MAX_DIFF_LINES
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_unified_format_parses_into_numbered_rows() {
        let raw = concat!(
            "diff --git a/src/main.rs b/src/main.rs\n",
            "index 1234567..89abcde 100644\n",
            "--- a/src/main.rs\n",
            "+++ b/src/main.rs\n",
            "@@ -10,4 +10,5 @@ fn main() {\n",
            " let a = 1;\n",
            "-let b = 2;\n",
            "+let b = 3;\n",
            "+let c = 4;\n",
            " let d = 5;\n",
            "\\ No newline at end of file\n",
        );
        let diff = parse_diff(raw, 100);

        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(diff.hunks[0].heading.as_deref(), Some("fn main() {"));
        assert_eq!((diff.added, diff.removed), (2, 1));

        // The preamble is dropped whole: `---` and `+++` start with the same
        // characters as a removed and an added line, and reading them as content
        // is the classic way to get two junk rows at the top of every diff.
        let lines = &diff.hunks[0].lines;
        assert_eq!(lines.len(), 5);
        assert!(!lines.iter().any(|l| l.text.contains("src/main.rs")));

        // Numbering runs down each side independently.
        assert_eq!((lines[0].kind, lines[0].number), (LineKind::Context, 10));
        assert_eq!((lines[1].kind, lines[1].number), (LineKind::Removed, 11));
        assert_eq!((lines[2].kind, lines[2].number), (LineKind::Added, 11));
        assert_eq!((lines[3].kind, lines[3].number), (LineKind::Added, 12));
        assert_eq!((lines[4].kind, lines[4].number), (LineKind::Context, 13));

        // "\ No newline at end of file" annotates the line above; it is not one.
        assert!(!lines.iter().any(|l| l.text.contains("No newline")));
    }

    #[test]
    fn a_second_files_header_does_not_become_two_rows() {
        // `git diff -- <path>` should only ever return one file, but a rename
        // pair or a future caller passing a directory would return two — and the
        // failure mode is silent and wrong rather than loud.
        let raw = concat!(
            "diff --git a/one b/one\n",
            "@@ -1 +1 @@\n",
            "-a\n",
            "+b\n",
            "diff --git a/two b/two\n",
            "--- a/two\n",
            "+++ b/two\n",
            "@@ -1 +1 @@\n",
            "-c\n",
            "+d\n",
        );
        let diff = parse_diff(raw, 100);
        assert_eq!(diff.hunks.len(), 2);
        assert_eq!(diff.hunks[1].lines.len(), 2);
        assert_eq!((diff.added, diff.removed), (2, 2));
    }

    #[test]
    fn a_cut_between_hunks_leaves_no_empty_heading() {
        let raw = concat!(
            "@@ -1,2 +1,2 @@ first\n",
            "-a\n",
            "+b\n",
            "@@ -9,2 +9,2 @@ second\n",
            "-c\n",
            "+d\n",
        );
        // Exactly enough room for the first hunk, so the second opens and then
        // takes nothing — a heading introducing an empty list.
        let diff = parse_diff(raw, 2);
        assert!(diff.truncated);
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(diff.hunks[0].heading.as_deref(), Some("first"));
    }

    #[test]
    fn a_hunk_with_no_section_name_still_parses() {
        let diff = parse_diff("@@ -1 +1 @@\n-a\n+b\n", 100);
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(diff.hunks[0].heading, None);
        assert_eq!(diff.hunks[0].lines[0].number, 1);
    }
}
