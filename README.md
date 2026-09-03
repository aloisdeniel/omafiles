# Omafiles

The Omarchy file explorer. Keyboard-first, themed by Omarchy, one binary.

## Install

Prebuilt for Omarchy (Arch Linux, x86_64). Grab the package from the
[latest release](https://github.com/aloisdeniel/omafiles/releases/latest):

```sh
curl -LO https://github.com/aloisdeniel/omafiles/releases/latest/download/omafiles-0.0.1-1-x86_64.pkg.tar.zst
sudo pacman -U omafiles-0.0.1-1-x86_64.pkg.tar.zst
```

pacman installs the runtime dependencies (`git`, `ffmpeg`, `xdg-utils`),
the `.desktop` entry, and removes it all again with `pacman -R omafiles`.

To build from source instead (a ~15 minute gpui build):

```sh
git clone https://github.com/aloisdeniel/omafiles
cd omafiles/contrib && makepkg -si
```

Everything that touches your home directory (default file manager, the
`SUPER+SHIFT+F` binding, the theme hook) is opt-in and documented in
[`contrib/README.md`](contrib/README.md).

## Releasing

Bump `version` in `Cargo.toml`, commit, tag `vX.Y.Z` and push. The release
workflow builds the pacman package and publishes it. Full process, including
the optional AUR step, in [`docs/RELEASE.md`](docs/RELEASE.md).
