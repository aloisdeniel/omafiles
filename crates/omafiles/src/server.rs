//! M10's HTTP server: serve a directory over HTTP (§6.7) — since revised to
//! **outlive the window**: the app spawns each server as a detached process
//! (itself, re-exec'd with `--serve`), tracked through a small on-disk
//! registry, so closing omafiles leaves the ports serving and a relaunched
//! omafiles finds them again. §6.7's "stops on app exit" is deliberately
//! reversed, by request; stopping is now the globe list's kill button.
//!
//! In-process `axum` rather than a spawned `miniserve` or `python -m
//! http.server`, and the plan's reasons are all lifecycle: stopping is
//! dropping the [`Handle`], status (bound port, request count, the log) is
//! read from the same process, the port auto-selects on conflict, and there is
//! no pid to orphan. The tokio runtime axum needs lives on **one background
//! thread owned by the handle** — the rest of the app never touches tokio.
//!
//! Two facts pinned at start and never silently changed:
//!
//! - **The root is the directory that was current when the server started.**
//!   Navigating away does not move it — a server that follows the browsing
//!   would be sharing whatever the user happened to look at next.
//! - **The bind address.** `127.0.0.1` by default; `0.0.0.0` only as the
//!   caller's second, explicit choice — that is a real exposure and must never
//!   happen by accident.
//!
//! Files are served by `tower-http`'s `ServeDir` (mime types, ranges,
//! streaming — none of which deserve reimplementing); directory listings are
//! ours, because `ServeDir` has none.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use tower::ServiceExt as _;
use tower_http::services::ServeDir;

use crate::entry::natural_cmp;

/// The port asked for first. On conflict the OS picks instead — a second
/// server on another directory is a legitimate thing to want.
const PORT: u16 = 8080;

/// Log lines kept. A ring, not a file: the log's job is "is it working and
/// who asked for what", not an audit trail.
const LOG_LINES: usize = 200;

/// A running server, held in-process. The serving *process* (`--serve`) owns
/// one; dropping it stops serving. The windowed app holds none — it tracks
/// detached serving processes through the registry below.
pub struct Handle {
    /// The directory being served, pinned at start.
    pub root: PathBuf,
    /// Where it actually bound — the port may not be [`PORT`].
    pub addr: SocketAddr,
    /// Whether this was the explicit "everyone on the network" choice.
    pub lan: bool,
    log: Arc<Mutex<VecDeque<String>>>,
    hits: Arc<AtomicU64>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Handle {
    /// The address to hand to a browser — or to a phone, when on the LAN.
    ///
    /// A LAN bind advertises the machine's routable address rather than the
    /// meaningless `0.0.0.0` it bound to.
    pub fn url(&self) -> String {
        let host = if self.lan {
            lan_ip().unwrap_or_else(|| self.addr.ip())
        } else {
            self.addr.ip()
        };
        format!("http://{host}:{}/", self.addr.port())
    }

    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    /// The most recent log lines, oldest first.
    pub fn log(&self) -> Vec<String> {
        self.log
            .lock()
            .map(|log| log.iter().cloned().collect())
            .unwrap_or_default()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What the request handler needs; one `Arc` shared with the [`Handle`].
struct Served {
    root: PathBuf,
    log: Arc<Mutex<VecDeque<String>>>,
    hits: Arc<AtomicU64>,
    started: Instant,
}

/// Start serving `root`. Binds **synchronously**, so a taken port or a denied
/// bind is an error here and now — not a background failure discovered later.
pub fn start(root: PathBuf, lan: bool) -> Result<Handle, String> {
    let host: IpAddr = if lan {
        Ipv4Addr::UNSPECIFIED.into()
    } else {
        Ipv4Addr::LOCALHOST.into()
    };

    // The preferred port, or whatever the OS gives: a conflict means someone
    // is already serving — probably us, in another window — and "a different
    // port" beats "an error" for the second server.
    let listener = std::net::TcpListener::bind(SocketAddr::new(host, PORT))
        .or_else(|first| {
            if first.kind() == std::io::ErrorKind::AddrInUse {
                std::net::TcpListener::bind(SocketAddr::new(host, 0))
            } else {
                Err(first)
            }
        })
        .map_err(|err| format!("could not bind: {err}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|err| format!("could not configure the socket: {err}"))?;
    let addr = listener
        .local_addr()
        .map_err(|err| format!("could not read the bound address: {err}"))?;

    let log = Arc::new(Mutex::new(VecDeque::new()));
    let hits = Arc::new(AtomicU64::new(0));
    let served = Arc::new(Served {
        root: root.clone(),
        log: log.clone(),
        hits: hits.clone(),
        started: Instant::now(),
    });
    let (shutdown, rx) = tokio::sync::oneshot::channel::<()>();

    // One current-thread runtime on one named thread. The app's executor never
    // learns tokio exists, and the thread ends when the shutdown fires.
    let thread = std::thread::Builder::new()
        .name("omafiles-http".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => return,
            };
            runtime.block_on(async move {
                let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                    return;
                };
                let app = axum::Router::new()
                    .fallback(axum::routing::get(handle))
                    .with_state(served);
                let _ = axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = rx.await;
                    })
                    .await;
            });
        })
        .map_err(|err| format!("could not start the server thread: {err}"))?;

