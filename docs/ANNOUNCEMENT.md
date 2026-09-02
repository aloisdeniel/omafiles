# Announcing omafiles

The launch thread for X, one post per section. Each post stays under 280
characters and names the screenshot to attach; the screenshots come from
`contrib/screenshots/take.sh`, which photographs a throwaway home so nothing
personal is in frame. The set referenced below was taken with:

```sh
contrib/screenshots/take.sh --theme current --theme "Tokyo Night" --theme "Catppuccin Latte"
```

Paths are relative to `docs/screenshots/`. Swap the theme directory for
whichever looks best on the day; every theme yields the same file names.

---

## 1

omafiles: the file explorer Omarchy was missing.

Keyboard-first, themed by Omarchy, one binary. Serve a directory in a keystroke, ask an agent about a file, read a diff where the preview goes, find by name and content in one window.

A thread on why, and what it does 🧵

📎 `themes.png`

## 2

Why another file explorer?

Finder has looked the same for ten years. The Linux ones are capable, but too inspired by Apple and Microsoft to fit a desktop like Omarchy.

"We can fix everything." So I started from the other end: what does a developer do in a file window all day?

📎 `ristretto/overview.png`

## 3

Themed by Omarchy itself, not by a theme of its own.

Colours from your colors.toml, sizes from shell.toml, corner radius from Hyprland. Run `omarchy theme set` and the window retints in a few hundred milliseconds, no keypress.

All 22 stock themes, light and dark.

📎 `catppuccin-latte/overview.png`

## 4

Three panes, no chrome.

Sidebar: places, pins, network, tabs, workspaces. Listing: name, size, age, a git marker on the icon. Detail panel: the preview, the facts, and the actions that make sense for this file.

Hairlines divide them. That is all the decoration there is.

📎 `tokyo-night/overview.png`

## 5

Vertical tabs, the way modern browsers and tools stack them.

Tabs live in the sidebar, not a strip on top. Each carries its own history, cursor and preview state. Everything restores across restarts and syncs across windows, from one session.toml.

^t opens one.

📎 `ristretto/overview.png`

## 6

Workspaces: group tabs by project.

New tabs land in the active workspace, so grouping needs no filing step. Collapse a group you are not in, drag tabs between groups, and the group holding your active tab lights up.

^n names a new one.

📎 `ristretto/workspace-new.png`

## 7

One search window for everything below here.

Empty, it lists recent files. Type, and it fuzzy-matches names as you go. Keep typing, and content matches arrive from ripgrep, deduplicated against the names. Enter lands you on the file, preview open.

Just `/`.

📎 `tokyo-night/search.png`

## 8

Previews that read the file.

Code through tree-sitter, sixteen grammars, coloured by your palette. Markdown rendered, not shown as source. Images, SVG, animated GIF. Video gets a poster frame plus what ffprobe knows. Binaries get a hex head and a guess.

📎 `tokyo-night/preview-code.png`

## 9

Space expands the preview over the listing and the panel, and leaves the sidebar alone, so you can still change directory.

j and k flick through the folder without collapsing. A folder of screenshots becomes a light table.

📎 `tokyo-night/preview-image-expanded.png`

## 10

A file manager sitting in a repository should say so.

The branch and change counts live in the status bar. Every entry's icon carries its status. A changed file previews as its diff, rendered as the file with a wash on the changed lines, not as a patch.

^⇧g switches branch.

📎 `tokyo-night/git-diff-expanded.png`

## 11

Serve any directory over HTTP in one keystroke: ^s.

Loopback by default; the network is a second, explicit choice with a warning. You get the URL and a live request log.

Servers are detached processes. Close the window, they keep answering. A globe lists them all.

📎 `ristretto/server-list.png`

## 12

Everything is a keystroke.

Vim-ish movement, key contexts per pane, arrows too. `?` opens the full shortcut sheet with a filter. ^k opens a palette listing every action with its effective binding, including the ones you rebound in ~/.config/omafiles/keymap.toml.

📎 `ristretto/help.png`

## 13

Act on a file without leaving.

t: a terminal here. a: ask your agent about it. s: share via LocalSend. z: zip. Copy, cut, paste, move, new file. Delete asks first, then trashes.

Copy a picture and it asks: the file, or a PNG at one of several sizes, ready to paste in a browser.

📎 `ristretto/copy-image.png`

## 14

Network locations: SMB, SFTP, WebDAV through GVfs.

Add one with ^⇧n, and a mount is just a directory. The listing, the preview and the search browse it with no special cases.

📎 `tokyo-night/network-add.png`

## 15

The panels get out of the way.

^b folds the sidebar, ^⇧b the detail panel. Below a width they dock as overlays instead of taking space, and widening the window brings them back.

📎 `ristretto/listing-only.png`

## 16

Small, native, honest.

Rust, on gpui, the framework Zed is built on. Three crates: pure token logic, a design system, the app. A couple of hundred tests, clippy clean, MIT.

No Electron, no webview, one binary.

## 17

Install it on your Omarchy: a prebuilt pacman package for x86_64, a .desktop entry, and opt-in snippets for the SUPER+SHIFT+F takeover. Nothing writes into your home directory unasked.

Source, package and docs: https://github.com/aloisdeniel/omafiles

📎 `catppuccin-latte/overview.png`
