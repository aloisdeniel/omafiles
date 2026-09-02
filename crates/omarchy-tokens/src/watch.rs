//! Keeps [`Tokens`] current as the system changes.
//!
//! # The trap this exists to avoid
//!
//! `omarchy-theme-set` applies a theme by replacing the theme *directory*:
//!
//! ```text
//! rm -rf  ~/.local/state/omarchy/current/theme
//! mv      ~/.local/state/omarchy/current/next-theme  →  .../theme
//! ```
//!
//! The inode changes on every switch. An inotify watch placed on `theme/` or
//! on `theme/colors.toml` dies with `IN_DELETE_SELF`/`IN_MOVE_SELF` after the
//! *first* switch and then silently never fires again — the worst kind of bug,
//! because it works in testing and fails on the second theme change.
//!
//! `Color.qml` sidesteps this by setting `watchChanges: false` on the theme
//! files and taking a push over Quickshell IPC instead. We cannot receive that
//! push, so we watch the **parent** directory, which is stable across the swap.
//!
//! # Triggers
//!
//! Four independent sources, all collapsing into one debounced reload. The
//! redundancy costs a wasted re-parse and buys not silently going stale:
//!
//! 1. `~/.local/state/omarchy/current/` — the theme swap (parent, not `theme/`)
//! 2. `~/.config/omarchy/` — the user's `shell.toml`
//! 3. `~/.config/fontconfig/` — the monospace family, via `omarchy-font-set`
//! 4. Hyprland's IPC socket — `configreloaded`, for `decoration:rounding`
//!
//! Directories are watched rather than files throughout: editors and
//! `omarchy-font-set` write by rename, which detaches a file watch the same way
//! the theme swap does.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, SyncSender, sync_channel};
use std::time::{Duration, Instant};

use anyhow::Result;
use notify::{Event, RecursiveMode, Watcher as _};

use crate::{Paths, Tokens, load_from};

/// How long to let changes settle. A theme switch touches several files in
/// quick succession; reloading on the first would read a half-applied theme.
const DEBOUNCE: Duration = Duration::from_millis(120);

/// A live view of the system tokens.
///
/// Hold this for as long as you want updates — dropping it stops the watcher
/// threads.
pub struct Watcher {
    current: Tokens,
    updates: Receiver<Tokens>,
    /// Dropping these stops the notify backends.
    _watchers: Vec<notify::RecommendedWatcher>,
    /// Signals the debounce and Hyprland threads to exit.
    _shutdown: Sender<()>,
}

impl Watcher {
    /// The most recent tokens. Call [`Self::poll`] first to pick up changes.
    pub fn current(&self) -> &Tokens {
        &self.current
    }

    /// Apply any pending updates. Returns `true` if the tokens changed.
    ///
    /// Non-blocking, so it is safe to call from a render loop.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(tokens) = self.updates.try_recv() {
            if tokens != self.current {
                self.current = tokens;
                changed = true;
            }
        }
        changed
    }

    /// Block until the tokens change, or `timeout` elapses.
    ///
    /// Returns `true` if they changed. Intended for CLIs and tests; a UI should
    /// use [`Self::poll`].
    pub fn wait(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.poll() {
                return true;
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            match self.updates.recv_timeout(remaining.min(DEBOUNCE)) {
                Ok(tokens) => {
                    if tokens != self.current {
                        self.current = tokens;
                        return true;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return false,
            }
        }
    }
}

/// Start watching the real system paths.
pub fn watch() -> Result<Watcher> {
    watch_from(Paths::system())
}

/// Start watching a tree rooted at `paths`. Use this in tests.
pub fn watch_from(paths: Paths) -> Result<Watcher> {
    let current = load_from(&paths)?;

    // Bounded: if a consumer stops polling we want to drop stale snapshots
    // rather than grow without limit. Each message is a complete snapshot, so
    // dropping intermediate ones loses nothing.
    let (updates_tx, updates_rx) = sync_channel::<Tokens>(4);
    let (wake_tx, wake_rx) = std::sync::mpsc::channel::<()>();
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel::<()>();

    let watchers = spawn_fs_watchers(&paths, wake_tx.clone())?;
    spawn_hyprland_listener(wake_tx.clone());
    spawn_debouncer(paths, wake_rx, updates_tx, shutdown_rx);

    Ok(Watcher {
        current,
        updates: updates_rx,
        _watchers: watchers,
        _shutdown: shutdown_tx,
    })
}

/// Watch the three parent directories. Missing ones are skipped rather than
/// failing the whole watcher — a machine may have no `~/.config/fontconfig`
/// until the first `omarchy font set`.
fn spawn_fs_watchers(paths: &Paths, wake: Sender<()>) -> Result<Vec<notify::RecommendedWatcher>> {
    let targets: Vec<PathBuf> = vec![
        paths.state_current.clone(),
        paths.config.clone(),
        paths.fontconfig_dir(),
    ];

    let mut watchers = Vec::new();
    for dir in targets {
        if !dir.is_dir() {
            continue;
        }
        let wake = wake.clone();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
            // Any event is a reason to re-read; the reload is cheap and the
            // debouncer collapses bursts. Filtering by path here would mean
            // re-implementing the swap semantics we are trying to avoid caring
            // about.
            if event.is_ok() {
                let _ = wake.send(());
            }
        })?;
        // NonRecursive on purpose: `state_current` contains the whole theme
        // directory including wallpapers, and recursing would watch hundreds of
        // image files for no benefit.
        watcher.watch(&dir, RecursiveMode::NonRecursive)?;
        watchers.push(watcher);
    }

    Ok(watchers)
}

