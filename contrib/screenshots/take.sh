#!/bin/bash
# Take the showcase screenshots of omafiles, on this Hyprland session.
#
#   contrib/screenshots/take.sh                       # current theme, every shot
#   contrib/screenshots/take.sh --theme "Tokyo Night" --theme "Catppuccin Latte"
#   contrib/screenshots/take.sh --theme all           # every stock theme
#   contrib/screenshots/take.sh --only overview,search --margin 0
#   contrib/screenshots/take.sh --list                # the shot names
#
# The app runs in a throwaway home (see fixture.sh): its own pins, session,
# network locations and servers, and not one real file. Themes other than the
# current one are staged the way `omarchy-theme-set` stages them, under an
# `OMARCHY_ROOT` the app alone sees — your desktop does not change. The window
# is floated on an otherwise empty Hyprland workspace, sized, and driven with
# wtype; grim captures the window plus a margin of wallpaper.
#
# Output: <out>/<theme>/<shot>.png, at the monitor's real pixel scale, and
# <out>/themes.png, a grid of the overview across themes, when more than one
# theme was taken.
#
# Needs: hyprctl, grim, wtype, jq, magick, git, python3; ffmpeg is optional
# (without it there is no video to preview). Runs the release binary from
# target/, building it first when missing.

set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
REAL_HOME=$HOME
OMARCHY_PATH=${OMARCHY_PATH:-/usr/share/omarchy}

# shellcheck source=fixture.sh
source "$HERE/fixture.sh"

OUT="$ROOT/docs/screenshots"
THEMES=()
SIZE="1440x900"
MARGIN=48
ONLY=""
BIN=""
KEEP=0
SETTLE=0.6

SHOTS=(
  overview
  git-diff git-diff-expanded git-branches
  preview-code preview-code-expanded
  preview-markdown-expanded preview-image preview-image-expanded preview-video
  search-recent search
  palette help
  server-start server-running server-list
  workspace-new context-menu path-edit network-add agent-prompt
  copy-image delete
  sidebar-collapsed listing-only
)

usage() { sed -n '2,24p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
log() { printf '\033[1m%s\033[0m\n' "$*" >&2; }
die() { printf 'take.sh: %s\n' "$*" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
  case $1 in
    --out) OUT=$2; shift 2 ;;
    --theme) THEMES+=("$2"); shift 2 ;;
    --size) SIZE=$2; shift 2 ;;
    --margin) MARGIN=$2; shift 2 ;;
    --only) ONLY=$2; shift 2 ;;
    --bin) BIN=$2; shift 2 ;;
    --keep) KEEP=1; shift ;;
    --list) printf '%s\n' "${SHOTS[@]}"; exit 0 ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1 (try --help)" ;;
  esac
done