    Ok(Handle {
        root,
        addr,
        lan,
        log,
        hits,
        shutdown: Some(shutdown),
        thread: Some(thread),
    })
}

/// Every request: a directory renders our listing, anything else goes to
/// `ServeDir`, and both leave one line in the log.
async fn handle(State(served): State<Arc<Served>>, request: Request<Body>) -> Response {
    let path = request.uri().path().to_string();

    let response = match local_path(&served.root, &path) {
        // Escapes and undecodable paths are a 404, not a 403: which paths
        // exist outside the root is exactly what must not leak.
        None => StatusCode::NOT_FOUND.into_response(),
        Some(full) if full.is_dir() => index(&served.root, &full, &path),
        // Files: mime, ranges and streaming are ServeDir's problem. It
        // resolves the path itself from the original URI.
        Some(_) => match ServeDir::new(&served.root).oneshot(request).await {
            Ok(response) => response.into_response(),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        },
    };

    served.hits.fetch_add(1, Ordering::Relaxed);
    let elapsed = served.started.elapsed().as_secs();
    let line = format!(
        "+{:>4}s  GET {}  \u{2192} {}",
        elapsed,
        path,
        response.status().as_u16()
    );
    if let Ok(mut log) = served.log.lock() {
        if log.len() == LOG_LINES {
            log.pop_front();
        }
        log.push_back(line);
    }
    response
}

/// Resolve a request path to a filesystem path under `root`, or refuse.
///
/// Built from decoded components joined one by one — never `root.join(path)`,
/// which an absolute or `..`-carrying path walks right out of.
fn local_path(root: &Path, request_path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(request_path)?;
    let mut full = root.to_path_buf();
    for component in decoded.split('/') {
        match component {
            "" | "." => continue,
            ".." => return None,
            name => full.push(name),
        }
    }
    Some(full)
}

