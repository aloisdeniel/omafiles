//! Copy, paste, move, create and compress — the actions that *make* files.
//!
//! Everything lands beside what already exists, never over it: a paste or a
//! zip whose name is taken gets a ` (2)`-numbered variant, because a file
//! manager that silently overwrites is a file manager exactly once. All of it
//! is **blocking** — background executors only.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A name in `dir` that does not exist yet, counting up from `name`.
///
/// The counter goes before the extension (`a.txt` → `a (2).txt`), because the
/// suffix is what makes the file openable and the counter is just bookkeeping.
pub fn unique_target(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, extension) = match name.split_once('.') {
        // A leading dot is a hidden file, not an empty stem with an extension.
        Some((stem, rest)) if !stem.is_empty() => (stem.to_string(), format!(".{rest}")),
        _ => (name.to_string(), String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){extension}")))
        .find(|candidate| !candidate.exists())
        .expect("the integers ran out")
}

/// Copy `source` into `dest_dir`, files and directories alike.
///
/// Returns the path it landed at — which may be numbered, and is what the
/// caller puts the cursor on.
pub fn copy_into(source: &Path, dest_dir: &Path) -> Result<PathBuf, String> {
    let name = source
        .file_name()
        .ok_or_else(|| "cannot copy a path with no name".to_string())?
        .to_string_lossy()
        .into_owned();
    let target = unique_target(dest_dir, &name);

    // Pasting a directory into itself would recurse forever via the freshly
    // created copy; refuse the cycle rather than racing it.
    if source.is_dir() && target.starts_with(source) {
        return Err("cannot paste a directory into itself".to_string());
    }

    copy_recursive(source, &target).map_err(|err| {
        // A half-written copy is worse than none: take the debris with us.
        let _ = if target.is_dir() {
            std::fs::remove_dir_all(&target)
        } else {
            std::fs::remove_file(&target)
        };
        format!("copy failed: {err}")
    })?;
    Ok(target)
}

fn copy_recursive(source: &Path, target: &Path) -> std::io::Result<()> {
    if source.is_dir() {
        std::fs::create_dir(target)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &target.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        // Symlinks are followed rather than recreated: a pasted copy should
        // stand on its own, and a dangling link in the copy would not.
        std::fs::copy(source, target).map(|_| ())
    }
}

/// Move `source` into `dest_dir`, files and directories alike.
///
/// A rename when the destination is on the same filesystem; a copy followed
/// by a delete when it is not (a USB stick, a network mount), so the verb
/// means the same thing everywhere. Returns the path it landed at, numbered
/// on collision like a paste.
pub fn move_into(source: &Path, dest_dir: &Path) -> Result<PathBuf, String> {
    let name = source
        .file_name()
        .ok_or_else(|| "cannot move a path with no name".to_string())?
        .to_string_lossy()
        .into_owned();
    if !dest_dir.is_dir() {
        return Err(format!("{} is not a directory", dest_dir.display()));
    }
    // Moving into the directory it is already in would only mint a numbered
    // duplicate — say so instead.
    if source.parent() == Some(dest_dir) {
        return Err(format!("\u{201c}{name}\u{201d} is already there"));
    }
    let target = unique_target(dest_dir, &name);
    if source.is_dir() && target.starts_with(source) {
        return Err("cannot move a directory into itself".to_string());
    }

    match std::fs::rename(source, &target) {
        Ok(()) => Ok(target),
        // EXDEV: a different filesystem. Same outcome, the long way round.
        Err(err) if err.raw_os_error() == Some(18) => {
            copy_recursive(source, &target).map_err(|err| {
                let _ = if target.is_dir() {
                    std::fs::remove_dir_all(&target)
                } else {
                    std::fs::remove_file(&target)
                };
                format!("move failed: {err}")
            })?;
            let removed = if source.is_dir() {
                std::fs::remove_dir_all(source)
            } else {
                std::fs::remove_file(source)
            };
            // The copy stands either way; only the cleanup is reported.
            removed.map_err(|err| format!("moved, but could not remove the original: {err}"))?;
            Ok(target)
        }
        Err(err) => Err(format!("move failed: {err}")),
    }
}

/// Write `bytes` as a new file called `name` in `dir` — numbered like a
/// paste if the name is taken. What a picture from the clipboard becomes.
pub fn write_new(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let target = unique_target(dir, name);
    std::fs::write(&target, bytes).map_err(|err| format!("could not write: {err}"))?;
    Ok(target)
}

