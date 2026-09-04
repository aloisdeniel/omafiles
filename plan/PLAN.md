# omafiles — Plan

A keyboard-first file explorer for Omarchy, written in Rust on gpui, plus a reusable
Omarchy design-system crate that keeps any gpui app visually in lockstep with the live
system theme.

Target system for this plan (verified on this machine, 2026-09-01):

| Thing | Value |
| --- | --- |
| Distro | Omarchy 4.0.1 (`BUILD_ID` in `/etc/os-release`), Arch-based |
| Compositor | Hyprland 0.56.2 |
| Shell | `omarchy-shell` — Quickshell/QML, not Waybar |
| Omarchy root | `/usr/share/omarchy` (`$OMARCHY_PATH`) |
| Present already | `fzf`, `rg`, `fd`, `jq`, `ffmpeg`/`ffprobe`, `mpv`, `bat`, `localsend`, `gio`, `wl-copy`, `fc-match` |
| GPU | AMD Renoir (Vega iGPU), RADV / Vulkan 1.4 loader present |
| Toolchain manager | `mise` (bun, node, flutter, gh, … all managed there) |
| Toolchain | Rust **1.97.1** via mise, pinned in `rust-toolchain.toml` (matches Zed's own pin) |

---

## 1. Why this shape

The headline discovery from surveying the system: **Omarchy 4 already has a real design
system.** It is not just a colour palette. `omarchy-shell` ships
`/usr/share/omarchy/shell/Commons/Style.qml`, a 515-line token system with a typographic
scale, a spacing scale, interactive-state tokens (normal / hover-cursor / focus /
selected / pressed), and per-surface tokens for popups, tooltips, menus, notifications
and the launcher. Themes and users feed it through `colors.toml` and `shell.toml`.

That changes the goal for the design-system crate. It should **not** invent an Omarchy
"vibe". It should be a faithful Rust/gpui port of `Style.qml` + `Color.qml`, so that
omafiles looks like it was built by the same people who built the bar and the menu —
because it is reading the same tokens, with the same defaults and the same resolution
rules. Getting this right is what makes the app feel native rather than themed-to-match.

Everything in §2 below is the reverse-engineered spec for that port.

---

## 2. What Omarchy actually exposes (verified)

### 2.1 The files

| Path | Role | Watchable? |
| --- | --- | --- |
| `~/.local/state/omarchy/current/theme/` | Active theme, **directory** | Not directly — see 2.5 |
| `~/.local/state/omarchy/current/theme/colors.toml` | Palette | via parent |
| `~/.local/state/omarchy/current/theme/shell.toml` | Generated structural tokens | via parent |
| `~/.local/state/omarchy/current/theme.name` | e.g. `tokyo-night` | via parent |
| `~/.local/state/omarchy/current/background` | Symlink to wallpaper | via parent |
| `~/.config/omarchy/shell.toml` | **User overrides — these win** | yes, directly |
| `~/.config/fontconfig/fonts.conf` | Monospace family (written by `omarchy-font-set`) | yes |
| `~/.config/omarchy/hooks/theme-set.d/*` | Executed after every theme change | — |
| `~/.config/omarchy/hooks/font-set.d/*` | Executed after every font change | — |
| `~/.config/omarchy/themed/*.tpl` | User templates rendered into the theme dir | — |
| Hyprland IPC `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket2.sock` | Corner radius + gaps source | yes |

### 2.2 `colors.toml`

23 keys. Verified shape (`tokyo-night`):

```toml
mode = "dark"                      # or "light"
accent = "#7aa2f7"
selection = "#292e42"
muted = "#414868"
background = "#1a1b26"
dark_background = "#13141c"
darker_background = "#0e0e14"
lighter_background = "#24283b"
foreground = "#a9b1d6"
dark_foreground = "#565f89"
light_foreground = "#b4bee6"
bright_foreground = "#c0caf5"
red / yellow / orange / green / cyan / blue / magenta / brown
bright_red / bright_yellow / bright_green / bright_cyan / bright_blue / bright_magenta
```

Omarchy resolves these into a wider alias set. `omarchy-theme-color --file <path> --all`
prints the full resolved map as `key\tvalue` — 56 keys for `tokyo-night`, adding:

- short aliases: `bg`, `fg`, `dark_bg`, `darker_bg`, `lighter_bg`, `dark_fg`, `light_fg`, `bright_fg`
- `purple` → `magenta`, `bright_purple` → `bright_magenta`
- `cursor` → `bright_foreground`
- `selection_background` → `selection`, `selection_foreground` → derived from `bright_foreground`
- `theme_type` → `mode`
- the legacy 16-colour terminal palette `color0`..`color15`

**Decision:** reimplement this resolution in Rust rather than shelling out to
`omarchy-theme-color` per read. It is a pure function over 23 inputs, it must run on a
hot path (theme switch during a render), and shelling out adds a fork per reload. We
keep `omarchy-theme-color --all` as the **conformance oracle**: a test asserts our
resolver produces byte-identical output for all 22 stock themes in
`/usr/share/omarchy/themes/`. That test is the thing that stops us drifting when Omarchy
adds a key.

### 2.2b The theme corpus — what we must survive

All 22 stock themes ship a `colors.toml` (none rely on the legacy
alacritty-derived path). 17 are `mode = "dark"`, 5 are `mode = "light"`
(`catppuccin-latte`, `flexoki-light`, `lupine`, `rose-pine`, `white`). **Light mode is
not an edge case** — nearly a quarter of the corpus. Any hardcoded assumption that
foreground is lighter than background is a bug.

Two extremes worth building against deliberately:

- `vantablack` — `background = #000000`
- `white` — `background = #ffffff`

These are the stress tests for the fill-alpha approach in §5: a 0.04 wash on pure black
lands at `#0a0a0a`, and on pure white at roughly `#f5f5f5`. If hover is invisible on
those two themes, the state system is wrong. Whatever `omarchy-shell` does here, we
should match rather than "improve" — consistency with the bar beats local legibility.

Hex casing is inconsistent across themes (`#060B1E` in `ethereal`, `#FFFCF0` in
`flexoki-light`, lowercase elsewhere). Parse case-insensitively, and make sure the
conformance test in §2.2 compares against `omarchy-theme-color`'s output casing rather
than assuming ours.

### 2.2c The derivation chain — themes do not all define the same keys

**Verified the hard way in M0:** `white`, `solitude` and `last-horizon` ship no `orange`
and no `brown`. A parser that requires every key fails on 3 of 22 themes, and if that
failure is swallowed the app silently falls back to a built-in palette — which is
exactly what happened before the corpus test existed.

`omarchy-theme-color` fills the gaps. Port these rules **in this order**; several feed
each other, so reordering changes results:

| Key | Rule when absent |
| --- | --- |
| `light_foreground` | `color7` ?? `foreground` |
| `bright_foreground` | `color15` ?? `foreground` |
| `cursor` | always `bright_foreground` |
| `lighter_background` | `color0` ?? `background` |
| `dark_foreground` | `color8` ?? `foreground` |
| `muted` | `color8` ?? `dark_foreground` |
| `selection` | `selection_background` ?? `color8` ?? `color0` ?? `background` |
| `selection_background` | `selection` |
| `selection_foreground` | `bright_foreground` |
| `orange` | `yellow` |
| `brown` | `mix(orange, #000000, 50%)` |
| `dark_background` | `mix(background, #000000, 25%)` |
| `darker_background` | `mix(background, #000000, 50%)` |
| `bright_{red,yellow,green,cyan,blue,magenta}` | `mix(<base>, #ffffff, 20%)` |

`mix(a, b, t)` is per-channel `int(a * (1 - t) + b * t + 0.5)` — round-half-up.

Two consequences for the parser: only `background` and `foreground` are truly required,
and **unknown keys must be kept, not rejected** — a future Omarchy release adding a
colour must not stop the app from starting.

The conformance test (`crates/omarchy-tokens/tests/corpus.rs`) checks 22 themes × 38 keys
against `omarchy-theme-color --all` and currently passes on all of them.

### 2.2d "lighter_background" is darker on light themes

On `white`, `background` is `#ffffff` and `lighter_background` is `#c0c0c0`. The name
describes what it does on a dark theme, not a direction. Treat it as *the contrasting
surface step*, whichever way that goes, and never write code that assumes
`lighter_background` is lighter.

Same theme, same caution: `dark_foreground` is also `#c0c0c0` there — identical to the
surface colour. Secondary text drawn in `dark_foreground` on a `lighter_background` panel
is invisible on that theme. M2 needs a contrast rule, not just a token lookup.

### 2.3 `shell.toml` — the structural tokens

Generated per theme into the theme dir, then layered under `~/.config/omarchy/shell.toml`.
Sections we care about: `[font]`, `[spacing]`, `[controls]`, `[popups]`, `[tooltip]`,
`[menu]`, `[launcher]`, `[notifications]`, `[bar]`, `[hyprland]`.

**Type scale** — `[font] base-size` is the rem root (theme default 12; this machine's
user override is 14). From `Style.qml`:

```
fontScale = max(1/12, base_size / 12)
fontPx(m) = max(1, round(base_size * m))

caption       = fontPx(0.833)
body-small    = fontPx(0.917)
body          = fontPx(1.0)
subtitle      = fontPx(1.083)
title         = fontPx(1.167)
heading       = fontPx(1.333)
display       = fontPx(2.0)
display-large = fontPx(2.333)
icon-small    = body-small
icon          = title
icon-large    = fontPx(1.5)
```

Any token may be pinned by an integer override under `[font]`; a pinned value is used
raw (rounded), bypassing the scale.

**Spacing scale**:

```
effectiveSpacingScale = spacing.scale * (spacing.scale-with-font ? fontScale : 1)
space(px)             = px <= 0 ? 0 : max(1, round(px * effectiveSpacingScale))
spacingToken(k, dflt) = override(k) exists ? round(override(k)) : space(dflt)
```

Note the asymmetry, and mirror it exactly: **an explicit override is NOT scaled**, the
default is. Defaults:

```
hairline 1 · xxs 2 · xs 3 · sm 4 · md 6 · lg 8 · xl 10 · xxl 12 · xxxl 14 · huge 18
control-gap 8 · control-padding-x 10 · control-padding-y 6 · input-padding-y 7
control-height 28 · popup-row-height 28 · row-gap 8 · row-padding-x 12 · label-gap 4
panel-gap 14 · panel-padding 18 · popup-padding 14
dropdown-width 240 · searchable-dropdown-width 260 · number-field-width 120
searchable-popup-min-height 220
```

**Interactive-state tokens** (`[controls]`) — the vocabulary is `normal`,
`hover-cursor`, `focus`, `selected`, `pressed`, `selection`. Each has a colour token
(a palette *role* name like `foreground` / `accent` / `urgent` / `background`, or a
literal hex), a fill alpha, a border colour, a border width and a border alpha.
Defaults: `normal` fill 0.04 / border-alpha 0.40 / border-width 1;
`hover-cursor` fill 0.08 / border-alpha 0.25 / width 1; `selected` fill 0.18 /
border-width 0; `pressed` fill 0.22; `selection` fill 0.35. `focus` defaults to the
`hover-cursor` values across the board.

This vocabulary is why omafiles will feel Omarchy-native: a row that is *hovered*, a row
the *keyboard cursor* is on, and a row that is *selected* are three distinct states with
system-defined treatments, and we get them for free.

**The five roles.** A colour token that is not a hex literal is one of five role names,
resolved by `Color.qml` as:

| Role | Source |
| --- | --- |
| `foreground` (alias `text`) | `colors.toml` `foreground` |
| `background` | `colors.toml` `background` |
| `accent` | `colors.toml` `accent` |
| `urgent` | `colors.toml` **`red`** (or `color1`) — there is no `urgent` key |
| `muted` | `colors.toml` `muted` |

`urgent ← red` is the one that is not guessable; it is why the bar's "calling attention"
colour in the generated `shell.toml` is `#f7768e` on `tokyo-night`.

**Surface resolution.** Two primitives, worth naming identically in Rust:

```
pick(key, fallback)                                   -> shell.toml value or fallback
composed(colorKey, alphaKey, colorFb, alphaFb)        -> alpha(pick(colorKey, colorFb),
                                                               pickAlpha(alphaKey, alphaFb))
```

Every per-surface colour (`popups.background`, `menu.selected-background`,
`tooltip.border`, …) is one `composed(...)` call with a documented fallback pair. Port
the fallback table verbatim from `Color.qml` rather than inventing defaults.

Two semantics to get right, both easy to get subtly wrong:

- `alpha(c, a)` **replaces** the alpha channel — it does not multiply into an existing
  one. `Qt.rgba(c.r, c.g, c.b, clamp(a, 0, 1))`.
- `flatColor(value, fallback)` resolves a token to a single opaque colour: take the
  first colour token (so a gradient collapses to its first stop), lower-case it, and if
  it names another `shell.toml` key, **recurse**. Then match the five roles plus
  `transparent`. A non-`#` string that resolves to nothing returns the fallback.

**Cross-references.** Surface tokens may reference other tokens by dotted path, e.g.
`border = "hyprland.active-border"` — handled by the recursion in `flatColor`, so this
is not a special case, just the same lookup. Values may also be Hyprland-style gradients
(`rgba(...) rgba(...) 45deg`); taking the first stop is not a v1 compromise, it is
exactly what `omarchy-shell` does wherever it needs a flat colour. Rendering the actual
gradient on borders is a later refinement.

### 2.4 Radius and gaps come from Hyprland, not from the theme

`Style.qml` sources these live:

- `cornerRadius` ← `hyprctl getoption decoration:rounding -j` → `.int` (4 on this machine)
- `gapsOut` ← `hyprctl getoption general:gaps_out -j` → `.css` first component, **halved**:
  `gapsOut = max(0, round(gaps_out / 2))` (10 → 5 here)

The halving is deliberate and documented in `Style.qml`: Hyprland's gap is tuned as a
window-to-window distance and reads as cavernous when used as a panel-to-edge inset.
Mirror it.

Hyprland config on Omarchy 4 is Lua (`~/.config/hypr/looknfeel.lua`), so **do not parse
the config** — query `hyprctl` for the effective value, and subscribe to
`.socket2.sock` for the `configreloaded>>` event to know when to re-query.

### 2.5 The reload mechanism, and the trap in it

`omarchy-theme-set` applies a theme like this:

```bash
rm -rf "$CURRENT_THEME_PATH"          # ~/.local/state/omarchy/current/theme
mv     "$NEXT_THEME_PATH" "$CURRENT_THEME_PATH"
echo "$THEME_NAME" > .../current/theme.name
omarchy-shell shell applyTheme <base64 colors.toml> <base64 shell.toml>
...
omarchy-hook theme-set "$THEME_NAME"
```

**The trap:** the theme directory's inode is replaced on every switch. An inotify watch
placed on `.../current/theme/` or on `.../current/theme/colors.toml` dies with
`IN_DELETE_SELF`/`IN_MOVE_SELF` after the *first* switch and silently never fires again.
This is exactly why `Color.qml` sets `watchChanges: false` on the theme files and takes
a push over IPC instead, while setting `watchChanges: true` only on the stable
`~/.config/omarchy/shell.toml`.

We cannot receive the Quickshell IPC push, so the crate uses **three redundant triggers**:

1. **Watch the parent** `~/.local/state/omarchy/current/` non-recursively. The `mv`
   surfaces as `Create`/`MovedTo` on the `theme` entry; `theme.name` write follows.
   Debounce ~50 ms, then re-read from scratch. Watching the parent is immune to the
   inode swap.
2. **Watch `~/.config/omarchy/` and `~/.config/fontconfig/`** (parent dirs, not files —
   editors rename-over) for `shell.toml` and `fonts.conf`.
3. **Ship an Omarchy hook.** `omarchy-hook` runs every executable in
   `~/.config/omarchy/hooks/theme-set.d/` and `font-set.d/` after a change. We install a
   tiny script there that pokes a socket / touches a stamp file. This is the *sanctioned*
   extension point and it fires even if inotify is starved.

Plus the Hyprland `configreloaded` subscription from §2.4 for radius/gaps.

All four collapse into one debounced "reload tokens" action, so redundancy costs nothing
but a wasted re-parse. Reload must be atomic: parse everything into a new `Tokens`
value, then swap — never mutate in place, or a render mid-switch sees a half-applied
theme.

### 2.6 Font

`fc-match monospace` is the source of truth — `omarchy-font-set` writes
`~/.config/fontconfig/fonts.conf` with a `prepend_first` match on the `monospace` alias,
and every Qt/GTK/shell consumer resolves through it. So: resolve the family by querying
fontconfig for `monospace` (via the `fontconfig` crate, or `fc-match monospace -f
'%{family}'` and take the first comma-separated entry, as `omarchy-font-current` does).
Size comes from `[font] base-size`, not from fontconfig.

Anchors, per `omarchy-display-text-size`: 12 px shell base ≡ GTK `text-scaling-factor`
1.0 ≡ 9 pt terminal. Range 9–20.

---

## 3. Repository layout

Cargo workspace, three crates:

```
omafiles/
├── Cargo.toml                  # [workspace]
├── crates/
│   ├── omarchy-tokens/         # pure Rust. no gpui, no UI. the §2 spec.
│   ├── omarchy-ui/             # gpui bindings + component kit
│   └── omafiles/               # the application
├── plan/PLAN.md
└── contrib/
    ├── omafiles.desktop
    └── hooks/omafiles-theme-reload   # installed into ~/.config/omarchy/hooks/theme-set.d/
```

**Why `omarchy-tokens` is separate from `omarchy-ui`:** it has no GPU, no window and no
gpui dependency, so it compiles and tests in CI in seconds and runs headless. That
matters because the conformance oracle test in §2.2 needs to run over all 22 stock
themes without a display. It is also independently useful — a Ratatui or Iced app, or a
shell script generator, can depend on it. `omarchy-ui` is the crate that requires the
heavy gpui git dependency.

---

## 4. `omarchy-tokens` — the observer

```rust
pub struct Tokens {
    pub palette: Palette,     // 56 resolved colour keys + mode
    pub typography: Typography,
    pub spacing: Spacing,
    pub controls: ControlStates,
    pub surfaces: Surfaces,   // popups, tooltip, menu, launcher, notifications
    pub geometry: Geometry,   // corner_radius, gaps_out — from Hyprland
    pub font: FontSettings,   // family, base_size
    pub theme_name: String,
}

pub enum Mode { Dark, Light }

/// Reads everything once, from the real system paths.
pub fn load() -> Result<Tokens>;

/// Same, but rooted at an arbitrary prefix — for tests and for `OMARCHY_ROOT`.
pub fn load_from(root: &Paths) -> Result<Tokens>;

/// Spawns the watcher threads; sends a fresh `Tokens` on every settled change.
pub fn watch() -> Result<(Tokens, Receiver<Tokens>)>;
```

Dependencies: `toml`, `notify` (+`notify-debouncer-full`), `serde`, `anyhow`. Hyprland
IPC is a plain UNIX socket — hand-rolled, no crate needed.

Every path is overridable (`OMARCHY_ROOT`, `XDG_*`) so the whole thing is testable
against fixture trees.

### Tests that matter

1. **Conformance** ✅ *(landed in M0 — `tests/corpus.rs`)*: for each of the 22 themes in
   `/usr/share/omarchy/themes/`, our resolver output == `omarchy-theme-color --file …
   --all`. Skipped when Omarchy is absent. Currently 22 themes × 38 keys, all agreeing.
   This test paid for itself immediately: it is what caught the three themes with no
   `orange`/`brown` (§2.2c).
2. **Scale algebra**: `base-size = 14` ⇒ `body = 14`, `heading = 19`, `caption = 12`;
   `spacing.lg` with `scale-with-font` on ⇒ `round(8 * 14/12) = 9`; a pinned
   `lg = 8` override ⇒ exactly `8`, unscaled.
3. **Inode-swap survival**: fixture tree, `rm -rf` + `mv` the theme dir five times in a
   loop, assert five distinct `Tokens` arrive. This is the regression test for §2.5.
4. **Torn read**: reload while the theme dir is mid-write ⇒ either old or new tokens,
   never a mix.

---

## 5. `omarchy-ui` — the gpui design system

> This section states **intent**. The exact gpui API — which is not stable and which
> most online material gets wrong — is deliberately not hardcoded here. It lives in
> [`plan/GPUI-NOTES.md`](./GPUI-NOTES.md), which is the M0 validation checklist and
> becomes the pinned reference once answered.

### Theme plumbing

`Theme` is a gpui **global**, holding the current `Tokens` converted into gpui types
(`Hsla`, `Pixels`, `Rems`, `Font`). One background task owns the
`omarchy_tokens::watch()` receiver; on each message it writes the global via
`cx.update_global::<Theme, _>(…)`.

**Important correction — gpui globals do not auto-invalidate views.** There is no
implicit dependency tracking: mutating a global pushes `NotifyGlobalObservers`, but a
view that merely *reads* `cx.global::<Theme>()` will not re-render. Each view must
subscribe, and must **hold the `Subscription`** — dropping it silently unsubscribes:

```rust
struct FileList { _theme: Subscription, /* … */ }

impl FileList {
    fn new(cx: &mut Context<Self>) -> Self {
        let sub = cx.observe_global::<Theme>(|_this, cx| cx.notify());
        Self { _theme: sub, /* … */ }
    }
}
```

A dropped subscription produces a view that is stale only after a theme change — which
is invisible in normal development and obvious to a user. So `omarchy-ui` should expose
a single `theme_subscription(cx)` helper (or a `#[derive]`/macro) and make it the
documented way to build a themed view, rather than leaving each site to remember.
The gallery example must include a view that deliberately forgets, so the failure mode
is visible once and never shipped.

An `ActiveTheme` extension trait on `App`/`Window` gives the ergonomic call site:

```rust
div()
    .bg(cx.theme().surface)
    .rounded(cx.theme().radius())          // Hyprland's decoration:rounding
    .p(cx.theme().space.panel_padding)
    .text_size(cx.theme().font.body)
    .font_family(cx.theme().font.family.clone())
```

**Fonts: resolved, and it's good news.** gpui's Linux text system is cosmic-text +
swash, loading system fonts through fontdb → fontconfig. Nothing is bundled, families
resolve by name (`div().font_family("JetBrainsMono Nerd Font")`), and because the family
is ordinary element state, changing it and calling `cx.notify()` re-shapes on the next
frame — **no window recreation**. `cx.text_system().all_font_names()` enumerates what is
installed. So `omarchy-font-set` changing the monospace family propagates through the
same subscription path as a colour change, with no special handling.

### Component kit

Ported from the `Ui/` widgets in `omarchy-shell` so the visual grammar matches:
`Panel`, `PopupCard`, `Button`, `WidgetButton`, `TextField`, `Dropdown`,
`SearchableDropdown`, `Toggle`, `PanelSeparator`, `PanelSectionHeader`, `ToolTip`,
`ConfirmDialog`.

Plus what a file explorer needs and the shell has no equivalent for: `VirtualList`
(virtualised rows), `SplitPane` (draggable, persisted ratio), `KeyHintBar`,
`StatusBadge`, `Breadcrumb`.

The single most important primitive is `InteractiveSurface` — the thing that renders the
five-state vocabulary from §2.3 (normal / hover-cursor / focus / selected / pressed)
using the `[controls]` tokens. Every row, tab, button and tile composes it. Getting this
one right is most of what "looks like Omarchy" means.

### The look

The brief is *modern / minimalist / retro*. Concretely, and consistent with what the
tokens push you toward anyway:

- **Monospace everywhere.** The system font *is* the monospace family; leaning into it
  rather than fighting it gives the retro-terminal register for free, and it makes
  columns align without measurement.
- **Structure by hairline and inset, not by shadow.** `normal-border-alpha` is 0.40 at
  1 px; there is no elevation token in Omarchy. No drop shadows.
- **Fill-alpha over solid fills.** States are washes on the background (0.04 → 0.22),
  which is what keeps 22 wildly different themes all legible.
- **A list item carries its state in its fill and its text, never in an outline.** Idle
  rows sit at the *secondary* foreground and step up to the primary one when hovered, when
  the cursor lands on them, or when they are selected; the wash stays. A list is mostly
  idle rows, so that step is enough to make one row the focal point, and it does not draw
  a box in the middle of a column of names the way a border does. Controls — buttons,
  inputs, cards — keep their border, because there the outline is what says "control".
- **`accent` is scarce.** One accent element per view: the focused pane's cursor row.
  Everything else is foreground at varying alpha. In the listing that is the cursor row's
  icon — *not* every directory icon, which was half the rows.
- **Density from `control-height` (28) and `row-padding-x` (12).** Do not hardcode row
  heights.
- **No custom colour, ever.** If a value is not derivable from the palette, that is a
  design bug. This is the rule that keeps every theme working without a per-theme
  stylesheet.
- **Translucency via Hyprland, not gpui.** gpui's `WindowBackgroundAppearance::Blurred`
  is gated entirely on the KDE-only `org_kde_kwin_blur_manager` protocol, which
  Hyprland 0.56.2 does not implement — so `Blurred` is a **silent no-op** here. But gpui
  calls `set_opaque_region(None)` for both `Transparent` and `Blurred`, so the surface
  genuinely is transparent and **Hyprland's own `decoration:blur` blurs behind it**. Use
  `Transparent` plus a Hyprland windowrule, and the window picks up whatever blur the
  user has configured for everything else — which is more Omarchy-native than a
  gpui-drawn blur would have been anyway.

A `cargo run -p omarchy-ui --example gallery` binary renders every component in every
state, and is the fastest way to check a theme switch end-to-end.

---

## 6. `omafiles` — the application

### 6.1 Layout

```
┌────────────────────────────────────────────────────────────────────┐
│  ~/Documents/Github/omafiles                    [http ●:8080] [⌘K] │  breadcrumb + status
├──────────────┬─────────────────────────────┬───────────────────────┤
│ PLACES       │  NAME              SIZE  AGE│                       │
│  Home        │  ▸ crates/                  │      P R E V I E W    │
│  Downloads   │  ▸ plan/                    │                       │
│  Documents   │  ▸ .git/                    │   image / video /     │
│  Pictures    │    Cargo.toml       412  2m │   markdown / code     │
│  .config     │    README.md         37  1h │   with tree-sitter    │
│  ─────────── │                             │                       │
│ WORKSPACES   │                             │                       │
│ ▾ Client X   │                             │                       │
│    invoices  │                             │                       │
│    assets    │                             │                       │
│ ▸ Personal   │                             │                       │
│  + New…      │                             │                       │
│  ─────────── │                             │                       │
│ TABS         │                             │                       │
│  omafiles    │                             │                       │
│  Downloads   │                             │                       │
├──────────────┴─────────────────────────────┴───────────────────────┤
│ ⏎ open  ␣ preview  / search  t term  a agent  s share  ? keys      │  key hints
└────────────────────────────────────────────────────────────────────┘
```

Three panes. The sidebar has three sections: **places** (navigation shortcuts),
**workspaces** (named groups of tabs), and the **global tabs** that belong to no
workspace.

The listing column is a table, so it gets a **column header** — `NAME`, `SIZE`, `AGE` in
the same uppercase secondary caption the sidebar's group labels use, over a hairline. It
sits outside the scrolling area: it stays put while the listing moves under it, and the
scrollbar does not run across it. `AGE` rather than `MODIFIED` because the column holds
`2m` / `3h` / `1y`, not a date. The labels and the row cells read their widths from the
same constants — two hand-written numbers would drift apart by a pixel and nobody would
catch it in a diff.

**Places** are standard XDG directories + `.config`, plus a user-editable pinned set
persisted to `~/.config/omafiles/places.toml`. Reorderable, removable, addable from the
current directory with a keystroke.

Resolve places from `~/.config/user-dirs.dirs` (or `xdg-user-dir <NAME>`), never from
hardcoded English folder names — they are localised, and this machine already shows two
traps: `XDG_DESKTOP_DIR` and `XDG_TEMPLATES_DIR` both point at `$HOME` (so a naive
sidebar shows "Desktop" as a duplicate Home entry), and there is a non-standard
`XDG_PROJECTS_DIR`. Rule: skip any place that resolves to `$HOME` or does not exist, and
surface non-standard `XDG_*_DIR` entries too.

### 6.2 Tabs and workspaces

**A place is a shortcut; a tab is an open view.** Clicking a place navigates the current
tab. A tab carries its own state — path, back/forward history, cursor position, scroll
offset, selection — which is what makes reopening one feel like returning rather than
starting over.

**A workspace is a named, ordered group of tabs.** Tabs that belong to no workspace are
*global*, which is really just the implicit default workspace rendered without a header.

Exactly one workspace is **active** at a time (global counts as one). The active
workspace is the scope for new tabs: opening a directory in a new tab while `Client X`
is active puts the tab in `Client X`, not in the global list. This is the whole point —
it means the grouping needs no explicit filing step.

| Action | Behaviour |
| --- | --- |
| Create workspace | Prompts for a name; becomes active |
| Rename / delete | Delete moves its tabs to global rather than destroying them — a workspace is a grouping, not a container that owns lifetimes |
| New tab | Lands in the active workspace |
| Drag a tab | Between workspaces, into global, or reordered within one |
| Collapse a workspace | Purely visual; does not close its tabs |
| Activate a workspace | Focuses its most-recently-used tab |

Drag and drop is *internal* to the window, which bare gpui already supports
(`GPUI-NOTES.md` §4). It does not need the external drag path or `gpui-component`.

**Keyboard first, as everywhere else.** Drag and drop is the discoverable route, not the
only one: `Ctrl-1..9` activates a workspace, `Ctrl-Shift-1..9` moves the current tab into
one, and the command palette exposes every workspace operation by name. A
shortcut-oriented app where grouping is mouse-only would be a contradiction.

#### Persistence and sync

Session state lives in **`~/.local/state/omafiles/session.toml`** — state rather than
config, because it is machine-written on nearly every navigation. `places.toml` stays in
`~/.config` because it is user-curated. Written atomically (temp file + rename), the same
discipline `omarchy-theme-set` uses.

```toml
version = 1
revision = 47            # monotonic; see the conflict rule below
active = "client-x"

[[workspace]]
id = "client-x"          # stable; the name is a label and may change
name = "Client X"
collapsed = false
[[workspace.tab]]
path = "/home/alois/Clients/X/invoices"
cursor = "2026-03.pdf"   # by name, not index — the directory changes underneath us
```

**Sync across instances — recommendation: a single process with multiple windows.**
A second `omafiles` invocation hands its arguments to the running one and exits. Then
"synced across instances" is true by construction, and an entire class of concurrent-write
bugs simply does not exist. This is what Nautilus and most editors do, and multiple
*windows* — which is what a Hyprland user actually wants across monitors and workspaces —
still works.

The state file plus a watcher stays underneath that as the durable layer, so a second
process that does appear (a race at startup, a crash recovery, a deliberate
`--new-instance`) converges rather than corrupting. We already have exactly this watcher
machinery from M1.

**⚠ The trap: do not reload your own writes.** The watcher will see our own atomic rename
and reload, clobbering in-flight UI state — a feedback loop that is easy to miss because
it only bites when a write and an edit overlap. Every write bumps `revision`, and the
writer remembers what it wrote; a reload whose `revision` is one we produced is ignored.

If concurrent processes ever become a real requirement rather than a fallback, last-writer-wins
is *not* good enough — two processes each opening a tab would lose one of them. The correct
answer is an append-only operation log (add-tab, move-tab, rename-workspace) that each
process replays, which converges properly. That is a significant amount of work and should
not be built on speculation; it is called out in §9.

### 6.3 Core model

- Directory listing via `ignore::WalkBuilder` at depth 1 (gives `.gitignore` awareness
  for free, which is worth having as a toggle) with a plain `std::fs` fallback.
- Listing runs on a background executor; the UI shows the previous listing until the new
  one lands, so navigation never blanks.
- `notify` watch on the current directory for live updates.
- Virtualised list — must stay smooth in a directory with 100k entries.
- Navigation history (back/forward) and a `..` that remembers which child you came from
  and restores the cursor onto it.

### 6.4 Search — one input, two modes

The brief says "using fzf". **Recommendation: use `nucleo` (the matcher Helix uses,
same algorithm family as fzf, MIT) in-process rather than spawning the `fzf` binary.**
Rationale: fzf is a full-screen TUI; driving it from a GUI means a pty, ANSI parsing and
a process per keystroke-session, and you cannot render the results with your own design
system. `nucleo` is multithreaded, incremental, and gives you a score per item so *we*
draw the rows. The fzf-quality matching is retained; only the TUI is dropped.

Two modes behind one `/` field:

| Mode | Trigger | Backend |
| --- | --- | --- |
| Filter current directory | `/` | `nucleo` over the loaded listing, instant |
| Recursive filename search | `/` then no local hits, or `Ctrl-P` | `fd`-equivalent walk via `ignore`, streamed into `nucleo` |
| Content search | `Ctrl-Shift-F` | ~~`grep-searcher` crates, or~~ **shell out to `rg --json`** *(landed post-M11 — M8's git reasoning: `rg` ships with Omarchy, `--json` is a documented interface, and it already knows gitignore, binary detection and encodings)*. Literal (`--fixed-strings`) with smart case — a query dying on an unbalanced `(` looks broken, not regexy. Debounced 250 ms, generation-guarded, capped per file, per line, and at 200 hits total with the cut admitted on screen |

Streaming matters: results appear as the walk proceeds, cancelled on the next keystroke.

### 6.5 Preview

| Kind | Approach |
| --- | --- |
| Images | gpui `img()` takes a path directly and handles avif/jpg/png/gif/webp/tiff/bmp/ico/hdr/exr/qoi — **animated GIF and WebP play automatically**, so no first-frame compromise. Cache by (path, mtime, size). **⚠ Reproduce [gpui-component #2527](https://github.com/longbridge/gpui-component/issues/2527) in M0 first** — an open, uncommented report that local images fail to render on Linux while still occupying layout. This gates the whole pane. |
| SVG | `img()` renders SVG through resvg in full colour. Do **not** use gpui's `svg()` element here — it is an alpha mask tinted by the text colour, i.e. monochrome, and paints nothing if no text colour is set. |
| Video | **Thumbnail + metadata in v1** (`ffprobe` for metadata, `ffmpeg` for a poster frame). Inline playback is a v2 item — see §10. `mpv` is installed and can be launched externally in the meantime. |
| Markdown | `pulldown-cmark` → our own gpui element tree, styled with the design system. Not an HTML webview. |
| Code | `tree-sitter` + `tree-sitter-highlight`, highlight names mapped onto the Omarchy palette. |
| PDF | Out of scope v1. |
| Text/other | Plain, with a byte-count and encoding note. |
| Binary | Hex head + `file`-style type detection. |

Everything is size-capped (say 10 MB) with an explicit "too large to preview" state, and
everything decodes off the main thread. A preview must never block a keystroke.

#### Expanded preview

A file worth previewing is often worth previewing *large* — an image, a diff, a page of
markdown. Every preview that can overflow the detail panel gets an expand affordance: a
button in the panel's corner, and **`Space`** on the listing, which is the Quick Look
gesture people already have in their fingers.

**It takes the listing column and the detail panel, not the window.** *(Revised twice in
M7. The original design said a full-window overlay on the modal layer; that was built,
tried, and rejected because covering the whole window also hides the sidebar, which is how
you change directory. The first correction left the detail panel docked — and that was
wrong too, in a way only visible on screen: the panel was rendering the same preview
beside the expanded one. Same title, same facts, same picture, twice.)*

So the expanded preview replaces **the centre pane and the detail panel**, and leaves the
sidebar alone. One copy of the preview, and you can still change directory without
collapsing it — which is what keeps this a pane rather than a modal. `Escape` or `Space`
collapses, and a compress button sits in the expanded view's corner, because the expand
button lived in the detail panel that the expanded view replaces. Making the *window*
fullscreen remains the compositor's job — the user already has `SUPER+F`.

**It shows the body alone — no title, no fact table.** The kind, size and modified date are
a *description* of a file and belong to the panel; expanded, you are looking at the file
itself. Asking to see a picture larger is not asking to see the words next to it larger,
and a fact table given the width of a maximised window puts its labels and values on
opposite edges of the screen. The name is the exception, and it moves to the footer beside
the key hints: with the title gone *and* the listing replaced, nothing on screen would
otherwise say which file `j`/`k` had landed on.

**The state is per tab, and persisted.** One tab can be a folder of images being flicked
through expanded while another is an ordinary listing; switching between them shows each
as it was left. It lives on `Tab` in `session.toml` beside `cursor`, so it survives a
restart and syncs across instances like everything else there. Written only when true, so
an older session file still loads.

**One renderer, two sizes.** The preview *body* must take its available space as a
parameter rather than existing in a panel version and an expanded version — two renderers
would drift, and the second would be the one nobody notices is wrong. Concretely the body
is a function of `(file, target)`, and both call sites go through it. `Target` is an enum
rather than a pixel width because the difference is one of kind: the panel shows a
thumbnail, a dozen hex rows, the head of a file; expanded shows the file. What the two
sites do *not* share is the surround — that is where the panel adds its title and facts
and the expanded view adds nothing.

**It must stay navigable while open.** `j`/`k` move to the next and previous entry
*without collapsing*, re-rendering the preview in place. Flicking through a folder of
images is most of the value. This falls out of the design rather than needing a special
case: the expanded pane is not an overlay and never takes focus, so the listing's key
context still owns `j`/`k`.

Applies to images, SVG, video poster frames, markdown, code and text, and to M8's diff
view. Not to the "too large to preview" and binary-hexdump states, which say all they have
to say in the panel — `Body::is_expandable` is where that list lives.

Three things that bite here specifically:

- **The size cap does not go away.** A 10 MB text file is no more previewable expanded
  than in the panel; the cap and its explicit "too large" state apply identically.
- **Expanding must not require the detail panel.** It reads the cursor, not the panel, so
  it works with the panel collapsed — and it replaces the panel when the panel is open.
- **Row widths are not scale-free.** The panel fits roughly 30 caption characters, so a
  16-byte hex row wraps there and a wrapped hex row loses the alignment that is the only
  reason to show hex at all. The panel uses four bytes a row and the expanded column
  sixteen. Anything laid out in columns needs this treatment, not just hex.

**Highlight mapping.** Capture names (`keyword`, `string`, `function`, `type`,
`comment`, `constant`, …) map onto palette roles once, in `omarchy-ui`, so every theme
colours code correctly with no per-theme work. This mapping table is the one place where
taste is encoded; get it right and 22 themes come out looking deliberate. Start with a
small grammar set — Rust, TOML, JSON, Markdown, Bash, JS/TS, Python, Lua, QML — and
grow it.

**tree-sitter vs syntect — decide in M0, not M7.** You asked for tree-sitter, and if we
adopt `gpui-component` it comes nearly free: it has a `tree-sitter-languages` feature
with 35 grammars and a `highlighter` module already wired to its theme system. The
catch is a version pin conflict — `gpui-component 0.5.1` pins `tree-sitter ^0.25.4`
while current is 0.27.0, zed `[patch]`es `tree-sitter-language`, and grammar crates are
ABI-sensitive, so mismatches surface as link errors rather than compile errors.
`syntect 5.3.0` (+ `two-face` for bat's syntax/theme set) sidesteps that entirely at the
cost of regex-based rather than parsed highlighting.

Recommendation: **take tree-sitter via `gpui-component`'s feature flag** if M0 adopts
that crate, and fall back to `syntect` if we go bare-gpui. Do not hand-assemble
tree-sitter grammar crates alongside gpui-component — that is the configuration that
fights itself.

**File icons.** `freedesktop-icons 0.4.0` resolves a Freedesktop icon theme (Omarchy
themes ship an `icons.theme` naming one — `Yaru-magenta` on `tokyo-night`), then render
with **`img(path)`, not `svg(path)`**. gpui's `svg()` element renders an alpha mask
tinted by the text colour — monochrome only, and it paints *nothing* if no text colour
is set. `img()` goes through resvg and gives full colour. Caveat: `img()` rasterises SVG
at scale factor 1.0, so expect softness when upscaled on HiDPI.

### 6.6 Actions

All four "do something with this file" actions reuse Omarchy's own scripts rather than
reimplementing them. That is deliberate: it means omafiles inherits the user's
configured defaults (terminal, agent) and stays correct when Omarchy changes.

| Action | Key | Implementation |
| --- | --- | --- |
| Terminal here | `t` | `setsid uwsm-app -- xdg-terminal-exec --dir=<cwd>` — the body of `omarchy-launch-terminal`, minus its `omarchy-cmd-terminal-cwd` call since we already know the cwd. |
| Agent chat from file | `a` | `omarchy-agent-prompt "<prompt>"` with cwd set to the file's directory. `omarchy-agent` respects the user's `omarchy default agent` (pi / claude / codex / opencode / …) and launches it in a terminal. Prompt is composed by us and editable in a small dialog before launch. Handle "no default agent configured" by surfacing Omarchy's own picker: `omarchy-menu summon setup.default.agent`. |
| Share via LocalSend | `s` | `omarchy-menu-share file <path…>`, which runs `systemd-run --user --quiet --collect localsend --headless send <files>`. Multi-select supported. Note: `localsend --help` **hangs** (it is a GUI binary) — never probe it, just invoke through the Omarchy script. |
| Open with default app | `⏎` on a file | `cx.open_with_system(path)` — gpui shells `xdg-open` for us. |

gpui already provides more platform integration than expected, so several things we'd
otherwise pull crates for are built in: `open_with_system`, `reveal_path`, `open_url`,
`add_recent_document`, clipboard read/write (including async), and
`prompt_for_paths`/`prompt_for_new_path` routed through xdg-desktop-portal via `ashpd`.
That means `rfd`, `arboard` and `open` are all unnecessary. `xdg-desktop-portal-hyprland`
is already installed here.

**Not in scope, but worth knowing:** Omarchy's own file picking (`omarchy-file-select`)
goes through the XDG desktop portal `org.freedesktop.portal.FileChooser`, backed by
`xdg-desktop-portal-hyprland` (`~/.config/hypr/xdph.conf`). Becoming *the system file
chooser* would mean implementing a portal backend — a separate D-Bus service, not a
feature of this app. Filing it here so it is a deliberate non-goal rather than a
surprise.

### 6.7 HTTP server

In-process `axum` + `tower_http::services::ServeDir`, **not** a spawned `python -m
http.server` or `miniserve`. Reasons: reliable stop (drop the task, no orphaned pid),
real status (bound port, request count, last request) from the same process, port
auto-selection on conflict, and no extra runtime dependency.

- ~~Toggle in the header.~~ **A button in the status bar** *(revised in M10, by
  request — and it is the same argument that moved the git branch there in M8: server
  state is a fact about the app, not part of the address)*. Stopped it reads `http off`,
  dimly; serving, it carries the accent and the port — plus the served path whenever
  that is not the directory on screen, because `:8080` next to the wrong directory
  would imply it serves that one. Clicking (or `^s`) opens a contextual menu: stopped,
  the two ways to start; running, the URL, a live request log, open-in-browser,
  copy-URL, and stop.
- Binds `127.0.0.1` by default. Serving on `0.0.0.0` is a second, explicit choice with
  a visible warning — this is a real exposure and should never happen by accident.
- Directory listing enabled; shows the URL and offers a QR code (Omarchy has
  `omarchy-capture-qr` precedent) for phone access when bound to the LAN.
- Serves the directory that was current *when started*, pinned. Navigating away does not
  silently move the server root — the badge stays showing the served path.
- ~~Stops on app exit. No lingering.~~ **Outlives the window** *(reversed post-M11, by
  request)*: each server is a detached process — the app re-exec'd as
  `omafiles --serve <dir> [--lan]` under `setsid` — registered in
  `~/.local/state/omafiles/servers/` (one TOML + one capped log file per process,
  written by the process itself, temp+rename). Closing omafiles leaves the ports
  serving; a relaunched omafiles — or a second window — lists the same registry and
  finds them again, hit counts included. Stopping is the globe list's kill button
  (SIGTERM, so in-flight requests finish); a process that dies ungracefully cannot
  sweep its own files, so the next listing does.

### 6.8 Keyboard model

Shortcut-oriented is the point, so this is a design surface, not an afterthought.

- gpui actions + `KeyBinding`s with **key contexts** per pane, so `/` means "filter" in
  the list and "literal slash" in a text field, with no manual guarding.
- Vim-ish defaults (`j`/`k`/`h`/`l`, `g`/`G`, `Ctrl-D`/`Ctrl-U`) *and* arrows.
- `Space` previews, `Enter` opens, `Backspace` goes up.
- `Ctrl-K` command palette listing every action with its binding — also the
  discoverability mechanism and the place new features get exposed for free.
- `?` shows the full keymap sheet.
- Persistent key-hint bar at the bottom, contextual to the focused pane.
- Keymap loaded from `~/.config/omafiles/keymap.toml`, merged over defaults, so it is
  rebindable without a rebuild.
- Destructive operations (delete) go through the `trash` crate to XDG trash, never
  `unlink`, and still confirm.

### 6.9 Git

A file manager sitting in a repository should say so. Three things: the current branch
with a way to switch it, a status marker on each entry's icon, and the diff of a changed
file in the preview.

#### Measured first, because it decides the architecture

On this machine:

| Repository | Tracked files | `git status --porcelain=v2` | `rev-parse --abbrev-ref HEAD` |
| --- | --- | --- | --- |
| omafiles | 31 | **4 ms** | 3 ms |
| zed checkout | 4,288 | **400 ms** | 2 ms |

So: **the branch is cheap enough to read on navigation; status is not.** 400 ms would drop
four frames if computed inline, and that is a mid-sized repo — a monorepo is worse. Status
goes on a background task, cached per repository, with the result folded in when it lands.
The listing renders immediately without it and gains the markers a moment later, the same
"never blank, fill in when ready" discipline as M3's directory reads.

Note what is *not* the problem: this repo carries 18 GB of gitignored `target/` and still
returns in 4 ms. Cost tracks tracked-file count, not directory size.

#### Library: `git` itself, for reads as well as the switch

*(Revised in M8. The original text chose `gix` 0.87 for reads and the binary only for
switching; the reasoning is preserved below, because the argument it made — against
libgit2 — still holds. What it never compared `gix` against was the git binary.)*

- **Reads** — branch, per-path status, diff — shell out to `git`.
- **Switching branches shells out to `git switch`.** Deliberately.

Four things decided the first half:

- **The binary is already a hard requirement.** The switch shells out because clobbering
  uncommitted work is unrecoverable, so a machine without `git` has no working M8 either
  way. `gix` would have been a second implementation of something we still cannot do
  without.
- **`--porcelain=v2 -z` is plumbing** — documented, stable, and reporting the staged and
  worktree halves of every path in one pass. The same picture out of `gix` means composing
  a tree-index diff with an index-worktree walk and reconciling them by hand.
- **The diff view wants unified diff *text*,** which is what `git diff` produces and what
  M7's `diff` grammar highlights. `gix` has no unified formatter, so we would have written
  one — and it would ignore the user's own diff configuration, which `git diff` honours.
- **The measurements below are of this implementation.** They were timed with
  `git status --porcelain=v2`, so the background-and-cache architecture is validated
  against exactly what runs.

The cost is a fork per status read, and it is bounded: repo discovery and the branch label
are filesystem reads with no process at all, status is one fork per repo per change, and
the diff is one fork for the previewed file. Nothing forks on the main thread.

One flag is load-bearing: **`--no-optional-locks` on every read.** `git status` normally
rewrites `.git/index` to refresh its stat cache, and we watch `.git` — so that write comes
back as an event, which re-runs status, which writes again. It is M5's session-file
watcher loop in a new costume, and this is git's own answer to it.

The switch is the important choice. Changing branches with uncommitted work is the
most destructive thing this app will ever do. `git switch` already knows when a checkout
would clobber local modifications and refuses; it handles submodules, sparse-checkout and
hooks. Reimplementing those semantics — or trusting a library's partial version of them —
risks destroying work that was never committed, which is unrecoverable. Shelling out
inherits git's exact safety behaviour *and* its error text, which we surface verbatim
rather than paraphrasing.

The switcher therefore never forces. If git refuses, we show why and leave the repo alone.

#### Status markers

A small badge composited onto the entry's existing icon, coloured from the Omarchy
palette so it retints with everything else — no hardcoded green and red:

| State | Palette role |
| --- | --- |
| added / new | `green` |
| modified | `yellow` |
| deleted | `urgent` (which Omarchy sources from `red`) |
| untracked | `muted` |
| conflicted | `urgent`, and it must be visually distinct from deleted |
| ignored | no marker; dimmed name at most |

**Directories roll up**: a folder shows a marker when anything inside it has changed. This
is the expensive half — a rollup needs status for the whole subtree, not one directory —
so it is computed once per repo from the single status call and indexed by path prefix,
never by walking per row.

#### Branch placement — settled: the status bar

**The current directory's git information lives in the status bar**, beside the directory
summary: `⎇ develop  +1 •2 −1 ?1`, clickable, and absent entirely outside a repository.

That is a change from the original plan, which put the branch in the header. The header is
the *path*; the branch is a fact about where you are rather than part of the address, and
putting it there made it compete with the breadcrumb for width on a narrow window. The
status bar already carries "what is in this directory" — "which branch this directory is
on" is the same kind of statement.

The counts beside it use the same glyphs and the same palette roles as the row markers, so
`•` means modified in both places. Nothing is shown while the background status is still
running, which is what keeps the branch itself on screen immediately.

The original text also asked for the branch *under* the name on directory items. That is
not done, and on a **listing row** it should not be: M3 uses gpui's `uniform_list`, which
requires every row to be the same height, so a second line means either making every row
two lines tall — halving the density of a file list, for information relevant on a handful
of rows — or losing the virtualisation that keeps a 100k-entry directory smooth. An inline
`omafiles ⎇ develop` would fit, but it costs a repository open per row, and with the branch
now in the status bar it would mostly repeat what is already on screen.

#### Things that will bite

- **Most directories are not in a repo.** Detection walks up looking for `.git`, so a
  negative result must be cached per directory, or every navigation stat-walks to `/`.
- **`.git/HEAD` changes outside the app.** Committing or switching in a terminal must be
  noticed. M3's watcher covers the current directory; this needs `.git` watched too.
- **A huge diff is not previewable.** Cap it, and say so, rather than rendering 50,000
  lines.
- **A repo mid-rebase or with a detached HEAD has no branch name.** Show the state
  (`rebasing`, `detached at abc1234`) rather than an empty label.

---

## 7. Milestones

Each milestone ends with something runnable.

**M0 — Ground. ✅ Done (2026-09-01).** Rust 1.97.1, workspace skeleton, and a gpui
window rendering on Hyprland. **gpui is a go** — full results, findings and build cost
in [`GPUI-NOTES.md`](./GPUI-NOTES.md) §8. Headlines: the image-rendering bug that
threatened the preview pane is a gpui-component issue and does not reproduce on bare
gpui; `app_id` and `WindowDecorations::Server` must be set explicitly; the theme-reload
path works end to end across dark, retro and light themes.

The original M0 text follows, for the record.

⚠️ **The one thing that will waste a day if missed:** on Linux you must enable a
windowing feature on `gpui_platform` (`features = ["wayland"]`, optionally `"x11"` for
XWayland). Without it, `gpui_linux::current_platform` hits an `unreachable!()` and the
app crashes at startup with no useful message.

M0's checklist, and the exact `Cargo.toml` pins, are in
[`GPUI-NOTES.md`](./GPUI-NOTES.md). The four things to test before committing:
fcitx5/ibus composition (untested upstream), RADV artifacts on Mesa 26.2.1
([#63358](https://github.com/zed-industries/zed/issues/63358)), floating-window resize
under Hyprland ([#63285](https://github.com/zed-industries/zed/pull/63285)), and local
image rendering ([gpui-component #2527](https://github.com/longbridge/gpui-component/issues/2527)).

**M1 — `omarchy-tokens`. ✅ Done (2026-09-01).** The `shell.toml` structural tokens and
the watcher.

- **Structural tokens**: `Typography`, `Spacing`, `ControlStates` and `Surfaces`, ported
  from `Style.qml`/`Color.qml` with their defaults *and their resolution order*. The
  theme's generated `shell.toml` layered under the user's, user keys winning. 102 keys
  merge on this machine.
- **Watcher**: four triggers collapsing into one debounced reload — the state parent
  directory (never `theme/` itself), `~/.config/omarchy`, `~/.config/fontconfig`, and
  Hyprland's `.socket2.sock` for `configreloaded`. A theme switch reaches a running
  window in a few hundred milliseconds with no keypress.
- **The inode-swap regression test** — and it has teeth: pointed at `theme/` instead of
  its parent, it fails on swap 1, exactly reproducing the "works once, then silently
  never again" bug it exists to prevent.
- `omarchy-tokens-dump --watch` streams changes, so the whole thing is provable with no
  UI at all.
- `contrib/hooks/omafiles-theme-reload` — optional belt-and-braces for exhausted inotify
  watches. It touches a stamp file in a directory we already watch, so it needs no IPC.

49 tests. The one that matters most is `survives_repeated_inode_swaps`.

**M2 — `omarchy-ui` foundation. ✅ Done (2026-09-01).**

- **`ActiveTheme`** — `cx.theme()` on `App` and `Context<T>`. Deliberately *not* on
  `Window`: it holds no globals, and a trait impl that panics is worse than no impl.
- **`InteractiveSurface`** — the five-state primitive every row, button and tile
  composes. Precedence (pressed > selected > focus > hover) is resolved in one place so
  call sites cannot each invent their own ordering. `Chrome::Quiet` is the default,
  because 200 visible borders in a directory listing is noise — and `.borderless()`
  takes the last of them off the rows entirely, leaving the fill and the secondary →
  primary text step to carry the state.
- **The contrast rule** (§2.2d made real). `ensure_contrast` is a *floor*, not a
  policy: it leaves a colour alone when it is already legible. The threshold was chosen
  from measurements, not taste — see below.
- **Components**: `Panel`, `Row`, `Button`, `Separator`, `SectionHeader`, `Badge`,
  `KeyHint`, `Breadcrumb`. Deliberately small; the rest arrive in M3–M7 with a real call
  site to shape them.
- **`examples/gallery`** — every component in every state, driven by the live theme.
- **`Modal`** *(added in M6)* — a generic scrim + card overlay reading `[menu]`'s tokens,
  so a dialog here looks like the Omarchy menu. Both the search palette and the workspace
  prompt are this. Two layout lessons are baked in: the overlay must be a **sibling of**
  the content column inside a plain positioned container, because an absolutely-positioned
  element that is also a flex *item* still gets sized by flex; and the scrim must be
  `darker_background`, not `background`, or it is invisible over a window already painted
  in that colour.

**On the success criterion.** "Run the gallery through 22 themes × 12 text sizes" is 264
screenshots, which nobody will repeat on every change. So it was split: the mechanical
half is `tests/legibility.rs` (contrast across all 22 themes, scale monotonicity and
clipping headroom across sizes 9–20, no display needed), and the gallery is left for what
genuinely needs eyes — layout, rhythm, clipping. Verified by eye on `tokyo-night` and
`white`.

**The threshold decision, from data.** `ensure_contrast`'s floor changes how many themes
get overruled:

| Floor | Pairs adjusted (of 44) | Themes touched (of 22) |
| --- | --- | --- |
| 2.0 | 6 | 4 |
| 2.5 | 20 | 13 |
| 3.0 (WCAG large text) | 28 | 16 |

Omarchy's `dark_foreground` sits at 1.0–2.9:1 against its own surfaces; dim text is dim
by design. **2.0 was chosen**: it rescues only what is genuinely unreadable — `white`'s
secondary text is byte-identical to its panel at 1.00:1 — and leaves the other 18 themes
exactly as their authors wrote them. This is the "match the shell" principle winning over
accessibility, which is a real trade-off and a one-line change to revisit.

**M3 — Explorer skeleton. ✅ Done (2026-09-01).** Two panes, a real listing, virtualised
scrolling, the keyboard model, live refresh.

**The composition question is settled, and the answer was "mostly not gpui-component".**
gpui's own `uniform_list` virtualises (our rows are a fixed `control_height`, exactly what
it wants), `omarchy_ui::Row` renders each row so the interaction states stay ours, and
**gpui-component supplies only the scrollbar** — the one thing bare gpui does not have at
all. Its `ScrollbarHandle` is implemented for `UniformListScrollHandle`, so the two
compose directly. We took none of its table or list: `Row` already existed, and their
table cannot do modifier-aware multi-select without a fork.

- `crates/omafiles/src/{entry,listing,navigation}.rs` are pure model, no gpui — 21 tests
  that run headless.
- Sorting is directories-first then **natural** order, so `frame2` precedes `frame10`.
  Case-insensitive with a case-sensitive tiebreak, because a non-total ordering lets
  `README` and `readme` swap places between refreshes.
- Hidden entries are *read* and filtered at the view, so `Ctrl-H` needs no disk re-read.
- The cursor indexes the full listing, not the visible subset, so toggling hidden files
  cannot slide it onto a different entry.
- Reads happen on a background task and the previous listing stays on screen until the new
  one lands — navigation never blanks. Holding the `Task` means a newer navigation cancels
  a slow one, so a stalled network mount cannot overwrite where you have since moved to.
- `notify` watches the current directory, debounced by draining the burst; copying a
  hundred files in produces one reload, not a hundred.
- Refresh and navigation both restore the cursor **by name**, since indices shift when a
  file appears above it.

The behaviour worth calling out: **going up puts the cursor on the directory you came out
of.** Verified end to end by driving the real window — enter `plan/`, toggle hidden,
press Backspace, and the cursor is on `plan`.

Deferred honestly: opening a *file* does nothing yet. Half-wiring `xdg-open` before there
is any way to report that it failed would be worse than the gap. M8.

**M4 — Sidebar. ✅ Done (2026-09-01).** XDG places, pinned locations, `places.toml`.

- **Places** are derived and not editable: `$HOME`, the `XDG_*_DIR` entries, and
  `~/.config`. **Pins** are the user's, persisted to `~/.config/omafiles/places.toml`.
- The XDG rules from §6.1 are all enforced and tested: skip anything that resolves to
  `$HOME` (this machine has `XDG_DESKTOP_DIR` and `XDG_TEMPLATES_DIR` both pointing
  there, which a naive sidebar renders as two more copies of Home), skip what does not
  exist, and **keep non-standard entries** — `XDG_PROJECTS_DIR` is real and dropping it
  would be wrong.
- Labels come from the directory on disk, not the `XDG_*` key, so a localised setup shows
  `Téléchargements` rather than "Downloads".
- Well-known directories are ordered deliberately rather than alphabetically; anything
  unrecognised is appended after, sorted.
- `places.toml` is written atomically (temp + `sync_all` + rename) and only stores a
  label when the user renamed the pin, so renaming the *directory* keeps the sidebar
  honest. A corrupt file loads empty and is **left on disk** — silently replacing it
  would destroy pins a typo could otherwise be fixed in.
- Keyboard: `Tab` moves between panes, `^p` pins the current directory, `Delete` unpins,
  `alt-j`/`alt-k` reorder. Reordering clamps rather than wrapping.

**Two states, both visible at once.** A sidebar row can be the *cursor* (where the
keyboard is) and/or *active* (the directory you are actually in) — you can browse
Downloads while the cursor sits on Documents, and both need to read distinctly. Active
matching is exact, not by ancestry: lighting up Home the whole time you are anywhere
under it tells you nothing.

The second focusable pane is what makes M3's key contexts pay off — `j` is bound in both
`Listing` and `Sidebar`, and means the right thing in each with no runtime check in either
handler.

**M5 — Tabs & workspaces. ✅ Mostly done (2026-09-01).** The first milestone with
persisted user data, and the failure modes landed where the plan said they would.

- **Tabs** carry their own path, history and cursor. M3's navigation became per-tab.
  Listings are cached per tab id so switching is instant, and dropped when a tab closes —
  otherwise a long session leaks one `Vec<Entry>` per directory ever opened.
- **Workspaces** are named groups with an active one that scopes new tabs. Global is
  workspace 0 with no name, rendered without a header.
- **Deleting a workspace moves its tabs to global.** The last tab anywhere is never
  closed — an app with no tabs has nothing to show and no way back.
- **Persistence** to `~/.local/state/omafiles/session.toml`, atomic (temp + `sync_all` +
  rename). Tabs restore on launch; a tab pointing at a deleted directory lands on its
  nearest existing ancestor.
- **Cross-instance sync** verified live: an external write with a higher revision is
  absorbed by the running app within a second.

All five tests the plan asked for exist and pass, plus repair cases for a session file
missing its global group.

### Two bugs worth recording

**The self-reload guard works, and it is load-bearing.** Instrumenting the watcher showed
`incoming rev 900 vs written 0 → absorbing`, then correctly refusing `900 vs written 900`.
Without it every one of our own writes would reload and clobber whatever the user was
doing in between.

**A hot loop the plan did not anticipate.** The guard stops us reloading our own *writes*
— but reading the file to *check* the revision emits an inotify **access** event, which
woke the watcher, which read again. A self-sustaining loop that pegged a core and produced
3.6 MB of log in seconds. Found by instrumenting rather than by reasoning; the log volume
was the tell. Fixed by filtering the watcher to `Create`/`Modify`/`Remove` only. The same
filter protects M3's directory watcher, which had the identical latent shape.

### Completed alongside M6

**Drag and drop** now works: `omarchy_ui::Row::draggable` carries a typed payload with a
themed drag preview, and each workspace header is a drop target that highlights on
hover. The header rather than the rows, because it stays hittable when a group is empty —
which is exactly when you most want to drag something into it. gpui routes drops by the
payload's **type**, so `DraggedTab` is its own struct rather than a tuple; two unrelated
drags sharing a shape would land in each other's handlers.

⚠️ Verified to compile and wired end to end, but **not exercised interactively** — this
machine has no mouse-automation tool (`ydotool`/`xdotool` absent), and `wtype` is keyboard
only. The keyboard path (`^⇧M`) is verified.

**Workspace naming** is now a modal prompt rather than an ordinal, using the new
`omarchy_ui::Modal`.

**M6 — Search. ✅ Done (2026-09-01).** `nucleo` filter, widening to a recursive walk.

- **`/` filters the current directory** instantly, over the already-loaded listing — no
  IO per keystroke. An empty query shows everything in the original order rather than
  nothing, since hiding the directory you are looking at is a worse default.
- **`^g` widens to a recursive walk** below the current directory, via `ignore`. Gitignore
  filtering is correct *here* and wrong in the listing: searching a repo, you almost never
  want its build output, and `target/` alone is 18 GB on this machine. Bounded by
  `RECURSIVE_LIMIT`, and the modal says when it truncated rather than implying the results
  are complete.
- Ranking breaks ties by label so equal scores cannot reshuffle between keystrokes and
  make the list jump under the cursor.
- Selecting a file navigates to its directory and puts the cursor on it — the useful
  outcome while opening files is still M9.

**Content search is not implemented.** §6.4's third mode (`ripgrep` via `grep-searcher`)
is a distinct feature with its own result shape; filter and recursive share an input and a
result list, content search does not. Deferred rather than rushed.

**`^p` stayed as "pin"** rather than becoming recursive search as §6.4 suggested — it
shipped in M4 and is in the hint bar. Widening is `^g`, and `/` is the single entry point.

**M7 — Preview. ✅ Done.** Text → markdown → tree-sitter → images → video poster.

Includes the **expanded preview** (§6.5): a button on the detail panel plus `Space`,
rendering the file's body over the listing column *and* the detail panel, navigable with
`j`/`k` while open and remembered per tab. The body renderer is a function of
`(file, target)` from the start — retrofitting that split after a panel-only version
exists is how the two end up drifting.

Delivered:

- `omafiles::preview` reads and classifies off the main thread, with no gpui context in
  it. Images, SVG-as-source, video posters via `ffmpeg`/`ffprobe`, markdown, code, plain
  text, binary hex, "too large", and unreadable.
- `omarchy_ui::SyntaxPalette` is §6.5's single mapping table. It serves both consumers —
  our own highlighter through gpui-component's `HighlightStyleResolver`, and
  gpui-component's `HighlightTheme` for fenced code inside a rendered markdown preview —
  so a source file and the same code in a README cannot drift apart.
- tree-sitter arrives through gpui-component's per-grammar features, as M0 recommended.
  Sixteen grammars, opted into individually rather than via the blanket
  `tree-sitter-languages` feature, which compiles 35.
- Markdown renders through gpui-component's `TextView` — headings, lists, tables and
  highlighted code blocks — rather than a hand-rolled element tree.

Changed from the plan: the expanded view is a pane, not a window overlay; it takes the
detail panel with it rather than leaving it to draw the same preview twice; and it shows
the body alone, not the title and fact table that belong to the panel. All three
corrections came from looking at it on screen. See §6.5.

Not done: **SVG previews as source, not as a picture.** `img()` would render it through
resvg, but someone opening an icon in a file manager more often wants to read it than to
see it at 16 px. Revisit if that turns out to be wrong.

**M8 — Git. ✅ Done (2026-09-01).** The branch in the status bar, status markers on the
entry icons, and the diff of a changed file in the preview. Design and measurements in
§6.9.

- `crates/omafiles/src/git.rs` is pure model, no gpui — 12 tests that run headless, and
  skip rather than fail on a machine with no `git`.
- **`git` for reads as well as for the switch** (§6.9, revised there). The reasoning lives
  in the module docs beside the code, so nobody has to go looking for why `gix` is absent.
- **The branch is read inline; the status is not.** The head is two filesystem reads, so
  the status bar has a branch the instant you navigate; status is the 400 ms half and lands
  a moment later — M3's "render now, fill in when it arrives" discipline.
- Directory rollups are computed once when the status lands and indexed by path prefix, so
  a row costs one hash lookup. The rollup stops at the repository root, or a marker leaks
  onto the user's home directory.
- Marker glyphs and colours come from palette roles — `+` green, `•` yellow, `−` and `!`
  urgent, `?` muted — composited onto the entry's existing icon. Conflicted is `!` rather
  than a second red dot, because §6.9 asks for it to be distinguishable from deleted and
  the two share a colour.
- **A changed file previews as its diff**, and the diff is rendered as *the file*, not as
  the patch — see below. It inherits the fact table and M7's expand affordance, which is
  where a diff is most useful since a hunk rarely fits the panel. Only textual bodies are
  replaced: a modified PNG's diff is one line about binary files differing, which says less
  than the picture does.
- The switcher is `^⇧g`, or a click on the status bar. It filters from a field, runs the
  checkout on the background executor, and on refusal shows git's message **verbatim** with
  the repository untouched.
- `.git` is watched, so a commit or a switch made in a terminal is noticed.

Every test the plan asked for exists and passes, plus porcelain-parsing cases for paths
containing spaces and for renames — whose second record has to be consumed rather than read
as another entry.

### The diff view, revised: render the file, not the patch

The first version showed `git diff`'s output as text under M7's `diff` grammar. It was
three lines of code and it read like a terminal. Zed does something better, and the
difference is worth stating because it is a *rendering* decision, not a library one — Zed's
`buffer_diff` and its editor are GPL-3.0-or-later on top of the whole `language`/`text`/
`rope`/`multi_buffer` stack, so none of it is takeable even if we wanted it. What is
takeable is the shape:

| | Patch as text | Rows of the file (now) |
| --- | --- | --- |
| Colour | the `diff` grammar tints whole lines by their prefix | the **file's own** grammar; a Rust hunk is Rust |
| Marking | a `+`/`-` character glued to the code | a full-width wash behind the row, plus a sign in its own column |
| Preamble | `diff --git`, `index 814f4a4..190d04c`, `---`, `+++` all on screen | dropped |
| Position | `@@ -1,2 +1 @@` arithmetic | the file's real line numbers, in a gutter |
| Hunk gaps | another `@@` line | `⋯` with git's section heading |

So `git::Diff` is parsed into hunks of tagged lines, and `main.rs` renders one element per
line. Three details are load-bearing:

- **Both sides are parsed whole.** Removed lines are not in the file any more, so the
  `HEAD` blob is read and parsed too, and each row borrows its colours from its own side
  indexed by line number. Handing tree-sitter a hunk on its own instead gives an `ERROR`
  node a line or two in and no captures after it — which is precisely where a diff needs
  them.
- **A removed line's gutter is blank.** It has no line number in the file as it stands,
  and printing the *old* one puts a 140 between a 145 and a 146 so the column stops
  counting. Zed leaves it blank for the same reason; the sign already says which side the
  row is on.
- **The wash is a palette colour at `[controls]`' selected alpha** — the same fill a
  selected row uses. That is what makes it survive `vantablack` and `white`, where a solid
  green would be violent, and it is why no colour is written down anywhere.

The line cap came down from 2,000 to **600** with it: one element per line is the price of
a full-width wash, where a text preview is a single laid-out run. Six hundred lines is also
past the point where something stops being a preview.

Not done: **word-level highlighting inside a modified line**, which is the next thing a
diff view gains and needs a second diff pass per line pair.

### The bug this milestone found in M6

**No action reached a handler while a modal's text field had focus.** `esc` did not close
the search palette, `↓`/`↑` did not move its list, and `^g` never widened anything. Two
causes, both invisible without driving the real window:

- The handlers were on the content column, and the overlay is a **sibling** of that column
  (M2's own layout note). gpui dispatches an action up the *focus* path, so with the modal
  focused the column was not on it. The handlers moved to the outermost container; the
  bindings stay context-scoped, so `j` still means only the listing's.
- The arrow and `^g` bindings were scoped to an `Input` key context that **does not
  exist** — gpui-component's `Input` sets no `key_context` at all, so they never matched.
  They are bound without a context now, which is safe because each handler is inert unless
  its overlay is open.

The same shape as the theme-subscription trap in §5: wired correctly, compiling cleanly,
doing nothing.

**Not done:** ignored files carry no marker and are not dimmed. Finding them means
`--ignored`, which walks every ignored path — `target/` alone is 18 GB here — and §6.9's
whole treatment of ignored was "no marker; dimmed name at most" anyway.


**M9 — Actions. ✅ Done (2026-09-01).** Terminal, agent, LocalSend, and open-with —
which finally makes `⏎` on a file do something outside the preview. All four go through
Omarchy's own scripts, per §6.6, so they inherit the user's configured terminal and agent.

- `crates/omafiles/src/actions.rs` is pure `std`, no gpui — the command lines are
  composed by pure functions the tests assert on without spawning anything.
- **Two spawning disciplines, and which one an action gets is the design.** The terminal
  and the agent are *launch and let go*: they outlive us on purpose, only a failure to
  spawn is reportable, and a reaper thread waits on the direct child so it cannot sit
  defunct in the process table. `xdg-open` and `omarchy-menu-share` are *run and read
  the answer*: they exit quickly, their exit status is the only failure channel, and they
  run on the background executor.
- **`⏎` runs our own `xdg-open`, not gpui's `open_with_system`.** gpui's fires and
  forgets, logging failures where no user will see them — and the entire reason
  open-with waited from M3 to M9 was to be able to say *in the window* that it failed.
  xdg-open's stderr is usually empty, so its documented exit codes (2 no such file,
  3 no application, 4 launch failed) are decoded into sentences.
- **Failures land in the status bar**, urgent-coloured, gone after six seconds. A
  sentence, not a modal: "no application can open this" is not worth interrupting the
  next keystroke. The expiry timer is generation-guarded so a notice that was already
  replaced cannot have its slower timer clear the newer one.
- **`a` composes the prompt in a dialog before launching** (§6.6's "editable in a small
  dialog"). The subtitle says which agent and in which directory — Enter must never
  launch something the dialog did not describe. With no default agent configured, `a`
  surfaces Omarchy's own picker (`omarchy-menu summon setup.default.agent`) instead of
  an error, plus a notice saying to press `a` again once chosen.
- `t` mirrors the body of `omarchy-launch-terminal` minus its `omarchy-cmd-terminal-cwd`
  call — that script asks the active *terminal* for its directory, and we already know
  ours. Bound in both panes; `a` and `s` act on the cursor entry, so they live where the
  cursor does.
- `s` shares files as `file` and directories as `folder` through `omarchy-menu-share`,
  and never touches `localsend` directly — §6.6's warning about the binary hanging on
  `--help` stands.

Verified on screen by driving the real window with PATH shims recording what would have
launched: the agent dialog carries the composed prompt and the right cwd through to
`omarchy-agent-prompt`, `s` hands the path to `omarchy-menu-share file`, `⏎` on a broken
symlink puts xdg-open's own message in the status bar and the notice clears itself, and
`t` opened a real `foot` at the listing's directory — the terminal's title said so.

**Added after: the same actions for the mouse.** The keyboard verbs got visible twins,
split by scope the way the facts already are. The **status bar** carries Terminal, Agent
and Share buttons acting on the *directory* — the status bar is where facts about the
directory live, so its buttons act on the directory too (its Share sends the folder, its
Agent asks about the directory). The **detail panel** carries Open, Agent and Share
buttons acting on the *cursor entry*, on the same toolbar line as M7's expand control,
beside the facts they act on. Both routes go through the same handlers as the keys, so
there is nothing the mouse can do that the keyboard cannot.

**M10 — HTTP server. ✅ Done (2026-09-01).** In-process `axum`, per §6.7 — stopping is
dropping a handle, status is read from the same process, and quitting the app cannot
leave a server behind.

- `crates/omafiles/src/server.rs`, headless-testable: the tokio runtime axum needs
  lives on **one thread owned by the handle**, and the rest of the app never learns
  tokio exists. The tests speak raw HTTP/1.1 over a `TcpStream` rather than growing a
  client dependency.
- **Two facts pinned at start and never silently changed**: the root (the directory
  current when started — navigating away does not move it) and the bind.
  `127.0.0.1` by default; `0.0.0.0` is a separate row whose warning is in the row
  itself, coloured urgent.
- **The bind is synchronous**, so a taken port or denied bind errors on the click that
  asked — not as a background failure discovered later. Port 8080 first; on conflict
  the OS picks, because a second window serving a second directory is legitimate.
- Files go through `tower-http`'s `ServeDir` (mime, ranges, streaming); the directory
  listing is ours because `ServeDir` has none. Dotfiles stay out of it, matching the
  app's own default. Paths are resolved component-by-component with `..` refused, and
  a refusal is a 404, not a 403 — what exists outside the root must not leak.
- **The status bar carries the server as a button** (revised §6.7): `http off` dim,
  `:8080` accent when serving — with `← path` appended when browsing elsewhere. The
  contextual menu behind it (`^s` or click) shows start options when stopped; running,
  it shows the URL, a live log (a 200-line ring, repainted twice a second only while
  the menu is open), request count, open-in-browser, copy-URL and stop. Enter in the
  menu starts the loopback server; stopping stays a deliberate click.
- The log line format is uptime-relative (`+  12s GET /x → 200`) — dependency-free,
  and "is it working, who asked for what" needs no wall clock.

Verified on screen: `^s` → Enter started serving, three real `curl`s (a listing, a
file, an escape attempt) appeared in the live log as `200 200 404`, the badge showed
`:8080` in accent and grew the `← path` suffix on navigating away, and killing the app
released the port — `curl` refused afterwards.

**Found while wiring it:** Enter over a field-less modal (help, the workspace menu)
fell through to the listing and navigated *under* the overlay — the `Open` handler
never checked for one. It now routes to the overlay's confirm instead, which is also
what makes the server menu keyboard-startable.

**Deferred:** the QR code §6.7 offers for phone access. `omarchy-capture-qr` turned out
to *decode* QR codes, not display them, so there is no Omarchy-native display path to
reuse; `qrencode` is installed and a follow-up can render one into the menu.

**M11 — Polish & desktop integration. ✅ Done (2026-09-01).** The command palette, the
rebindable keymap, and the packaging.

- **The keymap became data** (`crates/omafiles/src/keymap.rs`, pure and headless-tested).
  The defaults are a table; `~/.config/omafiles/keymap.toml` merges over them with the
  simplest rule that cannot surprise: *naming an action in a section replaces every
  default key that action had there* — a string binds one key, a list several, an empty
  list unbinds, and what the file does not name keeps its defaults. A broken file never
  stops the app: the defaults stand and the problem is a status-bar notice in the file's
  own words, because a file manager that refuses to start over a config typo locks the
  user out of the tool they would fix it with.
- `bind_keys` consumes that data; `typed_binding` is the one place a name meets its
  typed gpui action, and a test asserts every known name resolves — the failure it
  prevents is a rebind that silently does nothing.
- **`^k` — the command palette**, §6.8's discoverability mechanism: every action worth
  invoking by name, substring-filtered, Enter dispatches the real gpui action so the
  palette can never drift from the keys. Movement verbs are deliberately absent — a
  palette that moves the cursor one row is slower than the key it documents. The key
  hints show the keymap's **effective** keys, so a rebound action reads correctly there;
  the grouped help sheet (`?`) stays curated prose and describes the defaults.
- The expanded preview's footer hint now reads its collapse key from the keymap —
  caught on screen promising `space` after space had been rebound away.
- Confirming an overlay gained the window (`subscribe_in` rather than `subscribe`),
  which is what lets Enter in the palette dispatch an action.
- **`contrib/`**: the `.desktop` entry (installed as `dev.omarchy.omafiles.desktop` —
  the file name must match the Wayland app_id, `desktop-file-validate` clean), a
  `PKGBUILD` building the checkout's HEAD with `--locked` (source/pkgver machinery
  verified with `makepkg --nobuild`), the `SUPER+SHIFT+F` takeover as an opt-in Lua
  snippet in Omarchy's documented unbind-then-rebind shape, and a README covering the
  mimeapps takeover, the hook install and `keymap.toml`. Everything touching `$HOME` is
  opt-in and documented rather than written by the package — and the mimeapps takeover
  stays a user choice because §9's "replace Nautilus?" is still the user's question to
  answer.
- **No Hyprland window rules shipped.** The plan reserved a slot for them; nothing
  currently needs one — the app tiles like any other window and has no floating
  children. A slot deliberately left empty rather than filled with a rule that does
  nothing.

Verified on screen with `XDG_CONFIG_HOME` pointed at a scratch config: a keymap
rebinding `toggle_preview` to `p` worked on the first keypress (and the palette's hint
showed `p`), the deliberately broken line in the same file produced
`keymap.toml: unknown action "fly_to_the_moon"` in the status bar, and `^k` → "sidebar"
→ Enter closed the palette and toggled the sidebar.

The original integration checklist, for the record:

- `.desktop` file with app id `dev.omarchy.omafiles`, and set
  `inode/directory=dev.omarchy.omafiles.desktop` in `~/.config/mimeapps.list` to take
  over from `org.gnome.Nautilus.desktop` (Omarchy's default lives in
  `/usr/share/omarchy/default/applications/mimeapps.list`).
- Keybindings. Omarchy's defaults are in
  `/usr/share/omarchy/default/hypr/bindings/applications.lua`:
  `SUPER+SHIFT+F` → file manager, `SUPER+ALT+SHIFT+F` → file manager (cwd). The
  documented override pattern in `~/.config/hypr/bindings.lua` is `hl.unbind(key)`
  followed by `o.bind(key, desc, cmd)`. Ship this as an opt-in snippet, not something
  we write into the user's config unasked.
- Hyprland window rules — match on our app class, following the shape in
  `/usr/share/omarchy/default/hypr/apps/system.lua`.
- Install `contrib/hooks/omafiles-theme-reload` into `~/.config/omarchy/hooks/theme-set.d/`.

---

## 8. Decisions taken (and why)

| Decision | Alternative rejected | Why |
| --- | --- | --- |
| Port `Style.qml` faithfully | Invent our own token set | Native feel; and the whole point of the crate is *observing* system config |
| Reimplement colour resolution in Rust | Shell out to `omarchy-theme-color` | Hot path, no fork per reload; keep the CLI as a test oracle |
| Watch the *parent* directory | Watch `colors.toml` | The theme dir inode is replaced on every switch — see §2.5 |
| Also install a `theme-set.d` hook | inotify alone | Sanctioned path, survives watch starvation |
| `hyprctl` for radius/gaps | Parse `looknfeel.lua` | Config is Lua; `hyprctl` gives the *effective* value |
| `nucleo` in-process | Spawn `fzf` | fzf is a TUI; can't style its output or drive it per-keystroke cleanly |
| `axum` in-process HTTP | Spawn `miniserve`/`python -m http.server` | Clean stop, real status, no orphan pid, no extra dep |
| Reuse `omarchy-*` scripts for actions | Reimplement | Inherits user's configured terminal/agent, stays correct across Omarchy updates |
| Three crates | One | `omarchy-tokens` tests headless in CI without the heavy gpui git dep |
| gpui via **git rev pins**, both repos | crates.io `gpui 0.2.2` | crates.io froze in Oct 2025 and is the *pre-split* monolith with the dead Blade renderer. "Stability" there means shipping a 10-month-old graphics stack. If we skip gpui-component, `gpui-unofficial = "=1.17.2"` is a clean crates.io path to the current API. |
| Adopt `gpui-component` (leaning yes) | Bare gpui | Bare gpui has **13 elements total** and no text input, **no visible scrollbar at all**, and no context menu. A file explorer needs all three. Cost is accepted in the risks table. Final call is M0's. |
| `Transparent` + Hyprland windowrule | gpui `Blurred` | `Blurred` is a no-op on Hyprland (KDE-only protocol); the compositor's own blur is better anyway |
| `img(path)` for file icons | `svg(path)` | gpui's `svg()` is a monochrome alpha mask; `img()` goes through resvg in full colour |
| The expanded preview takes the centre pane **and** the detail panel | A full-window overlay, or an OS fullscreen request | Built as an overlay first, then as centre-only; both were wrong. Covering the window hides the sidebar you navigate with; leaving the detail panel docked drew the same preview twice. The compositor already does window fullscreen (`SUPER+F`) |
| Preview renderer takes its size as a parameter | Separate pane and fullscreen renderers | Two renderers drift, and the less-used one is the one nobody notices is broken |
| Single process, multiple windows | Multiple processes sharing a file | Makes "synced across instances" true by construction and deletes a whole class of concurrent-write bugs. The state file + watcher stays underneath so a stray second process converges rather than corrupting. |
| Session state in `~/.local/state` | `~/.config` | It is machine-written on nearly every navigation. `places.toml` stays in config because it is user-curated. |
| Deleting a workspace keeps its tabs | Delete the tabs with it | A workspace is a grouping, not an owner of lifetimes. Losing tabs to a mis-click is the kind of thing people do not forgive. |
| Workspaces have stable ids, names are labels | Key by name | Renaming must not orphan the tabs inside. |
| `gpui-component` (Apache-2.0) | Zed's `ui` (GPL-3.0-or-later) | Zed's crates would relicense omafiles and invert the theming architecture. `GPUI-NOTES.md` §9 |
| `omarchy-ui` drives gpui-component's `Theme` global | Two theme systems side by side | One source of truth; their widgets render in Omarchy's palette for free |
| gpui floats, `Cargo.lock` is the pin | `rev =` on gpui | A rev-pinned and a floating git source do not unify — pinning ours yields two copies of gpui and a broken build. Verified, not assumed. `GPUI-NOTES.md` §1 |
| Shell out to `git` for reads **and** the switch | `gix`, as originally planned | Revised in M8. The binary is already a hard requirement for the switch, `--porcelain=v2 -z` gives the staged and worktree halves in one pass, and the diff view wants unified diff *text* — which `git diff` produces and `gix` has no formatter for. §6.9 |
| Never reimplement the checkout | `gix` or `git2` for the switch | Clobbering uncommitted work is unrecoverable, and git already refuses when a checkout would. Inheriting its safety checks and its error text beats reimplementing them. §6.9 |
| The diff renders as rows of the file, syntax-highlighted, under a wash | `git diff`'s text under the `diff` grammar | Built the cheap way first and it read like a terminal. Zed's treatment keeps the code's own colours, drops the `+`/`-` prefixes and the `diff --git`/`index` preamble, and puts real line numbers in a gutter. §7, M8 |
| Parse the `HEAD` blob too | Highlight the hunk fragment on its own | Removed lines are not in the file any more, and tree-sitter handed a fragment hits an `ERROR` node a line or two in and stops producing captures — exactly where the diff needs them |
| The branch in the status bar | In the header, as originally planned | The header is the *path*; a branch is a fact about where you are, not part of the address, and there it competed with the breadcrumb for width. §6.9 |
| No branch label on listing rows | Inline `omafiles ⎇ develop` per row | Costs a repository open per row, and with the branch in the status bar it mostly repeats what is already on screen. The stacked version was never possible: it costs `uniform_list`. §6.9 |
| Action handlers on the outermost container | On the content column, beside the key context | The overlay is a *sibling* of the column and gpui dispatches up the focus path, so handlers on the column are unreachable from a modal. Found in M8; it had made `esc`, `↓`/`↑` and `^g` inert in every modal since M6 |
| Git status on a background task, cached per repo | Compute inline | Measured 400 ms on a 4,288-file repo — four dropped frames. §6.9 |
| Our own `xdg-open` for `⏎`, waited on | gpui's `open_with_system`, as §6.6 planned | gpui fires and forgets, logging failures where no user looks. The whole reason open-with waited from M3 to M9 was to report failure in the window, and the exit code is the only failure channel xdg-open has |
| Action failures are a status-bar notice, expiring | A modal, or silence | "No application can open this" is worth a sentence, not a dialog blocking the next keystroke. The expiry is generation-guarded so a replaced notice's slower timer cannot clear the newer one |
| The agent launches through a prompt dialog | Fire a canned prompt immediately | The prompt is ours, and §6.6 asked for it editable; the dialog's subtitle names the agent and the directory, so Enter never launches something undescribed. No agent configured → Omarchy's own picker, not an error |
| Launch-and-let-go spawns get a reaper thread | `wait()` inline, or nothing | Waiting inline blocks on the terminal's lifetime; not waiting leaves the direct child defunct until we exit. A thread per launch is cheap at the rate humans press `t` |
| Mouse actions split by scope: directory buttons in the status bar, entry buttons in the detail panel | One toolbar somewhere | The status bar holds the directory's facts, the panel holds the entry's — the buttons sit beside the facts they act on, and both routes share the keyboard handlers |
| The server lives in the status bar as a button + contextual menu | The header toggle §6.7 planned | Requested; and it is M8's branch argument again — server state is a fact about the app, not part of the address. One menu serves both states: start options stopped, log and stop running |
| The server's tokio runtime on one thread owned by the handle | tokio as the app's executor | gpui has its own executor; the whole tokio surface stays inside `server.rs`, and dropping the handle joins the thread — stop is provable |
| Server path resolution component-by-component, refusals are 404 | `root.join(request_path)` | A joined absolute or `..` path walks out of the root; and a 403 would confirm that something exists to be forbidden |
| `Enter` over any modal routes to the overlay's confirm | Letting it fall through to the pane | Found in M10: with a field-less modal open (help, menus) Enter navigated the listing under the overlay. The same fix is what makes the server menu keyboard-startable |
| Keymap defaults as data, `keymap.toml` merged by replacement | Patching individual keys | "Naming an action replaces its defaults in that section" is explainable in one sentence; per-key patching needs a remove syntax and two ways to say everything. §6.8, M11 |
| A broken keymap.toml is a notice, not a refusal to start | Fail fast on config errors | The defaults stand either way, and a file manager that will not start over a typo locks the user out of the tool they would fix it with |
| The palette dispatches real gpui actions | A parallel table of method calls | One dispatch path means the palette cannot drift from the keys; its hints read the *effective* keymap, so a rebound action shows its real binding |
| Palette filtering is substring, not fuzzy | `nucleo`, as the file search uses | Two dozen labels need no ranking model, and stable order beats clever order in a list that small |
| `contrib/` is opt-in configuration plus reference copies | Package writes into `$HOME` | Writing user config unasked is how software loses trust; the package installs the binary and the `.desktop`, the README shows the rest. Also keeps §9's "replace Nautilus?" the user's decision |
| Content search shells out to `rg --json` | The `grep-searcher` crates | M8's git reasoning again: `rg` ships with Omarchy, `--json` is documented plumbing, and it already knows gitignore, binaries and encodings. Literal `--fixed-strings` because a query dying on `(` looks broken, not regexy |
| Content search caps everything and says so | Complete results | Per-file, per-line, and 200 total; the child is killed at the cap and the modal admits the cut. A content search is a way in, not a report |
| The path editor is a modal panel completing from descendants | The inline header field with suggestion chips | Rebuilt by request. An existing path completes to its children, so the panel opens useful before a key is typed; suggestions became real pickable rows where the chips were only decoration, and the header never reflows |
| Path panel: `None` cursor means Enter takes the typed text | Always highlighting a suggestion | Typing an exact path then Enter must go *there*, never to whichever child happened to sort first; arrows opt into the list, typing opts back out |
| One `ActionButton` behind every small chrome verb | `icon_button`, `Button`, `Row` and two hand-rolled badge divs, per site | Uniformized on request. One primitive means one geometry (a step below `control-height`, square when glyph-only, radius capped at 2), one hover/pressed/disabled treatment and one subtle idle border (half the control border's alpha) — uniform by construction rather than by diligence. `Button` stays for the emphatic case; the git badge's branch and counts ride as children so information keeps its emphasis and palette roles |

---

## 9. Open questions for you

1. **App name and binary.** Repo is `omafiles`. Keep `omafiles` as the binary and
   `dev.omarchy.omafiles` as the app id? It reads as an Omarchy-official name — fine if
   that is the intent, worth a different name if not.
2. **Replace Nautilus?** Omarchy binds `SUPER+SHIFT+F` / `SUPER+ALT+SHIFT+F` to Nautilus
   and registers it as the `inode/directory` handler. Should omafiles take those over
   (M10 has the mechanism), or coexist? Related: Omarchy ships a nautilus-python
   extension at `default/nautilus-python/extensions/localsend.py` that adds
   "Send with LocalSend" to Nautilus' context menu — §6.6 is the native equivalent, so
   this is a feature we are replicating rather than inventing.
3. **Video playback depth.** Poster frame + metadata (cheap, v1), or genuine inline
   playback (`libmpv` embed — significant work, see §10)?
4. **Dual-pane / Miller columns?** The brief implies a single list + preview. A
   Norton-Commander two-pane mode is a different app; worth deciding now because it
   affects the navigation model.
5. **File operations scope for v1.** Is copy/move/rename/delete in, or is v1 a
   *browser* (navigate, search, preview, act) with mutation deferred?
6. **Do you actually need concurrent processes?** §6.2 recommends a single process with
   multiple windows, which makes syncing free. If you genuinely want two independent
   `omafiles` processes editing workspaces at once, that needs an operation log to
   converge without losing tabs — a meaningful chunk of work I would rather not build
   speculatively. Multiple *windows* works either way.
7. **Do tabs restore on launch, or start clean?** Persisting them means reopening where
   you left off; it also means a tab pointing at an unmounted drive or a deleted
   directory on every start. M5 degrades those to the nearest existing ancestor, but
   "restore everything" versus "restore workspaces, start with one fresh tab" is a taste
   call I would rather you made.

---

## 10. Risks

| Risk | Severity | Mitigation |
| --- | --- | --- |
| **gpui is not a stable public library.** It is Zed's internal framework; the usual dependency is a git pin, and the API has churned (`ViewContext`/`WindowContext` → unified `Context`, `View`/`Model` → `Entity`). Most tutorials online are wrong. | High | Pin an exact git rev, never a branch. Budget for API breakage on every bump. Keep gpui contained inside `omarchy-ui` so the app and tokens crates are insulated. See `plan/GPUI-NOTES.md`. |
| **`gpui-component` does not pin its own gpui dependency.** Pinning *ours* makes it worse, not better: a rev-pinned and a floating git source do not unify, so the graph gets two copies of gpui and the build fails outright. `[patch]` is rejected as pointing at the same source. | High, and **hit in practice** | gpui floats to match; `Cargo.lock` is the pin. `cargo update -p gpui` becomes a deliberate re-verify-M0 event. `GPUI-NOTES.md` §1 |
| ~~**Local images may not render on Linux**~~ ([#2527](https://github.com/longbridge/gpui-component/issues/2527)) | **Resolved** | Does not reproduce on bare gpui — `img(PathBuf)` renders a local PNG fine. #2527 is a gpui-component bug, not a gpui one. The preview pane is unblocked. `GPUI-NOTES.md` §8 |
| **gpui caches images by path, not content** — a theme switch replaces `preview.png` in place and the stale image persists | Medium | Found in M0. Key the preview cache on `(path, mtime, size)`, as §6.5 already specifies. |
| **Table/context-menu gaps confirmed by prior art.** Ferail — the one shipping file manager on this stack — filed four open, uncommented blockers, including table rows not carrying click `Modifiers` (no ctrl/shift multi-select) | Medium | Already mitigated by building our own list and `InteractiveSurface` rather than taking gpui-component's. `GPUI-NOTES.md` §7 |
| gpui on Wayland — IME specifically | Medium | Wayland is the *better-maintained* Linux backend (most open IME bugs are X11/XIM), and M0 cleared rendering, tiling, transparency and fonts. But dead keys/compose have an open regression ([#60964](https://github.com/zed-industries/zed/issues/60964)) and **nobody upstream has tested fcitx5 here at all**. Still open — validate in M6, when there is a text field. |
| ~~Runtime font-family change needs a window rebuild~~ | **Resolved** | It does not. cosmic-text + fontconfig; `cx.notify()` re-shapes next frame. See §5. |
| ~~**AMD/RADV rendering artifacts on Mesa 26.2.1**~~ ([#63358](https://github.com/zed-industries/zed/issues/63358)) | **Resolved here** | No artifacts in M0. That report is Navi31 discrete; this machine is Renoir (Vega iGPU) on the same Mesa. Watch it if the project moves to discrete AMD. |
| Video playback in Rust on Linux is genuinely hard — and worse than assumed: gpui's `surface()` zero-copy path is **macOS-only**, and `mpv --wid=` cannot work on Wayland (no window embedding) | Medium | Confirms v1 = poster frame via `ffmpeg-next`/`video-rs`. Real playback would mean GStreamer `appsink` → CPU convert → `RenderImage`, written by us. |
| Tree-sitter grammars bloat build times and binary size | Low | Static, curated grammar set; measure |
| `notify` on a huge directory or a network mount | Low | Cap watch depth at 1, debounce, degrade to manual refresh |
| Omarchy 4.x changes the token schema | Low | The conformance test catches palette drift on the next `pacman -Syu`; unknown keys parse-and-ignore rather than erroring, and only `background`/`foreground` are required (§2.2c) |
| ~~**A watcher loop from our own reads**~~ — reading the session file to check its revision emits an inotify *access* event, waking the watcher to read again. Pegged a core. | **Found and fixed in M5** | Filter watchers to `Create`/`Modify`/`Remove`. Reads must never look like changes. The same filter fixed the identical latent shape in M3's directory watcher. |
| **Reloading our own session writes** — the watcher sees our atomic rename and clobbers in-flight UI state | Medium | A monotonic `revision` the writer remembers; ignore reloads of a revision we produced. Regression-tested in M5. Only bites when a write and an edit overlap, so it will not show up casually. |
| **Concurrent processes losing tabs** if the single-instance handoff is ever dropped | Medium | Last-writer-wins is not sufficient — two processes each opening a tab would lose one. The correct fix is an operation log (§6.2), which is real work and should not be built on speculation. §9 asks whether it is needed. |
| **Git status is slow on large repos** — 400 ms measured on 4,288 tracked files, and monorepos are worse | Medium | Background + per-repo cache; the listing renders without markers and gains them when they land. Never inline. §6.9 |
| A branch switch destroying uncommitted work | **High if got wrong**, **mitigated in M8** | Never reimplement checkout: shell out to `git switch`, never force, and surface git's refusal verbatim. Regression-tested — a refused switch must leave the file byte-for-byte as it was |
| ~~**A `git status` / inotify loop.** `git status` rewrites `.git/index` to refresh its stat cache, and we watch `.git`. The write wakes the watcher, which re-runs status, which writes again — M5's session-file loop in a new costume | **Avoided in M8** | `--no-optional-locks` on every read. It is one flag, and leaving it off any single call site is enough to restart the loop, so all of them go through one `git()` helper |
| Repo detection stat-walking to `/` on every navigation | Low, **handled in M8** | Cache the negative result per directory. The cache counts its own walks, so the test asserts on the shipped path rather than on a `cfg(test)` counter |
| Session file grows without bound as tabs accumulate | Low | Cap history depth per tab; prune tabs whose path has not existed for N launches |
| **Silent fallbacks hide real bugs** — M0 shipped two: a palette that failed to load fell back to a built-in theme, and a discarded TOML error made `base-size` quietly revert to 12 | Medium | Both are now regression-tested. Rule: a fallback that a user could mistake for correct behaviour must log, and must have a test that asserts the *non*-fallback path. |
| `localsend --headless` behaviour is undocumented and the binary hangs on `--help` | Low | Only ever invoke via `omarchy-menu-share`; treat as fire-and-forget |

---

## 11. Status

**M0 complete (2026-09-01).** Rust 1.97.1 via mise (`rust-toolchain.toml` is honoured —
mise's rust is rustup-backed). Every system library gpui needs was already installed.
gpui builds and renders on this machine; full results and build cost in
[`GPUI-NOTES.md`](./GPUI-NOTES.md) §8.

`omarchy-tokens` landed with the palette derivation chain and the conformance suite —
22 themes × 38 keys agreeing with `omarchy-theme-color`. It is pure logic with no gpui
dependency, so it stays valuable regardless of which UI toolkit the project ends up on.

**M1 complete (2026-09-01).** Structural tokens and the watcher; a running window
retints within a few hundred milliseconds of `omarchy theme set`, with no keypress.

**M2 complete (2026-09-01).** `ActiveTheme`, `InteractiveSurface`, the contrast floor,
eight components and the gallery. 65 tests, clippy clean.

Ten milestones remain planned, one added since the original ten: **M5 — Tabs &
workspaces** (§6.2), which pushed search, preview, actions, the HTTP server and polish
down one number each.

**Component layer decided (2026-09-01).** `gpui-component` is adopted, with `omarchy-ui`
driving its theme — `GPUI-NOTES.md` §9. The deciding facts were that Zed's own `ui` crate
is GPL-3.0-or-later, and that gpui-component's Apache-2.0 kit can be made to render in
Omarchy's palette because its `Theme` is a gpui `Global`. The bridge is written and
tested; 69 tests pass.

**M3 complete (2026-09-01).** The explorer skeleton, and the component-layer question
answered in practice: gpui's `uniform_list` + our `Row` + gpui-component's scrollbar only.
90 tests, clippy clean.

Eleven milestones now, two added since the original ten: **M5 — Tabs & workspaces**
(§6.2) and **M8 — Git** (§6.9).

**M4 complete (2026-09-01).** The sidebar: derived places, user pins, atomic persistence,
and a second focusable pane.

**M5 mostly complete (2026-09-01).** Tabs, workspaces, session persistence and live
cross-instance sync. 116 tests, clippy clean. **Drag and drop is outstanding** — the
keyboard equivalents work, dragging a tab between workspaces does not.

**M6 complete (2026-09-01).** Fuzzy filter and recursive search, the `Modal` component,
and M5's drag-and-drop finished. 124 tests, clippy clean.

**Chrome pass (2026-09-01).** A round of UI work on top of M6:

- The status bar carries the directory summary and a single **`? Help`**; the wall of key
  hints became a modal listing every shortcut, grouped. One affordance instead of a
  permanent strip of them, and the full list is a keystroke away.
- The sidebar and detail pane are **real panels**: scrollable, collapsible (`^b`,
  `^⇧b`), and below `dropdown_width × 3` they stop taking space and dock as overlays
  instead. The threshold is derived from the token scale rather than a magic number, so
  it tracks the user's text size. Collapse state is *intent* — a narrow window overrides
  it without clearing it, so widening restores the panels.
- The navigation bar is editable (`^l` or click the path) with directory completion, and
  has back / parent buttons. Completion resolves against the typed path's parent and
  filters by its last component, the way a shell does.
- Workspace headers gained `+` (new tab **in that workspace**) and `…` (rename / delete).
  The default group has no header at all — it is where tabs live, not something the user
  made and can act on.
- Padding moved out of the main column and into each panel, so the bars sit flush.
- The panels lost their card chrome: no fill, no border, no rounded corners. Full-height
  hairline rules between the three columns are the only thing dividing them, and they run
  edge to edge because the shell's own padding went with the chrome.

**M7 is done.** The preview reads and classifies off the main thread and renders through
one function at two targets: the detail panel, and the listing column when expanded.
Sixteen tree-sitter grammars via gpui-component, coloured by a single capture-to-palette
table in `omarchy-ui` that also drives fenced code inside rendered markdown. Video posters
and metadata through `ffmpeg`/`ffprobe`, degrading to a plain card when neither is
installed. `Space` expands, `j`/`k` page through the folder without collapsing, and the
state is per tab and persisted.

Verified on screen against a directory holding an ELF binary, a PNG, an MP4, a Rust source
file and a README: each renders its own treatment, the expanded view keeps the sidebar
docked, and switching tabs shows each tab's own state.

**M8 is done (2026-09-01).** Git: the branch and a change summary in the status bar, status
markers composited onto the entry icons, and a changed file previewing as its diff — which
inherits M7's expand affordance, since that is where a diff is most useful. Reads and the
switch both go through the `git` binary rather than `gix`; §6.9 and the module docs say
why. 170 tests, clippy clean.

Verified on screen against a scratch repository holding every state at once — a staged new
file, two modified files, a deletion and an untracked one — plus this repository. The
markers colour correctly, the status bar reads `⎇ main +1 •2 −1 ?1`, `^⇧g` opens the
switcher, and a switch git refuses shows git's own words with the working tree untouched.

The diff view was then rebuilt to Zed's shape — rows of the file, coloured by the file's
own grammar, under a full-width wash, with real line numbers — after the first version
rendered `git diff`'s text under the `diff` grammar and read like a terminal. Zed's code is
GPL-3.0-or-later and sits on its whole editor stack, so what was taken is the treatment,
not the crate. §7, M8.

It also turned up a real M6 bug: **no action reached a handler while a modal's text field
had focus**, so `esc`, `↓`/`↑` and `^g` were inert in every modal. Both causes and the fix
are recorded under M8 in §7.

**M9 is done (2026-09-01).** Actions: `t` terminal, `a` agent with an editable prompt
dialog, `s` LocalSend, and `⏎` open-with — all through Omarchy's own scripts, with
failures reported as an expiring status-bar notice. The open-with gap M3 left on purpose
is closed the way it was waiting to be: `xdg-open`'s exit code read and put on screen.
182 tests, clippy clean.

Verified by driving the real window with PATH shims recording what would have launched:
the composed prompt and cwd reach `omarchy-agent-prompt` intact, the share path reaches
`omarchy-menu-share file`, a broken symlink's `⏎` puts xdg-open's own words in the status
bar (and the notice clears itself six seconds later), and `t` opened a real `foot` whose
title showed the listing's directory.

**The M9 verbs then grew mouse twins** (2026-09-01, by request): Terminal / Agent /
Share buttons in the status bar acting on the directory, and Open / Agent / Share
buttons in the detail panel acting on the cursor entry — each set beside the facts it
acts on, both going through the keyboard handlers.

**M10 is done (2026-09-01).** The HTTP server: in-process `axum` on one thread owned by
a handle whose drop is the stop, loopback by default with the network bind a separate
warned choice, root pinned to the directory current at start. Surfaced as a status-bar
button — `http off` / accent `:8080` — whose contextual menu shows start options when
stopped and the URL, live request log and stop when running. 189 tests, clippy clean.

Verified on screen: started from the menu with Enter, three real `curl`s showed up in
the live log (including a refused escape as 404), the badge pinned the served path
while browsing elsewhere, and killing the app released the port. It also flushed out a
real modal bug — Enter over a field-less overlay navigated the listing underneath it —
now routed to the overlay's confirm.

**M11 is done (2026-09-01), and with it the last planned milestone.** The keymap became
data with `~/.config/omafiles/keymap.toml` merged over it (replacement semantics, broken
files reported rather than fatal), `^k` opens a command palette whose hints show the
*effective* keys, and `contrib/` gained the `.desktop` entry, a `--locked` PKGBUILD, the
`SUPER+SHIFT+F` opt-in snippet and a README for everything that touches `$HOME`. 199
tests, clippy clean, `desktop-file-validate` clean.

Every milestone M0–M11 has shipped. What remains is the recorded leftovers rather than
planned work: word-level diff highlighting (M8), streaming recursive search (M6), the
LAN QR code (M10), SVG-as-picture (M7), ignored-file dimming (M8), and §9's open
questions — most consequentially whether omafiles should take over `SUPER+SHIFT+F` and
`inode/directory` by default, which `contrib/` now makes a one-line decision either way.

**Post-M11: content search, and the path editor rebuilt (2026-09-01).**

- **`^⇧f` — content search**, §6.4's third mode, the one M6 deferred because it shares
  nothing with the name searches but the input field. `crates/omafiles/src/grep.rs`
  shells out to `rg --json` (details now in §6.4's table); results render as
  `path:line` plus the matched line, root pinned at open like the server's, and Enter
  navigates to the file with the cursor on it — opening at the line is an editor's job.
  The debounce plus a generation guard is M3's listing discipline in miniature: a slow
  search for an old query cannot land over a newer one's results.
- **The path editor became a modal panel** (by request; the inline header field with
  its non-interactive suggestion chips is gone). Click the breadcrumb or `^l`: a modal
  prefilled with the current path, completing from the typed path's **descendants** —
  text naming an existing directory offers that directory's children, so the panel
  opens already listing the places one level down before a key is typed; anything else
  completes against the parent filtered by the last component, the way a shell does.
  Suggestions are real rows: `↓`/`↑` move into the list (`None` means Enter takes the
  typed text — arrowing back up past the first row returns there), Enter or a click
  navigates. The header never reflows, and dotfiles appear only once a dot is typed,
  matching the listing's hidden rule.

Verified on screen: the panel opened prefilled and listed the directory's children
unprompted, `↓↓⏎` navigated to the picked child; `^⇧f` then found `needle` in two files
with root-relative `path:line` rows after one debounce, and Enter landed on the hit's
file. 205 tests (6 new for grep, skipping without `rg`), clippy clean.

**Then the small action buttons were uniformized** (2026-09-01, on request). The chrome
had grown four dialects — `main.rs`'s local `icon_button`, bordered `Button`s in the
detail panel, `Row`s doing button work in the status bar, and two hand-rolled badge divs
for git and the server. All of them are now one `omarchy_ui::ActionButton`: glyph, label,
or both; a step below `control-height` (square when glyph-only), corners capped at 2px; idle at the secondary colour
stepping to primary on hover, Row's grammar; hover and pressed fills from `[controls]`;
a disabled state that looks unavailable; `.accent()` for the one badge that is "on"; and
a really subtle idle border — half the control border's alpha — so the verbs have an
edge to sit in without reading as a form. The git badge keeps its emphasis by carrying
the branch and counts as children rather than a label. The detail toolbar wraps tidily
with the expand control holding the corner via an auto margin, and the gallery shows the
component in all four states. Verified across the app on screen, including in the
`ristretto` theme the machine happened to switch to mid-check.

**Files gained their first mutating verbs, and entries a context menu** (2026-09-01, on
request). `fileops.rs` (pure, headless-tested): copy/paste with ` (2)`-numbered
collision targets — a file manager that silently overwrites is a file manager exactly
once — a refused paste-into-itself cycle, half-written copies removed on failure, and
compress-to-zip via `zip -r` with a `bsdtar --format=zip` fallback. `^c` remembers the
entry (and writes its path to the system clipboard as text, since gpui's clipboard
cannot carry a file list), `^v` pastes into the current directory on the background
executor, `z` zips beside the source; the watcher reloads and the remembered
`cursor_name` puts the cursor on what was made. Confirmations use a new quiet notice
level — the urgent red stays for errors. **Right click on a row** opens the entry's
gathered actions in a card at the pointer (clamped to the viewport, menu-surface
tokens); `shift-F10` opens the same menu centred, because the dedicated menu key never
reaches gpui's Wayland backend. Pin appears for directories, Paste-into for directories
while the clipboard holds something.

**The HTTP server grew plural** (same day, on request): `servers: Vec<Handle>`, one per
root, the port conflict resolving itself because only the first gets 8080. The status
bar badge and `^s` menu are now scoped to *the current directory's* server; a globe in
the navigation bar — counting and accented while anything serves — opens the list of
all of them (`^⇧s`), each row carrying copy-URL, go-to-directory and kill. Verified
live: two servers on two roots, both answering curl, the second on an OS-picked port,
and quitting the app released every port.

**Then the servers learned to outlive the window** (2026-09-02, by request): see the
revised bullet in §6.7 — detached `--serve` processes plus an on-disk registry, which
also bought cross-window and cross-restart discovery for free. Verified live: start,
kill the app, `curl` still 200; relaunch, the globe lists the survivor with its hit
count; kill it, the open list sweeps itself to "0 running" and the port is released.

**"Recent" joined the sidebar** (2026-09-02, on request): the tail of the PLACES group,
`^e`, and a palette entry all open a view of the 100 most recently modified files under
home — newest first, `~`-relative middle-truncated parents, ages in the AGE column's
own format, Enter landing on the file in its directory. `recent.rs` walks with the
same `ignore` sweep the recursive search uses (gitignore honoured, hidden trees like
`~/.cache` skipped — they churn constantly and answer nothing) into a bounded min-heap,
so memory stays flat whatever the tree's size; the walk runs on the background executor
behind a "scanning…" state. Home rather than `/`, deliberately: the system tree churns
with upgrades and logs nobody edited. The sidebar row sits outside the keyboard
place-cursor — it opens a view rather than navigating — which is why it has its own
key.

**The help sheet gained a filter, and the modals a proper guard** (2026-09-02, on
request): `?` now opens with a focused search field that filters the shortcut groups
(a group survives if its title matches, else only the entries whose keys or
description do). Chasing the report that keys leaked behind the sheet also surfaced a
real class of holes: field-less modals leave focus on the pane, so listing-context
bindings still resolved — `ctrl-d` scrolled, `backspace` navigated, `t` opened a
terminal, all under an open menu. Every world-mutating verb is now guarded by
`overlay_owns_input`; actions that merely open another modal stay unguarded, since
replacing an overlay is what they are for.

**Search, Recent and content search merged into the Finder** (2026-09-02, on request):
one window — `/`, or a magnifier in the navigation bar — for everything below the
current directory. Empty, it shows the newest files (the old Recent, now scoped to
where you are, so at `~` it behaves exactly as before); typed, fuzzy name matches
first, then files whose *contents* match, deduplicated so a file never appears twice.
One background walk feeds both the recents and the fuzzy corpus, so the whole window
answers from a single corpus with a single truncation note; the content half keeps the
grep debounce and generation guard. Gone with the merge: the sidebar's Recent entry,
`^g` widen, `^⇧f`, `^e`, and the three overlays they addressed — `search.rs`,
`recent.rs` and `grep.rs` stay as the engines under the one view.

**Network locations** (2026-09-02, on request): a NETWORK section below PLACES and
PINNED, appearing the moment one exists (the pinned discipline), with `^⇧n` opening a
one-field dialog for the URI — validated, named by derivation (`media on nas.local`),
persisted to `~/.config/omafiles/network.toml` with `places.toml`'s atomic write.
Built on GVfs rather than anything of ours: `gio mount` does SMB/SFTP/FTP/WebDAV, the
mount surfaces as a FUSE directory under `$XDG_RUNTIME_DIR/gvfs/`, and from there the
listing, preview and finder browse it with no special handling anywhere. Opening a row
navigates straight in when mounted, mounts in the background first when not, and
surfaces gio's own refusal (credentials come from the keyring; interactive auth is the
terminal's job in v1). A right click on the row gathers its actions — Open, Unmount
(disabled while unmounted), Forget — through the same `menu_surround` the entry context
menu uses, extracted for the purpose; unmounting a mount the tab was inside lands the
tab on the nearest surviving ancestor rather than blaming the user with an
unreadable-directory error. Forgetting never unmounts — the mount belongs to GVfs. Not
exercised against a live server here (none reachable); the mount-point resolution and
the list machinery are headless-tested, and the row is mouse-only for now like Recent's
was.

**A narrow window no longer loses the detail panel** (2026-09-04, on request): the
window once opened both panels floating over the listing the moment it went narrow,
stacked, so the first click anywhere shut the top one — and that click wrote to the
same flag the docked layout reads, leaving a wider window with no detail panel and
no control on screen to bring it back. Three changes. The float is its own state
(`Explorer::floating`, one side at most), opened from a strip and dismissed by the
scrim, and dropped on the frame that docks the panels again; the docked intent is
untouched by anything that happens while floating. Narrow mode now starts with
nothing floating and both strips showing. And the detail panel got what the sidebar
always had: a strip one button wide when collapsed (`detail_strip`) and a borderless
collapse at the far right of its bar, so every panel can be closed and reopened with
the mouse. `is_narrow` became `narrow`, read from the width recorded at the top of
the frame, so the handlers between frames and the layout within one agree.

**Three bars, one per panel** (2026-09-04, on request): the navigation bar stopped
spanning the listing and the detail panel together. The listing keeps back, up and the
path; the detail panel got a bar of its own holding the entry's verbs, which left the
foot of the fact sheet — so the sheet now reads cover, then facts, and the verbs sit at
the top where the bar rules align across all three panels. As many verbs as fit show
from the left, estimated from the monospace metrics before layout (`leading_that_fit`),
and a borderless `…` at the right edge opens the rest as a positioned menu
(`Overlay::DetailMenu`) drawing from the same list (`detail_actions`), so the bar and
the menu cannot disagree. The order is therefore a decision: preview, open, copy path,
copy, share first, then the verbs that change something (agent, cut, move, zip, delete,
pin). Preview stays present but disabled where nothing expands, so the verbs keep their
places. The expanded preview's instruction strip became that pane's bar too, at the
shared height and replacing the navigation bar: back where back always is, then the
name and the keys, with the body filling everything below.

**The detail panel became a cover sheet** (2026-09-02, on request): the preview body
rides on top, flush to the panel edges like a cover, a rule between it and the facts;
the info below dropped from panel-padding to row-padding; and the expand control became
an eye with a "Preview" label. The status bar's rhythm collapsed to one number — its
vertical inset now also serves as the horizontal inset and the gap between items.

**The navigation bar adopted the status bar's one-value rhythm, rows lost their corner
radius** (square fills — a rounded fill draws a lozenge in the middle of a column of
names), **and the product got a landing page** in `website/` — a single self-contained
`index.html` whose signature is that it behaves like the app: no colours of its own,
only tokens, with a switcher over five real Omarchy palettes (light included) retinting
the page and a DOM-built replica of the window, live. Fill alphas, hairlines, square
rows and 2px buttons all mirror the app's grammar; `?theme=` deep-links a palette.

**Places open tabs now** (2026-09-02, on request — revising §6.2's "clicking a place
navigates the current tab"): a place click selects the tab already sitting on that
directory, or opens a fresh one there; either way no tab loses where it was. With the
tab list carrying the you-are-here signal, the place rows dropped their active
highlight — a lit place would repeat the tab row above it. The Preview button also
left the toolbar's corner to ride with the other verbs.

**Then the menus went flush** (on request): the modal card carries no padding of its
own — the header, each body child and the hints footer pad themselves — so the
header/content rule and the subtle rules between rows run edge to edge to the card
border, the same discipline the window chrome adopted when its panels lost their
insets. A `modal_inset` helper is what a non-flush body child (an input, a status
line, prose) sits in; flush lists sit bare, their rows already carrying row padding.
The positioned right-click card follows the same structure.

**Menus got their rules, and the server list its polish** (2026-09-01, on request).
`Modal` draws a hairline between its header and its content, so every contextual menu
divides the same way; a new `Separator::subtle()` — half the pane rule's strength —
sits between the rows of every menu list (search, branches, palette, path, grep,
servers, both context-menu surrounds, the workspace and server menus), interleaved by
one `separated()` helper so no list can drift. Each server's description is one line
with the path cut in the middle — the first fifth and the tail survive, so a long root
shows where it starts *and* where it ends — and each server row gained a logs button:
`Overlay::Server` now carries the root it describes, so any server's log screen is one
click from the globe list, not just the current directory's.

Two follow-ups on request: **Pin joined the mouse actions** — in the detail panel when
the cursor entry is a directory, and in the status bar for the directory being looked
at. One button that reads its own state (`Pin` / `Unpin`, disabled on a system place
like Home, which is permanently in the sidebar), toggling through the same `Places`
machinery as `^p`. And **the server badge moved into the status bar's right-corner
action cluster** — it is an action whose label happens to also be its state, so it
belongs with the verbs rather than with the directory facts on the left.