/// The directory listing `ServeDir` does not have.
fn index(root: &Path, dir: &Path, request_path: &str) -> Response {
    let mut entries: Vec<(String, bool)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            // Dotfiles stay private by default, matching the app's own
            // listing. (`Ctrl-H` in the window does not reach over HTTP.)
            if name.starts_with('.') {
                return None;
            }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            Some((name, is_dir))
        })
        .collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| natural_cmp(&a.0, &b.0)));

    let shown = if dir == root {
        "/".to_string()
    } else {
        let trimmed = request_path.trim_end_matches('/');
        format!("{trimmed}/")
    };
    let base = if request_path.ends_with('/') {
        request_path.to_string()
    } else {
        format!("{request_path}/")
    };

    let mut body = String::new();
    let _ = write!(
        body,
        "<!doctype html><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title}</title>\
         <style>\
         body{{font-family:monospace;background:#111;color:#ddd;margin:2rem auto;\
         max-width:40rem;padding:0 1rem}}\
         a{{color:#8ab4f8;text-decoration:none;display:block;padding:.25rem 0}}\
         a:hover{{text-decoration:underline}}h1{{font-size:1rem;color:#888}}\
         </style>\
         <h1>{title}</h1>",
        title = escape(&shown)
    );
    if dir != root {
        let _ = write!(body, "<a href=\"{}..\">../</a>", escape(&base));
    }
    for (name, is_dir) in entries {
        let slash = if is_dir { "/" } else { "" };
        let _ = write!(
            body,
            "<a href=\"{base}{href}{slash}\">{text}{slash}</a>",
            base = escape(&base),
            href = escape(&percent_encode(&name)),
            text = escape(&name),
        );
    }

    ([(header::CACHE_CONTROL, "no-cache")], Html(body)).into_response()
}

/// The machine's routable address, for the URL a phone would use.
///
/// The connect never sends a packet — it only makes the OS pick the outgoing
/// interface, which is the classic std-only way to learn one's own address.
fn lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    Some(socket.local_addr().ok()?.ip())
}

/// Decode `%XX` escapes. `None` on malformed input or broken UTF-8 — a path
/// we cannot read precisely is a path we must not resolve approximately.
fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Encode a file name for use inside a URL path.
fn percent_encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for byte in name.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// Minimal HTML escaping for names and hrefs we interpolate.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}


// --------------------------------------------------------------- the registry
//
// The windowed app does not hold servers — it spawns them detached and reads
// this registry: one small TOML per serving process, written by the process
// itself, beside a log file it keeps rewritten. A relaunched app lists the
// same directory, which is what makes servers outlive any one window.

/// A detached serving process, as the registry records it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Info {
    pub pid: u32,
    pub root: PathBuf,
    pub port: u16,
    pub lan: bool,
    pub hits: u64,
}

impl Info {
    /// The address to hand to a browser — or to a phone, when on the LAN.
    pub fn url(&self) -> String {
        let host: IpAddr = if self.lan {
            lan_ip().unwrap_or_else(|| Ipv4Addr::UNSPECIFIED.into())
        } else {
            Ipv4Addr::LOCALHOST.into()
        };
        format!("http://{host}:{}/", self.port)
    }
}

/// `~/.local/state/omafiles/servers/`.
fn registry_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"))
                .join(".local/state")
        })
        .join("omafiles/servers")
}

fn info_path(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("{pid}.toml"))
}

fn log_path(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("{pid}.log"))
}

/// Is the process still with us? Linux-only, like the app.
fn alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Every serving process the registry knows, dead ones swept as found.
///
/// The sweep is what makes ungraceful ends harmless: a killed process cannot
/// remove its own files, so the next listing does.
pub fn list() -> Vec<Info> {
    list_in(&registry_dir())
}

fn list_in(dir: &Path) -> Vec<Info> {
    let mut found: Vec<Info> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|e| e == "toml"))
        .filter_map(|entry| {
            let info: Info = toml::from_str(&std::fs::read_to_string(entry.path()).ok()?).ok()?;
            if alive(info.pid) {
                Some(info)
            } else {
                let _ = std::fs::remove_file(entry.path());
                let _ = std::fs::remove_file(log_path(dir, info.pid));
                None
            }
        })
        .collect();
    found.sort_by_key(|info| info.port);
    found
}

/// Start serving `root` in a process of its own, detached from this one.
///
/// The app re-execs itself with `--serve` under `setsid`, so closing the
/// window — or the terminal that launched it — leaves the server running.
/// The child binds and registers itself; the caller sees it appear in
/// [`list`] a moment later.
pub fn spawn_detached(root: &Path, lan: bool) -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|err| format!("could not find the omafiles binary: {err}"))?;
    let mut command = std::process::Command::new("setsid");
    command.arg(exe).arg("--serve").arg(root);
    if lan {
        command.arg("--lan");
    }
    let child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|err| format!("could not spawn the server process: {err}"))?;
    // Reap the direct child (setsid exits at once) so it cannot sit defunct.
    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });
    Ok(())
}

