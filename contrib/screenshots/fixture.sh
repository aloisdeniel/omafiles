#!/bin/bash
# Builds the demo home the screenshots are taken in. Sourced by take.sh;
# exposes `build_fixture <home>` and writes only under that directory.
#
# Everything here is invented: a small web service called "lumen", a client
# folder, some pictures. The point is a home that looks lived-in without a
# single real file in it, so nothing private can end up on a social feed.

set -euo pipefail

# One-liners used by the image generators: a theme-agnostic set of colours
# that photograph well on light and dark themes alike.
FIXTURE_INK="#1e1f2e"
FIXTURE_PAPER="#f4f1ea"

build_fixture() {
  local home="$1"
  local repo="$home/Documents/Github/lumen"
  local clients="$home/Documents/Clients/Acme"

  mkdir -p "$home"/{Downloads,Documents,Pictures,Videos,.config,.local/share,.local/state,.cache}
  mkdir -p "$repo" "$clients"/{invoices,assets,contracts} "$home/Pictures/wallpapers" "$home/Pictures/screenshots"

  # XDG places, so the sidebar has something to derive.
  cat > "$home/.config/user-dirs.dirs" <<EOF
XDG_DOWNLOAD_DIR="\$HOME/Downloads"
XDG_DOCUMENTS_DIR="\$HOME/Documents"
XDG_PICTURES_DIR="\$HOME/Pictures"
XDG_VIDEOS_DIR="\$HOME/Videos"
XDG_PROJECTS_DIR="\$HOME/Documents/Github"
EOF

  # The font the user actually sees. fontconfig reads these from the home
  # directory, and a demo in the wrong monospace would not look like Omarchy.
  [[ -d "$REAL_HOME/.config/fontconfig" ]] && ln -s "$REAL_HOME/.config/fontconfig" "$home/.config/fontconfig"
  [[ -d "$REAL_HOME/.local/share/fonts" ]] && ln -s "$REAL_HOME/.local/share/fonts" "$home/.local/share/fonts"
  [[ -d "$REAL_HOME/.fonts" ]] && ln -s "$REAL_HOME/.fonts" "$home/.fonts"

  fixture_repo "$repo"
  fixture_media "$home"
  fixture_clients "$clients"
  fixture_downloads "$home/Downloads"
  fixture_omafiles_config "$home" "$repo" "$clients"
}