/// Move `path` to the trash, through `gio trash` — the same trash the rest
/// of the desktop restores from, with the original location recorded, and
/// working across mounts. Never `unlink`: a delete that cannot be undone is
/// not a verb a file manager should offer at all (§6.8).
pub fn trash(path: &Path) -> Result<(), String> {
    let output = Command::new("gio")
        .arg("trash")
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                "install glib2 (gio) to move files to the trash".to_string()
            } else {
                format!("could not run gio: {err}")
            }
        })?;
    if output.status.success() {
        return Ok(());
    }
    let why = String::from_utf8_lossy(&output.stderr);
    let why = why
        .lines()
        .last()
        .unwrap_or("gio refused")
        .trim()
        .trim_start_matches("gio: ");
    Err(format!("could not trash: {why}"))
}

/// Create an empty file at `path`, and any directories above it.
///
/// Never over an existing file: the whole point of a "new file" verb is
/// that it cannot lose anything.
pub fn create_file(path: &Path) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .ok_or_else(|| "a file needs a name".to_string())?
        .to_string_lossy()
        .into_owned();
    if path.exists() {
        return Err(format!("\u{201c}{name}\u{201d} already exists"));
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("could not create {}: {err}", parent.display()))?;
    }
    std::fs::File::create_new(path).map_err(|err| format!("could not create the file: {err}"))?;
    Ok(path.to_path_buf())
}

/// Create the directory at `path`, parents included, and return it.
pub fn create_directory(path: &Path) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .ok_or_else(|| "a directory needs a name".to_string())?
        .to_string_lossy()
        .into_owned();
    if path.exists() {
        return Err(format!("\u{201c}{name}\u{201d} already exists"));
    }
    std::fs::create_dir_all(path)
        .map_err(|err| format!("could not create the directory: {err}"))?;
    Ok(path.to_path_buf())
}