[[ ${#THEMES[@]} -eq 0 ]] && THEMES=(current)
[[ -n ${HYPRLAND_INSTANCE_SIGNATURE:-} ]] || die "not inside a Hyprland session"
for tool in hyprctl grim wtype jq magick git python3; do
  command -v "$tool" >/dev/null || die "$tool is not installed"
done

if [[ -z $BIN ]]; then
  BIN="$ROOT/target/release/omafiles"
  if [[ ! -x $BIN ]]; then
    log "No release binary, building one (this is the slow gpui build)"
    (cd "$ROOT" && cargo build --release --locked -p omafiles)
  fi
fi
[[ -x $BIN ]] || die "$BIN is not executable"

WIDTH=${SIZE%x*}
HEIGHT=${SIZE#*x}

# ---------------------------------------------------------------------------
# Themes

# "Tokyo Night" and "tokyo-night" both name the same directory.
slug() { printf '%s' "$1" | tr '[:upper:] ' '[:lower:]-'; }

theme_source() {
  local s; s=$(slug "$1")
  if [[ -d "$REAL_HOME/.config/omarchy/themes/$s" ]]; then
    printf '%s' "$REAL_HOME/.config/omarchy/themes/$s"
  elif [[ -d "$OMARCHY_PATH/themes/$s" ]]; then
    printf '%s' "$OMARCHY_PATH/themes/$s"
  else
    return 1
  fi
}

all_themes() {
  find "$OMARCHY_PATH/themes" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | sort
}

# `{{ mix a b 34% }}`: 34 parts of b into a, per channel.
mix_hex() {
  local a=$1 b=$2 p=${3%\%} i ca cb
  printf '#'
  for i in 1 3 5; do
    ca=$((16#${a:i:2})); cb=$((16#${b:i:2}))
    printf '%02x' $(((ca * (100 - p) + cb * p + 50) / 100))
  done
}

# What omarchy-theme-set does with default/themed/shell.toml.tpl, for the
# three placeholder forms the template uses. A gradient placeholder takes its
# fallback colour: the real one reads Hyprland's border, which belongs to the
# theme on the desktop, not the one being photographed.
render_shell_toml() {
  local colors=$1 tpl=$2 out=$3 key value ph inner val
  declare -A c
  while IFS='=' read -r key value; do
    key=${key//[[:space:]]/}
    value=${value//[\"[:space:]]/}
    [[ -n $key && $key != \#* ]] && c[$key]=$value
  done < "$colors"
  local text; text=$(<"$tpl")
  while read -r ph; do
    inner=${ph#\{\{}; inner=${inner%\}\}}
    # shellcheck disable=SC2086
    set -- $inner
    case $1 in
      mix) val=$(mix_hex "${c[$2]}" "${c[$3]}" "$4") ;;
      shell_gradient) val=${c[$3]} ;;
      *) val=${c[$1]:-} ;;
    esac
    text=${text//"$ph"/$val}
  done < <(grep -o '{{[^}]*}}' "$tpl" | sort -u)
  printf '%s\n' "$text" > "$out"
}

# Fills THEME_ENV for one theme: either the live desktop theme, through the
# paths the app would read anyway, or a staged copy under OMARCHY_ROOT.
prepare_theme() {
  local name=$1 s root src bg
  THEME_ENV=()
  THEME_STAGED=0
  THEME_BG=""
  THEME_FILL=""
  if [[ $name == current ]]; then
    THEME_SLUG=$(tr -d '\n' < "$REAL_HOME/.local/state/omarchy/current/theme.name" 2>/dev/null || echo current)
    mkdir -p "$STATE_HOME" "$CONFIG_HOME"
    ln -sfn "$REAL_HOME/.local/state/omarchy" "$STATE_HOME/omarchy"
    ln -sfn "$REAL_HOME/.config/omarchy" "$CONFIG_HOME/omarchy"
    return
  fi
  THEME_STAGED=1
  s=$(slug "$name")
  src=$(theme_source "$name") || die "no theme called '$name' (omarchy-theme-list knows the names)"
  THEME_SLUG=$s
  root="$WORK/themes/$s"
  rm -rf "$root"
  mkdir -p "$root/state/current" "$root/config"
  cp -r "$src" "$root/state/current/theme"
  printf '%s\n' "$s" > "$root/state/current/theme.name"
  bg=$(find "$root/state/current/theme/backgrounds" -maxdepth 1 -type f 2>/dev/null | sort | sed -n 1p)
  [[ -n $bg ]] && ln -sfn "$bg" "$root/state/current/background"
  THEME_BG=$bg
  THEME_FILL=$(grep -E '^background\s*=' "$src/colors.toml" | head -1 | grep -oE '#[0-9a-fA-F]{6}' || true)
  if [[ ! -f "$root/state/current/theme/shell.toml" && -f "$OMARCHY_PATH/default/themed/shell.toml.tpl" ]]; then
    render_shell_toml "$root/state/current/theme/colors.toml" \
      "$OMARCHY_PATH/default/themed/shell.toml.tpl" "$root/state/current/theme/shell.toml"
  fi
  rm -f "$STATE_HOME/omarchy" "$CONFIG_HOME/omarchy"
  THEME_ENV=(OMARCHY_ROOT="$root")
}

# ---------------------------------------------------------------------------
# The window

# Hyprland 0.56 speaks Lua: `hyprctl dispatch workspace 3` is gone, and the
# window verbs take the window object.
hypr() { hyprctl repl "$1" >/dev/null; }

APP_PID=""
ADDRESS=""
ORIG_WS=""
SHOT_WS=""

launch() {
  local start=$1
  env HOME="$FAKE_HOME" XDG_CONFIG_HOME="$CONFIG_HOME" XDG_STATE_HOME="$STATE_HOME" \
    XDG_CACHE_HOME="$FAKE_HOME/.cache" "${THEME_ENV[@]}" "$BIN" "$start" \
    >"$WORK/app.log" 2>&1 &
  APP_PID=$!
  local i
  for i in $(seq 1 100); do
    ADDRESS=$(hyprctl clients -j | jq -r --argjson pid "$APP_PID" '.[] | select(.pid == $pid) | .address' | sed -n 1p)
    [[ -n $ADDRESS ]] && break
    kill -0 "$APP_PID" 2>/dev/null || die "omafiles exited during startup; see $WORK/app.log"
    sleep 0.1
  done
  [[ -n $ADDRESS ]] || die "no window appeared for pid $APP_PID"
  hypr "local w = hl.get_window('address:$ADDRESS')
    hl.dispatch(hl.dsp.window.move({ workspace = tostring($SHOT_WS), window = w }))
    hl.dispatch(hl.dsp.window.float({ window = w }))
    hl.dispatch(hl.dsp.window.resize({ x = $WIDTH, y = $HEIGHT, window = w }))
    hl.dispatch(hl.dsp.window.center({ window = w }))
    hl.dispatch(hl.dsp.focus({ window = w }))"
  read_geometry
  sleep 1.2
}

close_app() {
  [[ -n $APP_PID ]] && kill "$APP_PID" 2>/dev/null && wait "$APP_PID" 2>/dev/null || true
  APP_PID=""
  ADDRESS=""
}

# The servers the shots start are detached on purpose; they must not outlive
# the script the way they outlive the window.
stop_servers() {
  local info pid
  for info in "$STATE_HOME"/omafiles/servers/*.toml; do
    [[ -f $info ]] || continue
    pid=$(basename "$info" .toml)
    kill "$pid" 2>/dev/null || true
  done
  rm -rf "$STATE_HOME/omafiles/servers"
}

cleanup() {
  close_app
  stop_servers
  [[ -n $ORIG_WS ]] && hypr "hl.dispatch(hl.dsp.focus({ workspace = tostring($ORIG_WS) }))" || true
  if [[ $KEEP -eq 1 ]]; then
    log "Kept $WORK"
  else
    rm -rf "$WORK"
  fi
}
trap cleanup EXIT INT TERM

# The monitor's scale and Hyprland's corner radius, read once the window is
# up: the capture is in physical pixels, the radius in logical ones.
SCALE=1
RADIUS=0
read_geometry() {
  local client
  client=$(hyprctl clients -j | jq -c --arg a "$ADDRESS" '.[] | select(.address == $a)')
  SCALE=$(hyprctl monitors -j | jq -r --argjson id "$(jq -r '.monitor' <<<"$client")" '.[] | select(.id == $id) | .scale')
  RADIUS=$(hyprctl getoption decoration:rounding -j | jq -r '.int // 0')
}

# Logical pixels to physical ones, rounded.
phys() { awk -v v="$1" -v s="$SCALE" 'BEGIN { printf "%d", v * s + 0.5 }'; }

# The window's box plus a margin, clamped to its monitor.
region() {
  local margin=$1 client mon x y w h mx my mw mh
  client=$(hyprctl clients -j | jq -c --arg a "$ADDRESS" '.[] | select(.address == $a)')
  [[ -n $client ]] || die "the window vanished"
  x=$(jq -r '.at[0]' <<<"$client"); y=$(jq -r '.at[1]' <<<"$client")
  w=$(jq -r '.size[0]' <<<"$client"); h=$(jq -r '.size[1]' <<<"$client")
  mon=$(hyprctl monitors -j | jq -c --argjson id "$(jq -r '.monitor' <<<"$client")" '.[] | select(.id == $id)')
  mx=$(jq -r '.x' <<<"$mon"); my=$(jq -r '.y' <<<"$mon")
  mw=$(jq -r '(.width / .scale) | floor' <<<"$mon"); mh=$(jq -r '(.height / .scale) | floor' <<<"$mon")
  local x0=$((x - margin)) y0=$((y - margin)) x1=$((x + w + margin)) y1=$((y + h + margin))
  ((x0 < mx)) && x0=$mx
  ((y0 < my)) && y0=$my
  ((x1 > mx + mw)) && x1=$((mx + mw))
  ((y1 > my + mh)) && y1=$((my + mh))
  printf '%d,%d %dx%d' "$x0" "$y0" $((x1 - x0)) $((y1 - y0))
}

wanted() {
  [[ -z $ONLY ]] && return 0
  [[ ",$ONLY," == *",$1,"* ]]
}

# A staged theme's window sits on the desktop's wallpaper, which belongs to
# a different theme. So for those the bare window is captured, its corners
# rounded the way Hyprland rounds them, and set on the theme's own wallpaper
# at the same margin the live capture would have had.
compose() {
  local win=$1 out=$2 ww hh m r
  read -r ww hh <<<"$(magick identify -format '%w %h' "$win")"
  m=$(phys "$MARGIN"); r=$(phys "$RADIUS")
  local mask=(-size "${ww}x${hh}" xc:none -draw "roundrectangle 0,0 $((ww - 1)),$((hh - 1)) $r,$r")
  local canvas="$((ww + 2 * m))x$((hh + 2 * m))"
  if [[ -n $THEME_BG && -f $THEME_BG ]]; then
    magick "$THEME_BG" -resize "${canvas}^" -gravity center -extent "$canvas" \
      \( "$win" \( "${mask[@]}" \) -compose CopyOpacity -composite \) \
      -gravity northwest -geometry "+$m+$m" -compose Over -composite "$out"
  else
    magick -size "$canvas" "xc:${THEME_FILL:-#000000}" \
      \( "$win" \( "${mask[@]}" \) -compose CopyOpacity -composite \) \
      -gravity northwest -geometry "+$m+$m" -compose Over -composite "$out"
  fi
}

shot() {
  local name=$1
  wanted "$name" || return 0
  sleep "$SETTLE"
  if [[ $THEME_STAGED -eq 1 ]]; then
    grim -g "$(region 0)" "$WORK/win.png"
    compose "$WORK/win.png" "$THEME_OUT/$name.png"
  else
    grim -g "$(region "$MARGIN")" "$THEME_OUT/$name.png"
  fi
  log "  $THEME_SLUG/$name.png"
}

# key ctrl shift g / key Escape / key space: modifiers first, the key last.
key() {
  local args=() mods=("${@:1:$#-1}") k=${*: -1} m
  for m in "${mods[@]}"; do args+=(-M "$m"); done
  args+=(-k "$k")
  for ((i = ${#mods[@]} - 1; i >= 0; i--)); do args+=(-m "${mods[i]}"); done
  wtype "${args[@]}"
  sleep 0.25
}

type_text() {
  wtype -d 25 -- "$1"
  sleep 0.3
}

# The finder is the deterministic way to land on a file: the row goes to its
# directory with the cursor on it, whatever the listing's order.
goto() {
  wtype '/'
  sleep 0.3
  type_text "$1"
  sleep 0.6
  key Return
  sleep 0.8
}

wait_for_server() {
  local i
  for i in $(seq 1 50); do
    if compgen -G "$STATE_HOME/omafiles/servers/*.toml" >/dev/null; then
      sleep 0.3
      return 0
    fi
    sleep 0.1
  done
  die "no server registered itself"
}

# A few requests, so the running-server view has a log and a hit count.
warm_server() {
  local info port
  for info in "$STATE_HOME"/omafiles/servers/*.toml; do
    port=$(grep -E '^port' "$info" | grep -oE '[0-9]+')
    curl -s "http://127.0.0.1:$port/" >/dev/null || true
    curl -s "http://127.0.0.1:$port/README.md" >/dev/null || true
    curl -s "http://127.0.0.1:$port/src/main.rs" >/dev/null || true
    curl -s "http://127.0.0.1:$port/assets/logo.png" >/dev/null || true
  done
  sleep 0.6
}

# ---------------------------------------------------------------------------
# The shots, in an order where each leaves the app ready for the next.
take_all() {
  # Frame one: the repo, README selected, workspaces in the sidebar.
  sleep 1
  shot overview

  # Git: the modified file previews as its diff, then large.
  goto "auth.rs"
  shot git-diff
  key space; sleep 0.5
  shot git-diff-expanded
  key Escape

  # Code: an unchanged Rust file, highlighted, then large.
  goto "main.rs"
  shot preview-code
  key space; sleep 0.5
  shot preview-code-expanded
  key Escape

  key ctrl shift g; sleep 0.5
  shot git-branches
  key Escape

  # Back to the repo root: the finder looks below the current directory.
  key h; sleep 0.5

  goto "README.md"
  key space; sleep 0.5
  shot preview-markdown-expanded
  key Escape

  # The finder: recent files on an empty query, then names and content.
  wtype '/'; sleep 0.8
  shot search-recent
  type_text "auth"; sleep 1.2
  shot search
  key Escape

  key ctrl k; sleep 0.4
  type_text "work"; sleep 0.4
  shot palette
  key Escape

  wtype '?'; sleep 0.5
  shot help
  key Escape

  # Serve the repo, warm the log, then a second one and the list.
  key ctrl s; sleep 0.5
  shot server-start
  key Return
  wait_for_server
  warm_server
  sleep 0.5
  shot server-running
  key Escape
  key ctrl Tab; sleep 0.8
  key ctrl s; sleep 0.4
  key Return; sleep 1.2
  key Escape
  key ctrl shift s; sleep 0.6
  shot server-list
  key Escape
  key ctrl shift Tab; sleep 0.6

  key ctrl n; sleep 0.4
  type_text "lumen 1.3"
  shot workspace-new
  key Escape

  goto "Cargo.toml"
  key shift F10; sleep 0.5
  shot context-menu
  key Escape

  key ctrl l; sleep 0.4
  key ctrl a
  type_text "~/Documents/Github/lumen/w"; sleep 0.5
  shot path-edit
  key Escape

  key ctrl shift n; sleep 0.4
  type_text "smb://nas.local/photos"
  shot network-add
  key Escape

  key a; sleep 0.5
  shot agent-prompt
  key Escape

  # Up to the home directory, so the finder can reach the pictures.
  key h; key h; key h; sleep 0.5
  goto "dunes"
  sleep 0.6
  shot preview-image
  key space; sleep 0.6
  key j; sleep 0.6
  shot preview-image-expanded
  key Escape

  # Copying a picture asks what to copy; the PNG rows fill in as ffmpeg
  # finishes each size.
  key ctrl c; sleep 2.5
  shot copy-image
  key Escape

  key Delete; sleep 0.5
  shot delete
  key Escape

  if [[ -f "$FAKE_HOME/Videos/launch-teaser.mp4" ]]; then
    key h; key h; sleep 0.4
    goto "teaser"
    sleep 1.2
    shot preview-video
  fi

  # Panels: collapse the sidebar, then the detail panel too.
  key h; sleep 0.4
  goto "routes.rs"
  key ctrl b; sleep 0.5
  shot sidebar-collapsed
  key ctrl shift b; sleep 0.5
  shot listing-only
  key ctrl shift b
  key ctrl b
}

# ---------------------------------------------------------------------------

WORK=$(mktemp -d "${XDG_RUNTIME_DIR:-/tmp}/omafiles-shots.XXXXXX")
FAKE_HOME="$WORK/home"
CONFIG_HOME="$FAKE_HOME/.config"
STATE_HOME="$FAKE_HOME/.local/state"

log "Building the demo home in $WORK"
build_fixture "$FAKE_HOME"

if [[ ${#THEMES[@]} -eq 1 && ${THEMES[0]} == all ]]; then
  mapfile -t THEMES < <(all_themes)
fi

ORIG_WS=$(hyprctl activeworkspace -j | jq -r '.id')
SHOT_WS=$(hyprctl workspaces -j | jq -r '[.[].id] | max + 1')
((SHOT_WS < 1)) && SHOT_WS=9
hypr "hl.dispatch(hl.dsp.focus({ workspace = tostring($SHOT_WS) }))"
sleep 0.4

TAKEN=()
for theme in "${THEMES[@]}"; do
  prepare_theme "$theme"
  THEME_OUT="$OUT/$THEME_SLUG"
  mkdir -p "$THEME_OUT"
  log "Theme: $THEME_SLUG"
  # Every theme starts from the same seeded session.
  fixture_omafiles_config "$FAKE_HOME" "$FAKE_HOME/Documents/Github/lumen" "$FAKE_HOME/Documents/Clients/Acme"
  stop_servers
  launch "$FAKE_HOME/Documents/Github/lumen"
  take_all
  close_app
  TAKEN+=("$THEME_OUT/overview.png")
done

if [[ ${#TAKEN[@]} -gt 1 ]] && wanted overview; then
  log "Grid: $OUT/themes.png"
  cols=3
  ((${#TAKEN[@]} <= 4)) && cols=2
  magick montage "${TAKEN[@]}" -tile "${cols}x" -geometry +24+24 -background none "$OUT/themes.png"
fi

log "Done: $OUT"
