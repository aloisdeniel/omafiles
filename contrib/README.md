# Desktop integration

Everything here is **opt-in**. The package installs the binary and the
`.desktop` entry; the pieces that live in *your* configuration are yours to
apply, because writing into `$HOME` unasked is how software loses trust.

## Install the package

Prebuilt, from the [latest release](https://github.com/aloisdeniel/omafiles/releases/latest):

```sh
sudo pacman -U omafiles-<version>-1-x86_64.pkg.tar.zst
```

Or built from source, here:

```sh
cd contrib && makepkg -si
```

That builds the repository's current HEAD with `--locked` — the committed
`Cargo.lock` is the gpui pin and an unlocked build defeats it. The
`release/` and `aur/` PKGBUILDs next to it are what the release workflow
uses; you do not need them.

## Become the default file manager

Omarchy's default associations live in
`/usr/share/omarchy/default/applications/mimeapps.list` and point
`inode/directory` at Nautilus. Your own file wins; add to
`~/.config/mimeapps.list`:

```ini
[Default Applications]
inode/directory=dev.omarchy.omafiles.desktop
```

## Take over the SUPER+SHIFT+F keys

Append `hypr-bindings.lua` (installed at
`/usr/share/omafiles/hypr-bindings.lua`) to `~/.config/hypr/bindings.lua`, or
load it from there:

```lua
dofile("/usr/share/omafiles/hypr-bindings.lua")
```

## Theme-reload hook (belt and braces)

omafiles watches theme files directly; this hook only matters if inotify
watches are exhausted, which fails silently. Header of the script has the
details.

```sh
mkdir -p ~/.config/omarchy/hooks/theme-set.d ~/.config/omarchy/hooks/font-set.d
install -m755 /usr/share/omafiles/hooks/omafiles-theme-reload \
    ~/.config/omarchy/hooks/theme-set.d/omafiles-theme-reload
ln -sf ~/.config/omarchy/hooks/theme-set.d/omafiles-theme-reload \
    ~/.config/omarchy/hooks/font-set.d/omafiles-theme-reload
```

## Rebind keys

`~/.config/omafiles/keymap.toml`, merged over the defaults — naming an action
replaces its default keys in that section; an empty list unbinds. Sections are
`[global]`, `[listing]`, `[sidebar]`; names are the snake_case action names
(the command palette, `^k`, shows every action with its *effective* keys).

```toml
[listing]
toggle_preview = "p"           # replaces space
open_terminal = []             # unbinds t
[global]
new_tab = ["ctrl-t", "f7"]     # several keys for one action
```

A broken file never stops the app: the defaults stand and the problem is
shown in the status bar.

## Screenshots

`screenshots/take.sh` photographs the app for the website and social posts.
It runs the release binary in a throwaway home full of invented files, floats
the window on an empty Hyprland workspace, drives it with `wtype`, and
captures with `grim`; `--theme "Tokyo Night"` stages any stock theme without
touching your desktop. The header of the script has the options, and
[`docs/ANNOUNCEMENT.md`](../docs/ANNOUNCEMENT.md) says which shot goes with
which post.
