//! Content search — §6.4's third mode, the one M6 deferred.
//!
//! Shells out to `rg --json` rather than pulling the `grep-searcher` crates,
//! on M8's git reasoning: `rg` ships with Omarchy, its JSON output is a
//! documented interface, and it already knows everything worth knowing about
//! walking a tree — gitignore, binary detection, encodings. Reimplementing
//! that in-process buys nothing but code.
//!
//! The search is **literal** (`--fixed-strings`), because someone typing into
//! a file manager's content search is looking for text, and a query that dies
//! on an unbalanced `(` looks broken rather than regexy. Smart case matches
//! how the fuzzy search already feels.
//!
//! Everything is capped: matches per file, matched-line length, file size and
//! total hits — a content search is a way *in*, not a report. When the cap
//! is hit the child is killed and [`Outcome::truncated`] says so, because
//! silently implying completeness is worse than admitting the cut.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Total hits kept. Past this the search is telling the user to type more.
pub const LIMIT: usize = 200;

/// Matches reported per file, so one log file cannot fill the list.
const PER_FILE: usize = 8;

/// Stored line length. A minified bundle's 40 kB line is not a preview.
const LINE_CAP: usize = 240;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    pub line: u64,
    /// The matched line, trimmed and capped.
    pub text: String,
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub hits: Vec<Hit>,
    /// True when [`LIMIT`] cut the list short.
    pub truncated: bool,
}

/// Search `root` for `query`. **Blocking** — background executors only.
pub fn search(root: &Path, query: &str) -> Result<Outcome, String> {
    let mut child = Command::new("rg")
        .arg("--json")
        .arg("--fixed-strings")
        .arg("--smart-case")
        // A file big enough to trip this is generated, and generated files
        // answer content questions with noise.
        .arg("--max-filesize=2M")
        .arg(format!("--max-count={PER_FILE}"))
        .arg("--")
        .arg(query)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("could not run ripgrep: {err}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "ripgrep gave no output".to_string())?;

    let mut outcome = Outcome::default();
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let Some(hit) = parse_line(root, &line) else {
            continue;
        };
        if outcome.hits.len() == LIMIT {
            // Enough. Killing rather than draining: on a big tree the rest of
            // the output is exactly the work we no longer want done.
            outcome.truncated = true;
            let _ = child.kill();
            break;
        }
        outcome.hits.push(hit);
    }
    let status = child.wait();

    // rg's exit codes are an interface: 0 matches, 1 clean no-match, 2 error.
    // A kill from the cap also lands here, so only a *real* failure with no
    // hits at all is worth surfacing — a partial list beats an error box.
    if outcome.hits.is_empty()
        && !outcome.truncated
        && let Ok(status) = status
        && status.code() == Some(2)
    {
        return Err("ripgrep failed on this directory".to_string());
    }
    Ok(outcome)
}

/// One `--json` event. Only `"type":"match"` produces a hit.
fn parse_line(root: &Path, line: &str) -> Option<Hit> {
    let event: serde_json::Value = serde_json::from_str(line).ok()?;
    if event.get("type")?.as_str()? != "match" {
        return None;
    }
    let data = event.get("data")?;
    let path = data.get("path")?.get("text")?.as_str()?;
    let line_number = data.get("line_number")?.as_u64()?;
    // A binary match carries no lines.text; skipping it is correct.
    let text = data.get("lines")?.get("text")?.as_str()?;

    let mut text = text.trim().to_string();
    if text.len() > LINE_CAP {
        // Truncate on a char boundary; a match preview can afford to be blunt.
        let cut = (0..=LINE_CAP).rev().find(|&i| text.is_char_boundary(i))?;
        text.truncate(cut);
        text.push('\u{2026}');
    }

    Some(Hit {
        // rg ran with `current_dir(root)`, so paths come back root-relative.
        path: root.join(path),
        line: line_number,
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rg_present() -> bool {
        Command::new("rg")
            .arg("--version")
            .stdout(Stdio::null())
            .status()
            .is_ok()
    }

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("omafiles-grep-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "one needle here\nplain line\n").unwrap();
        std::fs::write(dir.join("sub/b.txt"), "another needle\n").unwrap();
        std::fs::write(dir.join("c.txt"), "nothing to see\n").unwrap();
        dir
    }

    #[test]
    fn finds_matches_with_paths_and_line_numbers() {
        if !rg_present() {
            return;
        }
        let root = fixture("basic");
        let outcome = search(&root, "needle").expect("search");
        assert_eq!(outcome.hits.len(), 2);
        assert!(!outcome.truncated);

        let a = outcome
            .hits
            .iter()
            .find(|h| h.path.ends_with("a.txt"))
            .expect("a.txt hit");
        assert_eq!(a.line, 1);
        assert_eq!(a.text, "one needle here");
        assert!(a.path.is_absolute(), "paths resolve against the root");
        assert!(outcome.hits.iter().any(|h| h.path.ends_with("sub/b.txt")));
    }

    #[test]
    fn no_match_is_empty_not_an_error() {
        if !rg_present() {
            return;
        }
        let root = fixture("nomatch");
        let outcome = search(&root, "definitely-absent-string").expect("search");
        assert!(outcome.hits.is_empty());
        assert!(!outcome.truncated);
    }

    #[test]
    fn a_regex_metacharacter_is_just_text() {
        if !rg_present() {
            return;
        }
        let root = fixture("literal");
        std::fs::write(root.join("d.txt"), "weird (stuff [here\n").unwrap();
        let outcome = search(&root, "(stuff [").expect("fixed-strings never errors");
        assert_eq!(outcome.hits.len(), 1);
    }

    #[test]
    fn the_limit_truncates_and_says_so() {
        if !rg_present() {
            return;
        }
        let root = fixture("limit");
        // PER_FILE caps each file, so the flood needs many files.
        for i in 0..40 {
            let body = "needle\n".repeat(PER_FILE + 5);
            std::fs::write(root.join(format!("f{i}.txt")), body).unwrap();
        }
        let outcome = search(&root, "needle").expect("search");
        assert_eq!(outcome.hits.len(), LIMIT);
        assert!(outcome.truncated);
    }

    #[test]
    fn parse_line_reads_the_match_event_and_skips_the_rest() {
        let root = Path::new("/srv");
        let event = r#"{"type":"match","data":{"path":{"text":"x/y.rs"},"lines":{"text":"  let n = 1;\n"},"line_number":7,"absolute_offset":0,"submatches":[]}}"#;
        let hit = parse_line(root, event).expect("a match parses");
        assert_eq!(hit.path, PathBuf::from("/srv/x/y.rs"));
        assert_eq!(hit.line, 7);
        assert_eq!(hit.text, "let n = 1;");

        for other in [
            r#"{"type":"begin","data":{"path":{"text":"x"}}}"#,
            r#"{"type":"summary","data":{}}"#,
            "not json at all",
        ] {
            assert_eq!(parse_line(root, other), None);
        }
    }

    #[test]
    fn long_lines_are_capped_on_a_char_boundary() {
        let root = Path::new("/srv");
        let long = format!("é{}", "x".repeat(400));
        let event = format!(
            r#"{{"type":"match","data":{{"path":{{"text":"f"}},"lines":{{"text":"{long}"}},"line_number":1}}}}"#
        );
        let hit = parse_line(root, &event).expect("parses");
        assert!(hit.text.len() <= LINE_CAP + '\u{2026}'.len_utf8());
        assert!(hit.text.ends_with('\u{2026}'));
    }
}