# ---------------------------------------------------------------------------
# The repository: enough languages to show the grammars, a real history with
# branches, and a working tree with every git state the icons can carry.
fixture_repo() {
  local repo="$1"
  mkdir -p "$repo"/{src,web,docs,scripts,assets,.github/workflows}

  cat > "$repo/README.md" <<'EOF'
# lumen

A small, fast link shortener you can run on one box. One binary, one SQLite
file, no admin panel to babysit.

## Why

Most shorteners are a service. lumen is a **program**: it starts in under a
second, serves from a single directory, and the whole configuration is the
`config.toml` next to it.

## Quick start

```sh
cargo run --release -- --config config.toml
curl -X POST localhost:8080/links -d 'https://omarchy.org'
```

## Features

- Redirects in ~40µs, measured with `oha` on a laptop
- Signed admin tokens, rotated from the CLI
- Click counts with **no** tracking pixels
- A `/healthz` endpoint that says what it means

| Route          | Method | Auth  |
| -------------- | ------ | ----- |
| `/links`       | POST   | token |
| `/{code}`      | GET    | none  |
| `/healthz`     | GET    | none  |

> Built for people who would rather read the source than the pricing page.
EOF

  cat > "$repo/Cargo.toml" <<'EOF'
[package]
name = "lumen"
version = "1.2.0"
edition = "2024"
description = "A link shortener that fits in one directory."
license = "MIT"

[dependencies]
axum = "0.8"
tokio = { version = "1", features = ["full"] }
rusqlite = { version = "0.32", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
toml = "1"
blake3 = "1"
anyhow = "1"

[profile.release]
lto = true
codegen-units = 1
EOF

  cat > "$repo/src/main.rs" <<'EOF'
//! lumen: a link shortener that fits in one directory.

mod auth;
mod routes;
mod store;

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;
use axum::Router;
use tokio::net::TcpListener;

#[derive(Debug, serde::Deserialize)]
struct Config {
    /// Where the SQLite file lives. Relative to the config file.
    database: PathBuf,
    /// The address to bind. Loopback unless you mean it.
    #[serde(default = "default_bind")]
    bind: SocketAddr,
    /// Short code length. Six gives ~56 billion codes.
    #[serde(default = "default_length")]
    code_length: usize,
}

fn default_bind() -> SocketAddr {
    "127.0.0.1:8080".parse().expect("a literal address")
}

fn default_length() -> usize {
    6
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(2).unwrap_or_else(|| "config.toml".into());
    let raw = std::fs::read_to_string(&path).with_context(|| format!("reading {path}"))?;
    let config: Config = toml::from_str(&raw).context("parsing config")?;

    let store = store::Store::open(&config.database)?;
    let app: Router = routes::router(store, config.code_length);

    let listener = TcpListener::bind(config.bind).await?;
    println!("lumen listening on http://{}", config.bind);
    axum::serve(listener, app).await?;
    Ok(())
}
EOF

  cat > "$repo/src/auth.rs" <<'EOF'
//! Admin tokens: a keyed hash, nothing to store but the key.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a freshly minted token stays valid.
pub const TOKEN_TTL: Duration = Duration::from_secs(60 * 60 * 24);

#[derive(Debug, Clone)]
pub struct Signer {
    key: [u8; 32],
}

impl Signer {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    /// `issued.signature` — issued as unix seconds, signature over it.
    pub fn mint(&self) -> String {
        let issued = now();
        format!("{issued}.{}", self.sign(issued))
    }

    pub fn verify(&self, token: &str) -> bool {
        let Some((issued, signature)) = token.split_once('.') else {
            return false;
        };
        let Ok(issued) = issued.parse::<u64>() else {
            return false;
        };
        if now().saturating_sub(issued) > TOKEN_TTL.as_secs() {
            return false;
        }
        constant_time_eq(self.sign(issued).as_bytes(), signature.as_bytes())
    }

    fn sign(&self, issued: u64) -> String {
        blake3::keyed_hash(&self.key, &issued.to_le_bytes()).to_hex().to_string()
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
EOF

  cat > "$repo/src/routes.rs" <<'EOF'
//! The three routes, and the auth check in front of the one that writes.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::Router;

use crate::auth::Signer;
use crate::store::Store;

#[derive(Clone)]
struct App {
    store: Store,
    signer: Signer,
    code_length: usize,
}

pub fn router(store: Store, code_length: usize) -> Router {
    let signer = Signer::new(store.key());
    Router::new()
        .route("/links", post(create))
        .route("/healthz", get(|| async { "ok\n" }))
        .route("/{code}", get(follow))
        .with_state(App { store, signer, code_length })
}

async fn create(State(app): State<App>, headers: HeaderMap, body: String) -> impl IntoResponse {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if !token.is_some_and(|t| app.signer.verify(t)) {
        return (StatusCode::UNAUTHORIZED, "bad token\n".to_string());
    }
    match app.store.insert(body.trim(), app.code_length) {
        Ok(code) => (StatusCode::CREATED, format!("{code}\n")),
        Err(err) => (StatusCode::BAD_REQUEST, format!("{err}\n")),
    }
}

async fn follow(State(app): State<App>, Path(code): Path<String>) -> impl IntoResponse {
    match app.store.lookup(&code) {
        Some(url) => Redirect::temporary(&url).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
EOF

  cat > "$repo/src/store.rs" <<'EOF'
//! SQLite, one table, and the code generator.

use std::path::Path;

use anyhow::anyhow;
use rusqlite::{params, Connection};

const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

#[derive(Clone)]
pub struct Store {
    path: std::path::PathBuf,
}

impl Store {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS links (
                code TEXT PRIMARY KEY,
                url  TEXT NOT NULL,
                hits INTEGER NOT NULL DEFAULT 0
            );",
        )?;
        Ok(Self { path: path.to_path_buf() })
    }

    pub fn key(&self) -> [u8; 32] {
        *blake3::hash(self.path.as_os_str().as_encoded_bytes()).as_bytes()
    }

    pub fn insert(&self, url: &str, length: usize) -> anyhow::Result<String> {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(anyhow!("not a URL: {url}"));
        }
        let db = Connection::open(&self.path)?;
        let code = code_for(url, length);
        db.execute("INSERT OR IGNORE INTO links (code, url) VALUES (?1, ?2)", params![code, url])?;
        Ok(code)
    }

    pub fn lookup(&self, code: &str) -> Option<String> {
        let db = Connection::open(&self.path).ok()?;
        db.execute("UPDATE links SET hits = hits + 1 WHERE code = ?1", params![code]).ok()?;
        db.query_row("SELECT url FROM links WHERE code = ?1", params![code], |r| r.get(0))
            .ok()
    }
}

/// Deterministic, so the same URL shortens to the same code.
fn code_for(url: &str, length: usize) -> String {
    blake3::hash(url.as_bytes())
        .as_bytes()
        .iter()
        .take(length)
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect()
}
EOF

  cat > "$repo/web/app.ts" <<'EOF'
// The whole front end: one form, one fetch, one line of output.

type Created = { code: string };

const form = document.querySelector<HTMLFormElement>("#shorten")!;
const output = document.querySelector<HTMLOutputElement>("#result")!;

async function shorten(url: string, token: string): Promise<Created> {
  const response = await fetch("/links", {
    method: "POST",
    headers: { Authorization: `Bearer ${token}` },
    body: url,
  });
  if (!response.ok) {
    throw new Error(`lumen said ${response.status}`);
  }
  return { code: (await response.text()).trim() };
}

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const data = new FormData(form);
  try {
    const { code } = await shorten(String(data.get("url")), String(data.get("token")));
    output.value = `${location.origin}/${code}`;
  } catch (error) {
    output.value = error instanceof Error ? error.message : "something broke";
  }
});
EOF

  cat > "$repo/web/index.html" <<'EOF'
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <title>lumen</title>
    <link rel="stylesheet" href="styles.css" />
  </head>
  <body>
    <main>
      <h1>lumen</h1>
      <form id="shorten">
        <input name="url" type="url" placeholder="https://…" required />
        <input name="token" type="password" placeholder="admin token" required />
        <button>Shorten</button>
      </form>
      <output id="result"></output>
    </main>
    <script type="module" src="app.js"></script>
  </body>
</html>
EOF

  cat > "$repo/web/styles.css" <<'EOF'
:root {
  --ink: #1e1f2e;
  --paper: #f4f1ea;
  --accent: #d97757;
  font-family: ui-monospace, "JetBrains Mono", monospace;
}

body {
  margin: 0;
  min-height: 100vh;
  display: grid;
  place-items: center;
  background: var(--paper);
  color: var(--ink);
}

form {
  display: flex;
  gap: 0.5rem;
}

button {
  background: var(--accent);
  color: var(--paper);
  border: 0;
  padding: 0.5rem 1rem;
  border-radius: 0.25rem;
}
EOF

  cat > "$repo/docs/api.md" <<'EOF'
# HTTP API

Everything speaks plain text. There is no JSON because there is nothing to nest.

## `POST /links`

Body: the URL. Header: `Authorization: Bearer <token>`.

Returns `201` and the short code, or `401` when the token is stale.

## `GET /{code}`

A `307` to the stored URL. Each follow bumps the click count.

## `GET /healthz`

`ok`. If it says anything else, believe it.
EOF

  cat > "$repo/config.toml" <<'EOF'
# lumen configuration. Paths are relative to this file.
database = "lumen.db"
bind = "127.0.0.1:8080"
code_length = 6
EOF

  cat > "$repo/scripts/deploy.sh" <<'EOF'
#!/bin/bash
# Build, copy, restart. Nothing clever.
set -euo pipefail

host="${1:-lumen.example.net}"

cargo build --release
scp target/release/lumen "$host:/opt/lumen/lumen.next"
ssh "$host" 'mv /opt/lumen/lumen.next /opt/lumen/lumen && systemctl restart lumen'
echo "deployed to $host"
EOF
  chmod +x "$repo/scripts/deploy.sh"

  cat > "$repo/.github/workflows/ci.yml" <<'EOF'
name: ci
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --locked
      - run: cargo clippy -- -D warnings
EOF

  cat > "$repo/.gitignore" <<'EOF'
/target
*.db
EOF

  cat > "$repo/LICENSE" <<'EOF'
MIT License

Copyright (c) 2026 lumen contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND.
EOF

  fixture_image "$repo/assets/logo.png" 512 512 "$FIXTURE_INK" "#d97757" "lumen"
  fixture_image "$repo/assets/hero.png" 1600 900 "#2b3a67" "#7aa2f7" ""

  # History: three commits on main, a develop branch checked out, and a
  # few more branches for the switcher to list.
  (
    cd "$repo"
    git init -q -b main
    git config user.name "Ada Lumen"
    git config user.email "ada@example.net"
    export GIT_AUTHOR_DATE="2026-08-20T10:00:00" GIT_COMMITTER_DATE="2026-08-20T10:00:00"
    git add -A && git commit -qm "lumen: a shortener that fits in one directory"
    export GIT_AUTHOR_DATE="2026-08-27T14:30:00" GIT_COMMITTER_DATE="2026-08-27T14:30:00"
    echo "- Signed tokens rotate from the CLI" >> docs/api.md
    git commit -qam "docs: rotation"
    git branch feature/click-stats
    git branch fix/login-redirect
    git branch release/1.2
    git checkout -qb develop
    unset GIT_AUTHOR_DATE GIT_COMMITTER_DATE
  )

  # The working tree after that: one modified file with a readable diff, one
  # staged new file, one untracked file. Each is a different marker.
  python3 - "$repo/src/auth.rs" <<'EOF'
import sys
path = sys.argv[1]
src = open(path).read()
src = src.replace(
    "pub const TOKEN_TTL: Duration = Duration::from_secs(60 * 60 * 24);",
    "pub const TOKEN_TTL: Duration = Duration::from_secs(60 * 60 * 12);",
)
src = src.replace(
    "        if now().saturating_sub(issued) > TOKEN_TTL.as_secs() {\n            return false;\n        }\n",
    "        let age = now().saturating_sub(issued);\n        if age > TOKEN_TTL.as_secs() {\n            return false;\n        }\n",
)
open(path, "w").write(src)
EOF
  cat > "$repo/docs/CHANGELOG.md" <<'EOF'
# Changelog

## 1.2.0

- Tokens now expire after twelve hours instead of a day.
- `/healthz` answers before the database is opened.
EOF
  cat > "$repo/src/cache.rs" <<'EOF'
//! An in-memory cache in front of the store. Not wired in yet.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Clone, Default)]
pub struct Cache(Arc<RwLock<HashMap<String, String>>>);
EOF
  (cd "$repo" && git add docs/CHANGELOG.md)

  # Timestamps that read as a project someone is working on, not one that
  # was generated a second ago.
  touch -d "9 days ago" "$repo/LICENSE" "$repo/.gitignore" "$repo/.github/workflows/ci.yml"
  touch -d "6 days ago" "$repo/web" "$repo/web/"* "$repo/scripts/deploy.sh" "$repo/config.toml"
  touch -d "3 days ago" "$repo/assets" "$repo/assets/"* "$repo/docs/api.md"
  touch -d "1 day ago" "$repo/README.md" "$repo/Cargo.toml" "$repo/src/main.rs" "$repo/src/store.rs"
  touch -d "2 hours ago" "$repo/src/routes.rs" "$repo/src/cache.rs"
  touch -d "4 minutes ago" "$repo/src/auth.rs" "$repo/docs/CHANGELOG.md"
}