/// Zip `source` into a sibling archive and return its path.
///
/// `zip -r` when installed, else `bsdtar` (libarchive ships with the base
/// system) writing the same format. The archive is created in the source's
/// directory with relative paths inside, so unpacking elsewhere yields the
/// entry itself and not a `/tmp/...` tree.
pub fn compress(source: &Path) -> Result<PathBuf, String> {
    let dir = source
        .parent()
        .ok_or_else(|| "cannot compress the filesystem root".to_string())?;
    let name = source
        .file_name()
        .ok_or_else(|| "cannot compress a path with no name".to_string())?
        .to_string_lossy()
        .into_owned();
    let target = unique_target(dir, &format!("{name}.zip"));

    let zip = Command::new("zip")
        .arg("-r")
        .arg("-q")
        .arg(&target)
        .arg(&name)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();

    let output = match zip {
        Ok(output) => output,
        // No `zip` binary: same archive out of bsdtar.
        Err(_) => Command::new("bsdtar")
            .arg("--format=zip")
            .arg("-cf")
            .arg(&target)
            .arg(&name)
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|err| format!("neither zip nor bsdtar is available: {err}"))?,
    };

    if output.status.success() {
        return Ok(target);
    }
    let _ = std::fs::remove_file(&target);
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if stderr.is_empty() {
        "compression failed".to_string()
    } else {
        stderr
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("omafiles-fops-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/nested")).unwrap();
        std::fs::write(dir.join("a.txt"), "alpha").unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn x() {}").unwrap();
        std::fs::write(dir.join("src/nested/deep.txt"), "deep").unwrap();
        dir
    }

    #[test]
    fn unique_target_counts_before_the_extension() {
        let dir = fixture("unique");
        assert_eq!(unique_target(&dir, "b.txt"), dir.join("b.txt"));
        assert_eq!(unique_target(&dir, "a.txt"), dir.join("a (2).txt"));
        std::fs::write(dir.join("a (2).txt"), "x").unwrap();
        assert_eq!(unique_target(&dir, "a.txt"), dir.join("a (3).txt"));
        // Directories and hidden files have no extension to protect.
        assert_eq!(unique_target(&dir, "src"), dir.join("src (2)"));
        std::fs::write(dir.join(".env"), "x").unwrap();
        assert_eq!(unique_target(&dir, ".env"), dir.join(".env (2)"));
    }

    #[test]
    fn copies_a_file_and_numbers_the_collision() {
        let dir = fixture("copyfile");
        let dest = dir.join("dest");
        std::fs::create_dir(&dest).unwrap();

        let first = copy_into(&dir.join("a.txt"), &dest).unwrap();
        assert_eq!(first, dest.join("a.txt"));
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "alpha");

        let second = copy_into(&dir.join("a.txt"), &dest).unwrap();
        assert_eq!(second, dest.join("a (2).txt"));
    }

    #[test]
    fn copies_a_directory_tree_whole() {
        let dir = fixture("copytree");
        let dest = dir.join("dest");
        std::fs::create_dir(&dest).unwrap();

        let copied = copy_into(&dir.join("src"), &dest).unwrap();
        assert_eq!(
            std::fs::read_to_string(copied.join("nested/deep.txt")).unwrap(),
            "deep"
        );
    }

    #[test]
    fn refuses_to_paste_a_directory_into_itself() {
        let dir = fixture("cycle");
        let src = dir.join("src");
        let error = copy_into(&src, &src).unwrap_err();
        assert!(error.contains("into itself"));
        assert!(!src.join("src").exists(), "nothing half-made is left");
    }

    #[test]
    fn moves_a_file_and_numbers_the_collision() {
        let dir = fixture("movefile");
        let dest = dir.join("dest");
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(dest.join("a.txt"), "taken").unwrap();

        let landed = move_into(&dir.join("a.txt"), &dest).unwrap();
        assert_eq!(landed, dest.join("a (2).txt"));
        assert_eq!(std::fs::read_to_string(&landed).unwrap(), "alpha");
        assert!(!dir.join("a.txt").exists(), "the original is gone");
    }

    #[test]
    fn moves_a_directory_tree_whole() {
        let dir = fixture("movetree");
        let dest = dir.join("dest");
        std::fs::create_dir(&dest).unwrap();

        let moved = move_into(&dir.join("src"), &dest).unwrap();
        assert_eq!(
            std::fs::read_to_string(moved.join("nested/deep.txt")).unwrap(),
            "deep"
        );
        assert!(!dir.join("src").exists());
    }

    #[test]
    fn refuses_pointless_and_circular_moves() {
        let dir = fixture("moverefuse");
        let same = move_into(&dir.join("a.txt"), &dir).unwrap_err();
        assert!(same.contains("already there"));
        assert!(dir.join("a.txt").exists());

        let src = dir.join("src");
        let cycle = move_into(&src, &src.join("nested")).unwrap_err();
        assert!(cycle.contains("into itself"));
        assert!(src.join("lib.rs").exists(), "nothing moved");

        let missing = move_into(&dir.join("a.txt"), &dir.join("nowhere")).unwrap_err();
        assert!(missing.contains("not a directory"));
    }

    #[test]
    fn creates_an_empty_file_and_the_directories_above_it() {
        let dir = fixture("createfile");
        let created = create_file(&dir.join("notes/today.md")).unwrap();
        assert_eq!(created, dir.join("notes/today.md"));
        assert_eq!(std::fs::read_to_string(&created).unwrap(), "");

        let taken = create_file(&dir.join("a.txt")).unwrap_err();
        assert!(taken.contains("already exists"));
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "alpha");
        let no_name = create_file(Path::new("/")).unwrap_err();
        assert!(no_name.contains("name"));
    }

    #[test]
    fn compresses_and_the_archive_lands_beside_the_source() {
        let dir = fixture("zip");
        match compress(&dir.join("src")) {
            Ok(archive) => {
                assert_eq!(archive, dir.join("src.zip"));
                assert!(archive.exists());
                // Again: the name is taken now, so the next one numbers.
                let second = compress(&dir.join("src")).unwrap();
                assert_eq!(second, dir.join("src (2).zip"));
            }
            // A machine with neither tool skips rather than fails.
            Err(error) => assert!(error.contains("available")),
        }
    }

    #[test]
    fn a_directory_is_created_with_its_parents_and_never_twice() {
        let dir = std::env::temp_dir().join(format!("omafiles-mkdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let made = create_directory(&dir.join("a/b")).unwrap();
        assert!(made.is_dir());
        let again = create_directory(&dir.join("a/b")).unwrap_err();
        assert!(again.contains("already exists"), "{again}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trashing_nothing_is_an_error_not_a_panic() {
        if Command::new("gio").arg("version").output().is_err() {
            eprintln!("gio not installed; skipping");
            return;
        }
        let missing = std::env::temp_dir().join("omafiles-trash-does-not-exist");
        let err = trash(&missing).unwrap_err();
        assert!(err.starts_with("could not trash"), "{err}");
    }
}