/// Collapse a burst of events into one reload.
fn spawn_debouncer(
    paths: Paths,
    wake: Receiver<()>,
    updates: SyncSender<Tokens>,
    shutdown: Receiver<()>,
) {
    std::thread::Builder::new()
        .name("omarchy-tokens-reload".into())
        .spawn(move || {
            loop {
                // Wait for the first event of a burst.
                if wake.recv().is_err() {
                    return;
                }
                if shutdown.try_recv() != Err(std::sync::mpsc::TryRecvError::Empty) {
                    return;
                }

                // Then drain until things go quiet. A theme switch writes
                // colors.toml, shell.toml and theme.name in sequence.
                while wake.recv_timeout(DEBOUNCE).is_ok() {}

                match load_from(&paths) {
                    Ok(tokens) => {
                        // A full channel means the consumer is behind; its next
                        // poll will read the newest snapshot anyway.
                        let _ = updates.try_send(tokens);
                    }
                    Err(err) => {
                        // Expected transiently: the theme directory does not
                        // exist between `rm -rf` and `mv`. The trailing events
                        // of the same burst will bring us back.
                        eprintln!("omarchy-tokens: reload failed, keeping previous: {err:#}");
                    }
                }
            }
        })
        .expect("spawn reload thread");
}

/// Subscribe to Hyprland's event socket for `configreloaded`, which is when
/// `decoration:rounding` and `general:gaps_out` may have changed.
///
/// Best-effort: no Hyprland (or a different compositor) simply means no
/// geometry updates, which is why this returns nothing and never errors.
fn spawn_hyprland_listener(wake: Sender<()>) {
    let Some(socket) = hyprland_event_socket() else {
        return;
    };

    std::thread::Builder::new()
        .name("omarchy-tokens-hyprland".into())
        .spawn(move || {
            let Ok(stream) = UnixStream::connect(&socket) else {
                return;
            };
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                // Events are `name>>data`. Only a config reload can change the
                // options we read.
                if line.starts_with("configreloaded") && wake.send(()).is_err() {
                    return;
                }
            }
        })
        .expect("spawn hyprland listener");
}

