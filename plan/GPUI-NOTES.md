# gpui notes

Companion to [`PLAN.md`](./PLAN.md). gpui is Zed's internal UI framework, not a
stabilised public library, so `PLAN.md` deliberately avoids hardcoding API details. They
live here.

**Researched 2026-09-01** against `zed-industries/zed` @ `ce48461e` (2026-08-31) and
`longbridge/gpui-component` @ `12054f59` (2026-09-01), plus crates.io, docs.rs, and the
GitHub issue trackers. §1–§7 are verified. §8 is the M0 checklist — those need a running
window on *this* machine, not documentation.

---

## 0. Read this first: crates.io is a trap

In **February 2026** gpui had two breaking structural changes that **crates.io never
received**. Every tutorial, every blog post, and the entire published `gpui` crate
predate them.

| PR | Merged | Change |
| --- | --- | --- |
| [#46758](https://github.com/zed-industries/zed/pull/46758) | 2026-02-13 | `blade-graphics` removed; Linux renderer reimplemented on **wgpu** (`Backends::VULKAN \| GL`). Vulkan is no longer a hard requirement — GL is a real fallback. |
| [#49277](https://github.com/zed-industries/zed/pull/49277) | 2026-02-19 | `gpui` split into 12 crates. **`Application::new()` deleted.** |

`gpui 0.2.2` on crates.io was published **2025-10-22** and is the pre-split monolith
with the Blade renderer. `gpui_platform`, `gpui_linux`, `gpui_wgpu`, `gpui_web` and
`gpui_shared_string` **are not published at all**, so the official README's
`gpui_platform = { version = "*" }` snippet does not work today.

Why it froze: a Zed team member said on HN (~Feb 2026) that *"GPUI development is
getting some major brakes put on it. We gotta focus on some business relevant work in
2026."* That is about **stewardship** — docs, publishing, stability guarantees — not the
code. `gpui_wgpu` alone has 48+ commits with the newest on 2026-08-25; Zed shipped 1.0
on 2026-04-29 and is at v1.17.2.

**Do not use `create-gpui-app`** — last published 2025-03-17, generates the old API.

**`gpui-ce`** is a community fork spawned by the "brakes" announcement, but its
getting-started still shows `Application::new().run(…)`, so it forked *pre-split*.
Uncertain how far it has diverged — its docs landing page and crates.io metadata were
checked, the source was not audited. Not recommended.

---

## 1. Dependency — the recommended `Cargo.toml`

```toml
[workspace.dependencies]
# gpui floats; Cargo.lock is the pin. See "the unification trap" below.
gpui          = { git = "https://github.com/zed-industries/zed" }
gpui_platform = { git = "https://github.com/zed-industries/zed", features = ["wayland"] }

# Different repo, so no unification problem — rev-pinned deliberately.
gpui-component        = { git = "https://github.com/longbridge/gpui-component", rev = "5cb0946" }
gpui-component-assets = { git = "https://github.com/longbridge/gpui-component", rev = "5cb0946" }
```

### ⚠️ The unification trap — corrected 2026-09-01

**An earlier version of this file said "Cargo unifies git sources, so our pin wins for
gpui-component too." That is wrong, and building it disproved it.**

gpui-component declares `gpui = { version = "0.2.2", git = ".../zed" }` with **no rev**.
Cargo treats a rev-pinned git source and a floating one as **different sources** and does
not unify them. Pinning our side therefore puts *two* copies of gpui in one dependency
graph, and the build dies with a wall of:

```
error[E0308]: mismatched types
   expected `gpui::app::App`, found `App`
   note: there are multiple different versions of crate `gpui` in the dependency graph
```

That is upstream [#2532](https://github.com/longbridge/gpui-component/issues/2532),
reproduced here rather than taken on trust.

Two things that look like they should fix it and do not:

- **gpui-component's committed `Cargo.lock`.** It pins a known-good zed rev, but a
  lockfile is not inherited by downstream crates. Ours resolved zed to `a66fb6a` while
  gpui-component's lock said `f66ed399`. Worth stating plainly, because the lockfile is
  exactly the artefact you go looking for when you hit this.
- **`[patch."https://github.com/zed-industries/zed"]`.** Cargo rejects it outright:
  `patch for 'gpui' points to the same source, but patches must point to different
  sources`.

**What works:** let gpui float so both sides resolve to one source, and let the committed
`Cargo.lock` do the pinning. For an application that is reproducible; the tradeoff is
that `cargo update -p gpui` becomes a deliberate, re-verify-M0 event rather than routine
hygiene.

**Verified 2026-09-01:** with that arrangement the whole workspace — M0's spike, M1's
tokens and watcher, M2's kit — compiles and passes 69 tests against zed `a66fb6a`, six
days newer than the rev M0 was originally verified on.

The M0 re-verification that a gpui bump is supposed to trigger was then actually run, not
just written down: the gallery opens on `a66fb6a`, maps under Hyprland with the right
`app_id`, renders the palette, the five states, the type scale and the rows, and exits
with an empty stderr. So the API surface M0–M2 depends on is unchanged across those six
days. That is one data point, not a trend — the discipline still stands.

### ⚠️ The Wayland feature flag is mandatory

Without a windowing feature, `gpui_linux::current_platform` hits `unreachable!()` and the
app crashes at startup with no useful message. This was gpui-component issue #2315.


### Churn to expect

`dependabot.yml` bumps gpui **weekly** — 71 "bump gpui" PRs to date. Hard breaks land
roughly monthly: #2550 (`AnyView::into_any()` removed), #2403 (`BoxShadow` inset — broke
the build within **4 hours**), #2064 (the crate split). Bump deliberately, never on a
floating rev.

### If we skip gpui-component

[`gpui-unofficial`](https://github.com/iamnbutler/gpui-unofficial) is a clean crates.io
path to the current API — automated republish on every Zed release tag by `iamnbutler` (a
Zed designer), Apache-2.0, polled every 6h. Its `[lib] name` is `gpui`, so `use gpui::…`
works unmodified:

```toml
gpui = { package = "gpui-unofficial", version = "=1.17.2" }
gpui_platform = { package = "gpui-platform-gpui-unofficial", version = "=1.17.2",
                  features = ["wayland", "x11"] }
```

Pin exactly (`=`) — versions are verbatim Zed semver and the author documents that no fix
can be published for an already-released version.

**docs.rs built it, which makes it the only current, browsable gpui API reference in
existence: <https://docs.rs/gpui-unofficial/latest/gpui/>** (94.9% documented).

### Residual risk

Zed's workspace has a `[patch.crates-io]` redirecting `calloop` → a Zed fork and
`async-task` → a smol-rs rev. **Git dependents do not inherit `[patch]`**, so we build
`gpui_linux` against upstream `calloop 0.14.3`. gpui-component does exactly this and
works, so it is evidently fine — but if we hit odd event-loop behaviour, replicate them.

### Bootstrap

```rust
fn main() {
    let app = gpui_platform::application().with_assets(gpui_component_assets::Assets);
    app.run(move |cx| {
        gpui_component::init(cx);            // MUST be first inside run()
        cx.spawn(async move |cx| {
            cx.open_window(WindowOptions::default(), |window, cx| {
                let view = cx.new(|_| FileExplorer::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))   // Root hosts modal/notification/menu layers
            }).expect("failed to open window");
        }).detach();
    });
}
```

---

## 2. Linux / Wayland status

**The Wayland backend is in substantially better shape than X11** — nearly all open IME
bugs are X11/XIM. Bound protocols include `xdg_wm_base`, `zwp_text_input_v3`,
`wp_fractional_scale_v1`, `wp_viewporter`, `zxdg_decoration_manager_v1`,
`wp_cursor_shape_v1`, `wl_data_device_manager`, `zwlr_layer_shell_v1`,
`xdg_activation_v1`, `zwp_pointer_gestures_v1`.

| Area | Status |
| --- | --- |
| **IME** | `zwp_text_input_v3` properly wired. **Open: [#60964](https://github.com/zed-industries/zed/issues/60964)** — dead keys and compose broken on Linux/Wayland, a regression, untriaged. gpui-component has **zero** Linux IME issues filed, meaning fcitx5 on Hyprland is untested by anyone upstream. **Test day one.** |
| **Fractional scaling** | Implemented; blurriness fixed 2025-06. Residual: [#56294](https://github.com/zed-industries/zed/issues/56294) (1px shift during interactive resize), [PR #58106](https://github.com/zed-industries/zed/pull/58106) (multi-output fractional geometry). |
| **CSD / tiling** | `WindowOptions::window_decorations` defaults to `Client`. `xdg_toplevel::State::Tiled*` is parsed and `Tiling::tiled()` suppresses CSD resize edges and rounding — tiling is genuinely handled. But [PR #63285](https://github.com/zed-industries/zed/pull/63285) (open) notes *"Hyprland doesn't even support resizing a floating window with `xdg_surface::set_window_geometry`, it only supports resizing via setting min/max size."* |
| **Blur / transparency** | `Blurred` is gated on `org_kde_kwin_blur_manager` — **KDE only**. Confirmed absent from the Hyprland 0.56.2 binary on this machine, so `Blurred` is a **silent no-op**. But `set_opaque_region(None)` is still called, so use `Transparent` + a Hyprland windowrule and the compositor's own blur applies. Two competing PRs ([#53746](https://github.com/zed-industries/zed/pull/53746), [#59842](https://github.com/zed-industries/zed/pull/59842)) would replace this with `ext-background-effect`, motivated by KDE removing the protocol in Plasma 6.7. |
| **Cursor themes** | `wp_cursor_shape_v1` implemented — the compositor owns cursor appearance. The fallback path reads theme/size from **xdg-desktop-portal Settings**, not `XCURSOR_*` env vars. |
| **Clipboard** | CLIPBOARD + PRIMARY both implemented. Several open bugs ([#60823](https://github.com/zed-industries/zed/issues/60823) selection storm, [#47004](https://github.com/zed-industries/zed/issues/47004), [#53446](https://github.com/zed-industries/zed/issues/53446)). |
| **Drag & drop** | Inbound file drop works. **Outbound** external drag landed [#61947](https://github.com/zed-industries/zed/pull/61947), merged 2026-07-31. Limitation: [#62790](https://github.com/zed-industries/zed/issues/62790) — outbound offers only `text/uri-list`, so Flatpak drop targets can't open the file (needs the XDG FileTransfer portal). |
| **Renderer** | [#63358](https://github.com/zed-industries/zed/issues/63358) open — intermittent horizontal artifacts on AMD/RADV **Navi31** + Mesa 26.2.1. This machine is Renoir (Vega iGPU) on the same Mesa, so it may or may not apply. |

### Arch dependencies

A bare gpui app needs far less than Zed's full build:

```
wayland libxkbcommon fontconfig freetype2 vulkan-icd-loader mesa pkgconf
libxcb libxkbcommon-x11          # only with the x11 feature
xdg-desktop-portal-hyprland      # file dialogs + cursor/theme settings (gpui uses ashpd)
+ a Vulkan driver: vulkan-radeon | vulkan-intel | nvidia-utils
```

**All present on this machine.** Rust is the only gap — see `PLAN.md` §11.

Useful runtime env vars: `ZED_DEVICE_ID=0x<hex>` (force GPU), `MESA_VK_DEVICE_SELECT`,
`ZED_LOG=wgpu=info`, `ZED_FONTS_GAMMA` (default 1.8).

---

## 3. Current API — the idioms that changed

### A tutorial is out of date if it shows any of these

`Application::new()` · `ViewContext` / `WindowContext` · `Model<T>` / `View<T>` ·
`cx.spawn(|this, mut cx| async move {…})` · `uniform_list(view, id, n, f)` ·
`#[gpui::action]` · `render(&mut self, cx: &mut ViewContext<Self>)` · any mention of
`blade-graphics` · a `gpui` git dep with no `gpui_platform`.

### Entry

```rust
gpui_platform::application().run(|cx: &mut App| { … });   // Application::new() is gone
```

### Contexts and entities

`ViewContext`, `WindowContext`, `Model<T>` and `View<T>` all return **zero hits** when
grepping `crates/gpui/src`. They are gone.

- `App` — root context, owns all entity data.
- `Context<'a, T>` — derefs to `App`. `cx.notify()`, `cx.emit()`, `cx.spawn()`,
  `cx.listener()`, `cx.processor()`, `cx.observe()`, `cx.subscribe()`.
- `Entity<T>` — the handle. A "view" is just an `Entity<T> where T: Render`.
- `Window` — **not a context**; passed alongside `&mut App` / `&mut Context<T>`.

Every callback is now `(…, &mut Window, &mut App)` or `(…, &mut Window, &mut Context<T>)`.

### Globals — no automatic invalidation

This is the one that matters most for the theme design in `PLAN.md` §5.

```rust
impl Global for Theme {}                     // marker trait, no methods

cx.global::<Theme>()                         // &G   — read, does NOT notify
cx.global_mut::<Theme>()                     // &mut G — DOES notify
cx.update_global::<Theme, _>(|t, cx| { … })
```

`global_mut`, `default_global`, `set_global` and `remove_global` each push
`Effect::NotifyGlobalObservers`; the read-only accessors do not. **But views do not
auto re-render** — there is no implicit dependency tracking. Subscribe explicitly, and
**hold the `Subscription`**; dropping it silently unsubscribes:

```rust
struct FileList { _theme: Subscription, /* … */ }
let sub = cx.observe_global::<Theme>(|_this, cx| cx.notify());
```

(Doc bug worth knowing: `remove_global`'s comment says *"Does not notify global
observers"* but the code pushes the effect. Code wins.)

### Actions and key contexts

```rust
actions!(omafiles, [GoUp, Rename, ToggleHidden]);   // namespace required

cx.bind_keys([
    KeyBinding::new("backspace", GoUp, Some("FileList")),   // 3rd arg = context predicate
]);

div().key_context("FileList")
     .track_focus(&self.focus_handle)          // context applies only while focused
     .on_action(cx.listener(|this, _: &GoUp, window, cx| { … }))
```

Predicates support boolean expressions, so `Some("FileList && !editing")` works — which
is exactly what `PLAN.md` §6.8 needs to disambiguate `/`. `KeyBinding::new` **panics** on
a parse error; `KeyBinding::load` returns `Result`.

⚠️ `crates/gpui/docs/key_dispatch.md` shows a `#[gpui::action]` **attribute** macro. **It
does not exist** — the only derives are `Action, IntoElement, Render, AppContext,
VisualContext`. The doc is stale.

### `uniform_list`

```rust
uniform_list("entries", self.entries.len(),
    cx.processor(|this, range: Range<usize>, _window, _cx| {
        range.map(|ix| div().id(ix).child(this.entries[ix].name.clone())).collect()
    }))
    .track_scroll(&self.scroll_handle)
```

Old tutorials pass a view as the first arg — wrong now. For **variable-height** rows use
`list(ListState, |ix, window, cx| -> AnyElement)`.

### `img()` and `svg()` — the distinction decides the icon strategy

`img()` accepts `&str`/`String` (URL → fetch, else embedded asset), `&Path`/`PathBuf`,
`Arc<RenderImage>`, `Arc<Image>`, or a closure. Formats include avif, jpg, png, gif,
webp, tiff, bmp, ico, hdr, exr, qoi and **svg**. **Animated GIF and WebP play
automatically** — better than the first-frame fallback `PLAN.md` §6.5 assumed.

- `svg().path(…)` renders an **alpha mask tinted by `style.text.color`** — monochrome,
  and paints **nothing** if no text colour is set. For UI glyphs.
- `img("/usr/share/icons/…/folder.svg")` goes through resvg → **full colour**. For
  Freedesktop icon-theme icons. Rasterises at scale factor 1.0, so expect softness when
  upscaled on HiDPI.

### Async — closure shape changed

```rust
cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
    let entries = cx.background_spawn(async move { read_dir_blocking(path) }).await?;
    this.update(cx, |this, cx| { this.entries = entries; cx.notify(); })
}).detach();
```

Also `cx.background_spawn`, `cx.spawn_in(window, …)`, `cx.background_executor()`.
Executor is smol-based; `gpui_tokio` exists as a bridge.

### Styling

Layout is **taffy 0.13** — flexbox **and CSS grid** (`grid_cols`, `col_start`, …), plus
`text_ellipsis`, `line_clamp`, `truncate`, and `container_query()`.

---

## 4. What gpui does NOT provide

Bare gpui has **13 elements total**: `anchored, animation, canvas, container_query,
deferred, div, image_cache, img, list, surface, svg, text, uniform_list`. That is the
entire widget set.

| Need | Bare gpui | gpui-component |
| --- | --- | --- |
| **Text input** | ❌ trait + a 784-line example to copy | ✅ `input::Input` — rope-backed, multi-line, selection, clipboard, undo/redo, word motions, IME |
| **Scrollbar** | ❌ **none exists.** `scrollbar_width()` only reserves taffy space; nothing is painted | ✅ `scroll::{Scrollbar, Scrollable, ScrollbarAxis, …}` |
| **Context menu** | ❌ primitives only (`anchored`, `deferred`, `occlude`) | ✅ `menu::{ContextMenu, PopupMenu, DropdownMenu}` |
| **File icons** | ❌ | ⚠️ `IconName` is 88 **Lucide UI glyphs**, not a file-type set. Use `freedesktop-icons` + `img()`. |
| **Video** | ❌ `surface()` is `#[cfg(target_os = "macos")]` **only** | ❌ |
| **Tree, table, tabs, resizable panes, dialogs, toasts, breadcrumbs, sidebar** | ❌ | ✅ |
| **Markdown / syntax highlighting** | ❌ | ✅ `text_view` (uses the `markdown` crate, not pulldown-cmark) + `highlighter` |
| **Drag & drop** | ✅ internal + external, both directions | internal only |

**These three gaps — text input, scrollbar, context menu — are why `PLAN.md` leans
toward adopting gpui-component** despite everything in §1.

### Platform services already on `App` (no crate needed)

`reveal_path`, `open_with_system` (shells `xdg-open`), `prompt_for_paths` /
`prompt_for_new_path` (via xdg-desktop-portal / `ashpd`), clipboard read/write including
async, `open_url`, `add_recent_document`, `set_menus`, `window_appearance`,
`keyboard_layout`. On `Window`: `set_app_id`, `start_window_move`,
`start_window_resize`, `request_decorations`, `set_background_appearance`,
`set_cursor_style`, `defer`, `on_next_frame`. Plus AccessKit a11y, `notify-rust`
notifications and an `inspector` feature.

**So `rfd`, `arboard` and `open` are all unnecessary.**

### Native popups worth knowing about

`WindowKind::AnchoredPopup(PopupOptions{ anchor_rect, gravity, constraint_adjustment,
grab, … })` maps onto `xdg_positioner`. With `grab: true` it behaves as a real menu —
takes keyboard focus, dismisses on outside click, and **can extend beyond the parent
window**. Also `WindowKind::LayerShell` via `zwlr_layer_shell_v1`.

---

## 5. Fonts — resolved, and favourable

**fontconfig-backed, nothing bundled, swappable at runtime.** Since the wgpu rewrite the
Linux text system is `gpui_wgpu/src/cosmic_text_system.rs`: **cosmic-text 0.19** + swash
+ a Zed fork of font-kit. `FontSystem::new()` loads system fonts via fontdb →
fontconfig. On native, the examples' `load_fonts` is a **no-op**.

```rust
cx.text_system().all_font_names() -> Vec<String>     // enumerate installed families
cx.text_system().add_fonts(…)                        // register embedded fonts
```

**Loading by family name works, and a live swap needs no window recreation.** Because
the family is ordinary element state, changing it and calling `cx.notify()` re-shapes on
the next frame. This closes the open risk in `PLAN.md` §10 — `omarchy-font-set` changes
propagate through the same subscription path as a colour change.

Tuning: `ZED_FONTS_GAMMA` (1.0–2.2, default 1.8),
`ZED_FONTS_GRAYSCALE_ENHANCED_CONTRAST`.

---

## 6. Ecosystem versions to pin

Verified against crates.io 2026-09-01.

| Need | Crate | Version | Note |
| --- | --- | --- | --- |
| Fuzzy match | `nucleo` | **0.5.0** | **Zed itself uses `nucleo = "0.5"`.** Confirms `PLAN.md` §6.4 — no reason to shell out to fzf. |
| FS watch | `notify` | **8.2.0** | ⚠️ zed `[patch]`es this to a fork and gpui-component pins **7.0.0** — watch for a duplicate-version graph in `omarchy-tokens` |
| | `notify-debouncer-full` | **0.7.0** | Want this, not raw `notify` |
| Traversal | `ignore` | **0.4.33** | Parallel walker + gitignore; prefer over `walkdir` |
| Content search | `grep-searcher` / `grep-matcher` / `grep-regex` | 0.1.17 / 0.1.9 / 0.1.14 | ripgrep's engine as a library |
| Images | `image` | **0.25.10** | gpui already depends on `^0.25` — **match it or you get two copies** |
| Markdown | `pulldown-cmark` | **0.13.4** | Note gpui-component's `text_view` uses `markdown 1.0` instead |
| Syntax | `tree-sitter` + `-highlight` | **0.27.0** | ⚠️ gpui-component pins `^0.25.4`; zed `[patch]`es `tree-sitter-language`; grammars are ABI-sensitive. Prefer gpui-component's own `tree-sitter-languages` feature (35 grammars) over hand-assembling. |
| | `syntect` | **5.3.0** | The no-ABI-minefield fallback; pair with `two-face 0.5.2` for bat's syntax/theme set |
| HTTP server | `axum` + `tower-http` | **0.8.9** + **0.7.1** | `ServeDir` — confirms `PLAN.md` §6.7 |
| Safe delete | `trash` | **5.2.6** | XDG trash spec |
| Config | `toml` | **1.1.4** | 1.x now; `toml_edit 0.25` for format-preserving writes |
| **File icons** | `freedesktop-icons` | **0.4.0** | The right tool; pair with `img(path)` |
| "Open With" | `freedesktop-desktop-entry` | 0.8.2 | |
| MIME | `mime_guess` 2.0.5 · `tree_magic_mini` 3.2.2 · `infer` 0.22.0 | | |
| Video thumbnails | `ffmpeg-next` 9.0.0 · `video-rs` 0.11.0 | | **Best answer for a file explorer** |
| Video playback | `gstreamer` 0.25.3 | | Most viable, but glue is ours to write |
| **Unnecessary** | ~~`rfd`~~ ~~`arboard`~~ ~~`open`~~ | | gpui covers all three natively via portals |

**Video reality check.** There is **no zero-copy path on Linux** — `surface()` is
macOS-only. Ranked: (1) GStreamer `appsink` → CPU convert → `RenderImage`; (2) `libmpv2`
with render-context readback; (3) a separate mpv window — but note `mpv --wid=`
**cannot work on Wayland**, there is no window embedding. Thumbnails-only via
`ffmpeg-next` is by far the cheapest answer, which is what `PLAN.md` §6.5 already plans.

---

## 7. Prior art: Ferail

**<https://github.com/jonx/Ferail>** — a desktop file manager on gpui + gpui-component,
and as far as can be found, the only shipping one. (Located via search; its upstream
issue filings were read, its source was not audited.)

**What it proves:** the full file-manager shell on this stack is real — virtualised
listing, tree sidebar, resizable panes, tabs, context menus, theming. Nobody has to
prove the concept.

**Where it hit walls.** On 2026-08-21 the author filed three precise gap reports. All
**open, all zero comments**:

| Issue | Problem |
| --- | --- |
| [#2795](https://github.com/longbridge/gpui-component/issues/2795) | `TableEvent::SelectRow` carries no `Modifiers` — kills ctrl-click toggle, shift-click range, drag-delay on selected, rubber-band select. Ferail's answer was to **ship a full local fork of the table + virtual list**. |
| [#2796](https://github.com/longbridge/gpui-component/issues/2796) | `context_menu` has no `on_open`/`on_close` and no queryable open state |
| [#2797](https://github.com/longbridge/gpui-component/issues/2797) | `context_menu`'s builder runs **once per open**, so an open menu can never be rebuilt — an async-populated "Open With" submenu is impossible without forking |
| [#2266](https://github.com/longbridge/gpui-component/issues/2266) | `Tree` hard-overrides `ListItem::selected` from a single `selected_ix`, so Tree multi-selection also requires forking |

**This is confirmed cost, not speculative risk.** It also validates `PLAN.md`'s instinct
to build our own list and `InteractiveSurface` rather than inherit gpui-component's —
multi-select with modifiers is a core file-explorer interaction and we cannot take it
broken.

### Maintainer responsiveness

**45 of 76 open issues have zero comments; 23 of the 30 most recent have zero.** No issue
labels in use, effectively two people with merge rights. But *build-breakers* get
same-day fixes. Translation: **crashes get fixed fast, API-gap requests are silently
ignored.** Plan to fork, not to file.

Other landmines: [#2527](https://github.com/longbridge/gpui-component/issues/2527)
(open, 0 comments) local images fail to render on Linux x86_64 while still occupying
layout — **reproduce before committing to a thumbnail pane**. #2733/#2734 deliberately
hardened `TextView` so image sources **cannot read arbitrary local paths** — check it
doesn't block thumbnails. #2759 "Out of memory" (open, 0 comments). #1528 (closed):
right-clicking a `TitleBar` on XFCE/X11 froze the entire session's input.

Licensing footnote: [#1392](https://github.com/longbridge/gpui-component/issues/1392) —
the root `LICENSE-APACHE` isn't in the crates.io tarball, but the SPDX `license` field
*is* set in every manifest, which is what `cargo-deny` reads. Cosmetic; whitelist the
path in CI.

---

## 8. M0 results — **gpui is a go**

Run on 2026-09-01 against the pinned revs in §1, on this machine (Hyprland 0.56.2,
AMD Renoir, RADV / Mesa 26.2.1, Rust 1.97.1). The harness is `crates/omafiles`, which
puts every check on screen at once; `cargo run -p omafiles`, then `r` to reload the
theme.

### Blockers — all clear

| Check | Result |
| --- | --- |
| Window opens and renders on Hyprland + RADV | ✅ Mapped, tiled, no errors on stderr |
| Rendering artifacts ([#63358](https://github.com/zed-industries/zed/issues/63358)) | ✅ None. That report is Navi31 discrete; this is Renoir integrated, same Mesa 26.2.1 |
| **Local images render** ([gpui-component #2527](https://github.com/longbridge/gpui-component/issues/2527)) | ✅ **Does not reproduce.** `img(PathBuf)` on bare gpui renders a local PNG correctly. #2527 is a gpui-component issue, not a gpui one — the preview pane is unblocked |
| Nerd Font glyphs at icon sizes | ✅ All eight test glyphs render in JetBrainsMono Nerd Font |
| Transparency + Hyprland blur | ✅ `WindowBackgroundAppearance::Transparent` + a translucent root background; Hyprland's own `decoration:blur` (enabled, size 6) blurs behind it, as predicted in §2 |
| `cx.observe_global::<Theme>` re-renders on a live theme switch | ✅ Verified across `tokyo-night` → `retro-82` → `white`; the whole UI retints |
| Light mode | ✅ `white` renders correctly, and all four state washes stay distinguishable on a pure-white background |

### Findings that changed the code

**1. `app_id` must be set explicitly, or Hyprland sees nothing.** With
`WindowOptions::default()`, `hyprctl clients -j` reports `class=""` and `title=""` — no
windowrule can match, and the window is anonymous in the taskbar. Fixed by setting
`app_id` and `titlebar.title`:

```rust
WindowOptions {
    app_id: Some("dev.omarchy.omafiles".to_string()),
    titlebar: Some(TitlebarOptions { title: Some("…".into()), ..Default::default() }),
    window_decorations: Some(WindowDecorations::Server),   // gpui defaults to Client
    window_background: WindowBackgroundAppearance::Transparent,
    ..Default::default()
}
```

**2. `WindowDecorations` defaults to `Client`.** Set `Server` so the compositor owns
decorations — a CSD titlebar has no business inside a tiling layout.

**3. gpui caches images by path, ignoring content.** After a theme switch,
`current/theme/preview.png` is a different file at the same path, and gpui kept showing
the old one. `PLAN.md` §6.5 already specifies caching by `(path, mtime, size)` — this is
the evidence that it is required, not defensive.

**4. Three stock themes omit `orange` and `brown`.** `white`, `solitude` and
`last-horizon`. A struct requiring every key fails to load them, and the app silently
fell back to a built-in palette. Fixed by porting `omarchy-theme-color`'s real
derivation chain; see `PLAN.md` §2.2c.

### Still open

| Check | Status |
| --- | --- |
| fcitx5 / ibus composition, dead keys ([#60964](https://github.com/zed-industries/zed/issues/60964)) | ⏳ Needs an IME configured; no text input in the harness yet. Do it in M5. |
| Key contexts disambiguating `/` | ⏳ M3, when there is a list and a field to disambiguate between |
| Floating-window resize ([#63285](https://github.com/zed-industries/zed/pull/63285)) | ⏳ Only tested tiled |
| Fractional scaling | ⏳ This display is integer-scaled |
| Live **font-family** swap | ⏳ Palette reload is proven and the watcher covers `~/.config/fontconfig`; an actual `omarchy font set` is still unexercised |

### Build cost

| | |
| --- | --- |
| Cold `cargo check` (incl. git fetch) | ~10 min |
| Cold `cargo build` after check | 5m 46s |
| Incremental rebuild of `omafiles` only | ~4 s |
| Debug binary | 652 MB |
| `target/` | 4.8 GB |
| Zed git checkout in `~/.cargo/git` | 481 MB |
| Dependencies compiled | 955 |

CI needs a warm cargo cache or it will spend ten minutes on every run. Release-profile
size is not yet measured.

### Decision log

| Date | Question | Answer |
| --- | --- | --- |
| 2026-09-01 | Toolchain | Rust **1.97.1** via mise; `rust-toolchain.toml` is honoured (mise's rust is rustup-backed) |
| 2026-09-01 | gpui source | Git rev pins, **not** crates.io. `zed@f66ed399` verified to build |
| 2026-09-01 | Adopt `gpui-component`? | **Not yet.** M0 proved bare gpui works, and #2527 turned out to be gpui-component's bug, not gpui's. Defer to M3, where the real cost (text input, scrollbar, context menu) is felt. Nothing so far requires it. |
| 2026-09-01 | Syntax highlighting | Deferred with the above — it is downstream of the gpui-component decision |
| 2026-09-01 | How does the watcher reach gpui? | A dedicated OS thread (blocking FS IO should not occupy a pool thread) pushing through `futures::channel::mpsc` onto `cx.spawn`. Push-based — no polling timer. `AsyncApp::update` is infallible in this rev, and the detached task's lifetime is the app's, so channel closure is what stops the thread. |

---

## 9. The M3 decision: adopt `gpui-component`, keep `omarchy-ui` as the theme

**Decided 2026-09-01, and verified by building it.**

### Why not Zed's own crates

The obvious "reuse well-known components" answer is Zed's `ui` crate — 28,571 lines with
exactly what a file explorer needs (`scrollbar.rs` 1,722 lines, `context_menu.rs` 2,534,
plus `tab`, `tab_bar`, `disclosure`, `indent_guides`, `navigable`, `keybinding`). It is
also not entangled with `editor`/`project`; it depends only on `gpui`, `theme`, `icons`,
`menu`, `component` and macros.

**But every Zed crate above gpui is `GPL-3.0-or-later`.** Verified by reading the
manifests directly:

| Crate | License |
| --- | --- |
| `gpui`, `gpui_platform` | Apache-2.0 |
| `ui`, `ui_input`, `ui_macros`, `ui_prompt` | GPL-3.0-or-later |
| `picker`, `component`, `menu`, `theme`, `icons` | GPL-3.0-or-later |
| `workspace`, `project_panel`, `file_finder`, `editor` | GPL-3.0-or-later |

They are also `publish = false`, so they are git-only. Adopting them would relicense
omafiles from MIT to GPL-3.0-or-later — a decision with consequences for packaging and
for ever upstreaming into Omarchy, and not one to make for convenience.

There is a second, subtler problem: Zed's `ui` components read colour from Zed's `theme`
crate through 38 distinct `cx.theme().colors().*` keys. Adopting them means either
adopting Zed's theme model or writing an adapter that impersonates it — which inverts the
architecture of a project whose entire premise is that Omarchy is the source of truth.

### Why `gpui-component`

Apache-2.0, actively developed, and it has the three things bare gpui lacks and a file
explorer cannot do without: **text input, a visible scrollbar, and a context menu**
(§4). Also `table`, `tree`, `virtual_list`, `dock`, `sidebar`, `breadcrumb`, `dialog`.

The cost is the pinning arrangement in §1, which is real but bounded and now understood.

### The architecture: one source of truth, two consumers

gpui-component ships its own theme, and a widget kit painting its own colours beside
`omarchy-ui` painting Omarchy's would be a Frankenstein. The resolution is that
gpui-component's `Theme` is a gpui **`Global`** (`impl Global for Theme`, with
`global_mut` and `sync_base`), so `omarchy-ui` can simply drive it —
`crates/omarchy-ui/src/interop.rs`:

```
Omarchy tokens ──> omarchy_ui::Theme        (our components read this)
               └─> gpui_component::Theme    (their components read this)
```

`ThemeColor` has 135 fields, but they are semantic (`list_hover`, `scrollbar_thumb`,
`button_danger_active`) and derive from Omarchy's ~10 roles plus the `[controls]` state
alphas. There is no need to fill all 135 — start from the mode-appropriate default and
override what carries the theme's identity.

Two things the mapping has to get right, both non-obvious:

- **`sync_base` is not optional.** gpui-component mirrors radius, colours and fonts into
  a second "base" global that the scrollbar and resize handles read. Mutating the theme
  without syncing leaves those on the old palette — a *partial* retint, which is worse
  than none because most widgets look right.
- **Their fields are opaque `Hsla`; Omarchy's states are alpha washes.** The compositing
  has to happen in the bridge, not at paint time, or every state fill arrives transparent
  over nothing.

The contrast floor (§2.2d in `PLAN.md`) applies here too: `muted_foreground` is what most
of their secondary labels use, and handing it Omarchy's raw `dark_foreground` would make
those labels invisible on the `white` theme.

### Still to decide, at the first real call site

Whether to take gpui-component's `table`/`list` or keep `omarchy_ui::Row`. M2 already
built `Row` on `InteractiveSurface`, and Ferail had to vendor 8 files to get
modifier-aware multi-select out of gpui-component's table — so the current lean is to keep
our own row and take gpui-component for input, scrollbar and context menu only. Decide it
in M3 with the listing in front of us, not now.
