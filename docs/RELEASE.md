# Releasing omafiles

A release is a git tag. Pushing `vX.Y.Z` runs `.github/workflows/release.yml`,
which builds a pacman package for Omarchy (Arch Linux, x86_64), attaches it to a
GitHub release, and, when credentials exist, publishes `omafiles-bin` to the AUR.
Nothing is built or uploaded by hand.

## 1. Bump the version

The version lives in one place, `[workspace.package]` in the root `Cargo.toml`:

```toml
[workspace.package]
version = "0.2.0"
```

The workflow refuses to release if the tag and this value disagree, so bump it
first, then run the tests and a locked release build to make sure the lockfile
is still the pin it is meant to be:

```sh
cargo test --workspace
cargo build --release --locked -p omafiles
```

Commit the bump on its own, e.g. `Release 0.2.0`.

## 2. Try the package locally (optional, recommended)

This is exactly what the workflow does, minus the container. It reuses the local
`target/` so it does not rebuild gpui from scratch:

```sh
V=0.2.0
git archive --format=tar.gz --prefix="omafiles-$V/" -o "contrib/release/omafiles-$V.tar.gz" HEAD
cd contrib/release
sed -i "s/^pkgver=.*/pkgver=$V/" PKGBUILD
CARGO_TARGET_DIR="$PWD/../../target" makepkg -f -d --noconfirm
sudo pacman -U omafiles-$V-1-x86_64.pkg.tar.zst
omafiles --version
git checkout PKGBUILD && rm -rf src pkg *.tar.gz *.pkg.tar.zst
```

`git archive` reads the committed tree, so commit before running it.

## 3. Tag and push

```sh
git tag -a v0.2.0 -m "omafiles 0.2.0"
git push origin main v0.2.0
```

Then watch the *Release* workflow under the repository's Actions tab. A cold
run takes 15 to 20 minutes, a warm one about 5; the cargo registry, the Zed git
checkout and `target/` are cached between runs.

When it finishes the release page holds:

| File | Purpose |
| --- | --- |
| `omafiles-X.Y.Z-1-x86_64.pkg.tar.zst` | What users install with `sudo pacman -U` |
| `omafiles-X.Y.Z-x86_64-linux.tar.gz` | The installed tree (`usr/bin`, `usr/share`), consumed by the AUR `-bin` package |
| `SHA256SUMS` | Checksums of both |

Release notes are generated from the merged pull requests and commits since the
previous tag; edit them on the release page if they need a human sentence.

## 4. The AUR package

The `aur` job renders `contrib/aur/PKGBUILD` with the version and the tarball's
checksum, then pushes it to `ssh://aur@aur.archlinux.org/omafiles-bin.git`. It
runs only when the `AUR_SSH_PRIVATE_KEY` repository secret is set; otherwise it
logs a notice and is skipped, and the GitHub release is still complete.

One-time setup:

1. Create an account on <https://aur.archlinux.org> and add an SSH public key
   under *My Account*.
2. Store the matching private key as the `AUR_SSH_PRIVATE_KEY` secret in the
   GitHub repository settings.
3. The first push creates the package. After that, users install with
   `yay -S omafiles-bin`, and every tag updates it.

Once the AUR package exists, add the `yay` line to the website and README.

## 5. Update the install snippets

The README and `website/index.html` show a `curl -LO` of a versioned package
file name. Update the version in both after tagging; the `releases/latest`
URL always points at the newest release, but the file name inside it changes.

## If the workflow fails

- **"tag vX.Y.Z does not match Cargo.toml version"**: the bump was not
  committed before tagging. Fix `Cargo.toml`, then delete and re-push the tag.
- **Link errors mentioning `ts_*` symbols**: makepkg's `lto` option is on.
  Both PKGBUILDs set `options=('!debug' '!lto')`; keep that when editing them.
- **A full gpui rebuild on every run**: the cache was evicted (10 GB per-repo
  limit) or `rust-toolchain.toml` changed, which is part of the cache key.
  It recovers on the next run.
- **Re-running a release**: delete the tag locally and remotely, delete the
  draft or published release on GitHub, and push the tag again. Never reuse a
  tag for different content; bump `pkgrel` in `contrib/release/PKGBUILD`
  instead if only the packaging changed and the source did not.

## Where things live

| Path | Role |
| --- | --- |
| `.github/workflows/release.yml` | The whole pipeline |
| `contrib/release/PKGBUILD` | Versioned `omafiles` package, built from a tarball of the tag |
| `contrib/aur/PKGBUILD` | `omafiles-bin` template, `@VERSION@` and `@SHA256@` filled by the workflow |
| `contrib/PKGBUILD` | `omafiles-git`, builds a checkout's HEAD for local use |
| `rust-toolchain.toml` | Pinned toolchain, matched to Zed; the workflow honours it via rustup |
| `Cargo.lock` | The gpui pin; every build here is `--locked` |