fn hyprland_event_socket() -> Option<PathBuf> {
    let signature = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let socket = Path::new(&runtime)
        .join("hypr")
        .join(signature)
        .join(".socket2.sock");
    socket.exists().then_some(socket)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::TempTree;

    const COLORS: &str = "mode = \"dark\"\n\
         background = \"#1a1b26\"\nforeground = \"#a9b1d6\"\naccent = \"#7aa2f7\"\n\
         red = \"#f7768e\"\nyellow = \"#e0af68\"\ngreen = \"#9ece6a\"\n\
         cyan = \"#449dab\"\nblue = \"#7aa2f7\"\nmagenta = \"#ad8ee6\"\nmuted = \"#414868\"\n";

    fn write_theme(paths: &Paths, dir: &Path, background: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("colors.toml"),
            COLORS.replace("#1a1b26", background),
        )
        .unwrap();
        std::fs::write(dir.join("shell.toml"), "[font]\nbase-size = 12\n").unwrap();
        let _ = paths;
    }

    fn setup(name: &str, background: &str) -> (TempTree, Paths) {
        let tree = TempTree::new(name);
        let paths = Paths::rooted(tree.path().to_path_buf());
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::create_dir_all(&paths.state_current).unwrap();
        write_theme(&paths, &paths.theme_dir(), background);
        std::fs::write(paths.theme_name(), "first\n").unwrap();
        (tree, paths)
    }

    /// Reproduce `omarchy-theme-set` exactly: build a sibling directory, then
    /// `rm -rf` the live one and `mv` the new one into place.
    fn swap_theme(paths: &Paths, background: &str, name: &str) {
        let next = paths.state_current.join("next-theme");
        let _ = std::fs::remove_dir_all(&next);
        write_theme(paths, &next, background);

        std::fs::remove_dir_all(paths.theme_dir()).unwrap();
        std::fs::rename(&next, paths.theme_dir()).unwrap();
        std::fs::write(paths.theme_name(), format!("{name}\n")).unwrap();
    }

    /// **The regression test for the whole module.** A watch on the theme
    /// directory itself survives exactly one swap; this asserts we survive
    /// five.
    #[test]
    fn survives_repeated_inode_swaps() {
        let (_tree, paths) = setup("inode-swap", "#111111");
        let mut watcher = watch_from(paths.clone()).unwrap();
        assert_eq!(watcher.current().palette.background().to_hex(), "#111111");

        let backgrounds = ["#222222", "#333333", "#444444", "#555555", "#666666"];
        for (i, background) in backgrounds.iter().enumerate() {
            swap_theme(&paths, background, &format!("theme{i}"));

            assert!(
                watcher.wait(Duration::from_secs(5)),
                "swap {i} ({background}) produced no update — the watch died on an \
                 earlier inode replacement"
            );
            assert_eq!(
                watcher.current().palette.background().to_hex(),
                *background,
                "swap {i} delivered the wrong palette"
            );
        }
    }

    #[test]
    fn picks_up_user_shell_toml_changes() {
        let (_tree, paths) = setup("user-shell", "#111111");
        let mut watcher = watch_from(paths.clone()).unwrap();
        assert_eq!(watcher.current().typography.base_size, 12.0);

        std::fs::write(paths.user_shell_toml(), "[font]\nbase-size = 18\n").unwrap();

        assert!(watcher.wait(Duration::from_secs(5)), "no update");
        assert_eq!(watcher.current().typography.base_size, 18.0);
        assert_eq!(watcher.current().typography.body(), 18.0);
    }

    /// A burst of writes must produce one settled reload, not one per write.
    #[test]
    fn debounces_a_burst_into_a_single_settled_reload() {
        let (_tree, paths) = setup("debounce", "#111111");
        let mut watcher = watch_from(paths.clone()).unwrap();

        for i in 0..10 {
            std::fs::write(
                paths.user_shell_toml(),
                format!("[font]\nbase-size = {}\n", 12 + i),
            )
            .unwrap();
        }

        assert!(watcher.wait(Duration::from_secs(5)), "no update");
        // Whatever arrives must be the *final* state, never an intermediate.
        assert_eq!(watcher.current().typography.base_size, 21.0);
    }

    /// Between `rm -rf` and `mv` there is no theme directory at all. The
    /// watcher must hold the previous tokens rather than erroring or clearing.
    #[test]
    fn keeps_previous_tokens_while_the_theme_directory_is_missing() {
        let (_tree, paths) = setup("torn-read", "#111111");
        let mut watcher = watch_from(paths.clone()).unwrap();

        std::fs::remove_dir_all(paths.theme_dir()).unwrap();
        watcher.wait(Duration::from_millis(600));
        assert_eq!(
            watcher.current().palette.background().to_hex(),
            "#111111",
            "must not clear or panic mid-swap"
        );

        write_theme(&paths, &paths.theme_dir(), "#999999");
        assert!(watcher.wait(Duration::from_secs(5)), "no recovery");
        assert_eq!(watcher.current().palette.background().to_hex(), "#999999");
    }

    /// `contrib/hooks/omafiles-theme-reload` works by touching a stamp file and
    /// relying on this watch to notice. It has no other channel, so if the
    /// watched set ever stops covering the config directory, the hook becomes a
    /// silent no-op. This asserts the coupling.
    #[test]
    fn a_write_anywhere_in_the_config_dir_triggers_a_reload() {
        let (_tree, paths) = setup("hook-stamp", "#111111");
        let mut watcher = watch_from(paths.clone()).unwrap();

        // Change something the reload will actually observe...
        std::fs::write(paths.user_shell_toml(), "[font]\nbase-size = 17\n").unwrap();
        // ...but announce it the way the hook does, with an unrelated file.
        std::fs::write(paths.config.join(".omafiles-reload"), "").unwrap();

        assert!(
            watcher.wait(Duration::from_secs(5)),
            "stamp file did not wake the watcher"
        );
        assert_eq!(watcher.current().typography.base_size, 17.0);
    }

    #[test]
    fn poll_is_non_blocking_and_reports_no_change() {
        let (_tree, paths) = setup("poll", "#111111");
        let mut watcher = watch_from(paths).unwrap();
        assert!(!watcher.poll(), "nothing changed yet");
    }

    #[test]
    fn watching_a_missing_tree_is_an_error() {
        let paths = Paths::rooted(std::env::temp_dir().join("omarchy-tokens-watch-absent"));
        assert!(watch_from(paths).is_err());
    }
}