# ---------------------------------------------------------------------------
# Pictures, an SVG, a short clip: the preview's non-text bodies.
fixture_media() {
  local home="$1"
  local pics="$home/Pictures"

  fixture_image "$pics/wallpapers/dunes.png" 1920 1080 "#c9a227" "#3b1f0e" ""
  fixture_image "$pics/wallpapers/harbor.png" 1920 1080 "#0f3057" "#00587a" ""
  fixture_image "$pics/wallpapers/moss.png" 1920 1080 "#1b4332" "#95d5b2" ""
  fixture_image "$pics/wallpapers/ember.png" 2560 1440 "#3d0000" "#ff7b54" ""
  fixture_image "$pics/screenshots/2026-08-30_14-02-11.png" 1280 800 "#2a2d3a" "#8b95c9" ""
  fixture_image "$pics/screenshots/2026-09-01_09-15-47.png" 1280 800 "#20232a" "#61dafb" ""
  fixture_image "$pics/passport-scan.jpg" 900 1200 "$FIXTURE_PAPER" "#d8d2c4" ""
  fixture_svg "$pics/lumen-mark.svg"

  if command -v ffmpeg >/dev/null; then
    ffmpeg -loglevel error -y -f lavfi -i "testsrc2=duration=4:size=1280x720:rate=24" \
      -pix_fmt yuv420p "$home/Videos/launch-teaser.mp4"
    ffmpeg -loglevel error -y -f lavfi -i "mandelbrot=size=960x540:rate=24" -t 3 \
      -pix_fmt yuv420p "$home/Videos/render-test.mp4"
  fi

  touch -d "12 days ago" "$pics/wallpapers/"*
  touch -d "3 days ago" "$pics/screenshots/2026-08-30_14-02-11.png"
  touch -d "1 day ago" "$pics/screenshots/2026-09-01_09-15-47.png" "$pics/lumen-mark.svg"
  touch -d "40 minutes ago" "$pics/passport-scan.jpg"
}