/// Stop one serving process. SIGTERM, never -9: axum gets to finish the
/// requests in flight.
pub fn stop(pid: u32) -> Result<(), String> {
    let status = std::process::Command::new("kill")
        .arg(pid.to_string())
        .status()
        .map_err(|err| format!("could not run kill: {err}"))?;
    if !status.success() {
        return Err(format!("could not stop the server (pid {pid})"));
    }
    // The killed process cannot clean up after itself.
    let dir = registry_dir();
    let _ = std::fs::remove_file(info_path(&dir, pid));
    let _ = std::fs::remove_file(log_path(&dir, pid));
    Ok(())
}

/// The most recent log lines of one serving process, oldest first.
pub fn read_log(pid: u32) -> Vec<String> {
    std::fs::read_to_string(log_path(&registry_dir(), pid))
        .map(|text| text.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Write one process's registry entry and log, atomically (temp + rename —
/// a reader must never see half a file).
fn write_state_in(dir: &Path, info: &Info, log: &[String]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let body = toml::to_string(info).unwrap_or_default();
    let tmp = dir.join(format!("{}.toml.tmp", info.pid));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, info_path(dir, info.pid))?;
    let tmp = dir.join(format!("{}.log.tmp", info.pid));
    std::fs::write(&tmp, log.join("\n"))?;
    std::fs::rename(&tmp, log_path(dir, info.pid))?;
    Ok(())
}

/// The `--serve` entry point: serve until killed.
///
/// Registers itself, then keeps the registry entry and log file current —
/// the ring stays capped in memory and the files are rewritten whole, so
/// neither grows without bound. Returns only if serving could not start.
pub fn serve_forever(root: PathBuf, lan: bool) -> Result<std::convert::Infallible, String> {
    let handle = start(root, lan)?;
    let dir = registry_dir();
    let info = |hits: u64| Info {
        pid: std::process::id(),
        root: handle.root.clone(),
        port: handle.addr.port(),
        lan: handle.lan,
        hits,
    };
    write_state_in(&dir, &info(0), &[]).map_err(|err| format!("could not register: {err}"))?;

    let mut written = 0_u64;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let hits = handle.hits();
        if hits != written {
            written = hits;
            let _ = write_state_in(&dir, &info(hits), &handle.log());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;

    /// One blocking HTTP/1.1 request, raw over a socket: the tests should not
    /// grow an HTTP client dependency to talk to their own server.
    fn get(addr: SocketAddr, path: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(addr).expect("connect");
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n"
        )
        .expect("write");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read");
        let status = response
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        (status, response)
    }

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("omafiles-http-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("hello.txt"), "hello over http").unwrap();
        std::fs::write(dir.join("with space.txt"), "spaced").unwrap();
        std::fs::write(dir.join(".secret"), "not listed").unwrap();
        std::fs::write(dir.join("sub/inner.txt"), "inner").unwrap();
        dir
    }

    #[test]
    fn serves_a_listing_and_a_file() {
        let dir = fixture("listing");
        let server = start(dir.clone(), false).expect("start");

        let (status, body) = get(server.addr, "/");
        assert_eq!(status, 200);
        assert!(body.contains("hello.txt"));
        assert!(body.contains("sub/"), "directories carry a trailing slash");
        assert!(
            !body.contains(".secret"),
            "dotfiles stay out of the listing"
        );

        let (status, body) = get(server.addr, "/hello.txt");
        assert_eq!(status, 200);
        assert!(body.contains("hello over http"));

        let (status, body) = get(server.addr, "/sub/");
        assert_eq!(status, 200);
        assert!(body.contains("inner.txt"));
        assert!(body.contains(".."), "a subdirectory links back up");

        // A file with a space round-trips through the encoded link.
        let (status, body) = get(server.addr, "/with%20space.txt");
        assert_eq!(status, 200);
        assert!(body.contains("spaced"));

        assert_eq!(server.hits(), 4);
        assert_eq!(server.log().len(), 4);
    }

    #[test]
    fn refuses_to_leave_the_root() {
        let dir = fixture("escape");
        let server = start(dir.clone(), false).expect("start");

        for path in ["/../", "/%2e%2e/", "/sub/../../", "/..%2f..%2f"] {
            let (status, _) = get(server.addr, path);
            assert_ne!(status, 200, "{path} must not resolve");
        }
    }

    #[test]
    fn dropping_the_handle_stops_the_server() {
        let dir = fixture("stop");
        let server = start(dir.clone(), false).expect("start");
        let addr = server.addr;
        let (status, _) = get(addr, "/");
        assert_eq!(status, 200);

        drop(server);
        // The drop joins the serving thread, so by here the listener is gone.
        assert!(
            TcpStream::connect(addr).is_err(),
            "the port must be released"
        );
    }

    #[test]
    fn a_taken_port_falls_back_rather_than_failing() {
        let dir = fixture("conflict");
        let first = start(dir.clone(), false).expect("first");
        let second = start(dir.clone(), false).expect("second");
        assert_ne!(first.addr.port(), second.addr.port());
        let (status, _) = get(second.addr, "/");
        assert_eq!(status, 200);
    }

    #[test]
    fn the_loopback_url_names_loopback() {
        let dir = fixture("url");
        let server = start(dir.clone(), false).expect("start");
        assert!(server.url().starts_with("http://127.0.0.1:"));
    }

    #[test]
    fn path_resolution_never_escapes() {
        let root = Path::new("/srv/files");
        assert_eq!(
            local_path(root, "/a/b.txt"),
            Some(PathBuf::from("/srv/files/a/b.txt"))
        );
        assert_eq!(local_path(root, "/"), Some(PathBuf::from("/srv/files")));
        assert_eq!(local_path(root, "/../x"), None);
        assert_eq!(local_path(root, "/%2e%2e/x"), None);
        assert_eq!(local_path(root, "/a/%2e%2e/%2e%2e/x"), None);
        assert_eq!(local_path(root, "/%zz"), None, "malformed escape");
    }

    #[test]
    fn encoding_round_trips() {
        assert_eq!(percent_encode("with space.txt"), "with%20space.txt");
        assert_eq!(
            percent_decode("with%20space.txt").as_deref(),
            Some("with space.txt")
        );
        assert_eq!(escape("a<b>&\"c\""), "a&lt;b&gt;&amp;&quot;c&quot;");
    }
}
    #[test]
    fn the_registry_lists_the_living_and_sweeps_the_dead() {
        let dir = std::env::temp_dir().join(format!("omafiles-reg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let me = Info {
            pid: std::process::id(),
            root: PathBuf::from("/srv/live"),
            port: 8080,
            lan: false,
            hits: 3,
        };
        let ghost = Info {
            pid: u32::MAX - 1, // no such process
            root: PathBuf::from("/srv/dead"),
            port: 9090,
            lan: true,
            hits: 0,
        };
        write_state_in(&dir, &me, &["+1s GET / -> 200".to_string()]).unwrap();
        write_state_in(&dir, &ghost, &[]).unwrap();

        let listed = list_in(&dir);
        assert_eq!(listed, vec![me.clone()]);
        assert!(
            !info_path(&dir, ghost.pid).exists(),
            "the dead entry is swept"
        );
        assert!(!log_path(&dir, ghost.pid).exists());
        assert!(info_path(&dir, me.pid).exists());

        // The URL comes from the record, not from a live handle.
        assert_eq!(me.url(), "http://127.0.0.1:8080/");
        assert!(ghost.url().starts_with("http://"));
    }