# A gradient with an optional word on it. Everything ImageMagick 7 does in
# one call; `magick` is what Omarchy ships.
fixture_image() {
  local out="$1" w="$2" h="$3" from="$4" to="$5" label="$6"
  local args=(-size "${w}x${h}" -define gradient:angle=35 "gradient:${from}-${to}" -gravity center)
  if [[ -n "$label" ]]; then
    args+=(-fill "$FIXTURE_PAPER" -pointsize $((h / 4)) -annotate 0 "$label")
  fi
  magick "${args[@]}" "$out"
}

fixture_svg() {
  cat > "$1" <<'EOF'
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="256" height="256">
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#d97757"/>
      <stop offset="1" stop-color="#7aa2f7"/>
    </linearGradient>
  </defs>
  <rect width="256" height="256" rx="48" fill="url(#g)"/>
  <circle cx="128" cy="128" r="64" fill="none" stroke="#f4f1ea" stroke-width="18"/>
  <circle cx="128" cy="128" r="18" fill="#f4f1ea"/>
</svg>
EOF
}

# ---------------------------------------------------------------------------
# A client folder: the workspace demo needs somewhere unrelated to the repo.
fixture_clients() {
  local dir="$1"
  local i
  for i in 2026-06 2026-07 2026-08; do
    printf 'Invoice %s\n\nAcme Corp — retainer\nAmount: 4,200.00 EUR\nDue: 30 days\n' "$i" > "$dir/invoices/$i.txt"
  done
  cat > "$dir/contracts/retainer-2026.md" <<'EOF'
# Retainer agreement

Twelve months, renewed monthly, terminable with thirty days' notice by either
side. Deliverables are listed per month in `invoices/`.
EOF
  fixture_image "$dir/assets/brand-mark.png" 800 800 "#0b132b" "#5bc0be" "Acme"
  fixture_image "$dir/assets/deck-cover.png" 1920 1080 "#1c2541" "#6fffe9" ""
  touch -d "70 days ago" "$dir/invoices/2026-06.txt"
  touch -d "40 days ago" "$dir/invoices/2026-07.txt"
  touch -d "9 days ago" "$dir/invoices/2026-08.txt"
  touch -d "5 days ago" "$dir/assets/"* "$dir/contracts/retainer-2026.md"
}

# ---------------------------------------------------------------------------
# Downloads: archives and a binary, for the sizes column and the hex preview.
fixture_downloads() {
  local dir="$1"
  head -c 3145728 /dev/urandom > "$dir/omarchy-3.2.1.iso.part"
  head -c 812345 /dev/urandom > "$dir/fonts-nerd-jetbrains.zip"
  head -c 214000 /dev/urandom > "$dir/lumen-1.2.0-x86_64.pkg.tar.zst"
  printf '%%PDF-1.4\n%% a placeholder, not a real document\n' > "$dir/onboarding.pdf"
  head -c 60000 /dev/urandom >> "$dir/onboarding.pdf"
  cp /bin/true "$dir/lumen" 2>/dev/null || head -c 40000 /dev/urandom > "$dir/lumen"
  chmod +x "$dir/lumen"
  touch -d "20 days ago" "$dir/fonts-nerd-jetbrains.zip"
  touch -d "2 days ago" "$dir/onboarding.pdf"
  touch -d "6 hours ago" "$dir/lumen-1.2.0-x86_64.pkg.tar.zst" "$dir/lumen"
  touch -d "20 minutes ago" "$dir/omarchy-3.2.1.iso.part"
}

# ---------------------------------------------------------------------------
# omafiles' own files: pins, a network location, and a session with
# workspaces already laid out so the sidebar tells the story on frame one.
fixture_omafiles_config() {
  local home="$1" repo="$2" clients="$3"
  mkdir -p "$home/.config/omafiles" "$home/.local/state/omafiles"

  cat > "$home/.config/omafiles/places.toml" <<EOF
version = 1

[[pin]]
path = "$repo"

[[pin]]
path = "$clients"
label = "Acme"
EOF

  cat > "$home/.config/omafiles/network.toml" <<'EOF'
[[location]]
name = "media"
uri = "smb://nas.local/media"

[[location]]
name = "deploy"
uri = "sftp://lumen.example.net/opt/lumen"
EOF

  cat > "$home/.local/state/omafiles/session.toml" <<EOF
version = 1
revision = 12
active = "global"

[[workspace]]
id = "global"
collapsed = false
active_tab = 0

[[workspace.tab]]
path = "$repo"
cursor = "README.md"

[[workspace.tab]]
path = "$home/Downloads"

[[workspace]]
id = "ws-acme"
name = "acme"
collapsed = false
active_tab = 0

[[workspace.tab]]
path = "$clients/invoices"
cursor = "2026-08.txt"

[[workspace.tab]]
path = "$clients/assets"

[[workspace]]
id = "ws-home"
name = "home"
collapsed = true
active_tab = 0

[[workspace.tab]]
path = "$home/Pictures/wallpapers"

[[workspace.tab]]
path = "$home/.config"
EOF
}
