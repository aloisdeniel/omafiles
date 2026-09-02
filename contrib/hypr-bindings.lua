-- Take over Omarchy's file-manager keys — OPT-IN.
--
-- Omarchy binds these to Nautilus in
-- /usr/share/omarchy/default/hypr/bindings/applications.lua. The documented
-- override pattern is unbind-then-rebind in your own config; append this
-- file's contents to ~/.config/hypr/bindings.lua (or `dofile` it from there).
--
-- omafiles opens in the directory it is given, so the cwd variant reuses
-- Omarchy's own "ask the focused terminal for its directory" helper.

hl.unbind("SUPER + SHIFT + F")
hl.unbind("SUPER + ALT + SHIFT + F")

o.bind("SUPER + SHIFT + F", "File manager", "uwsm-app -- omafiles")
o.bind(
  "SUPER + ALT + SHIFT + F",
  "File manager (cwd)",
  'sh -c \'uwsm-app -- omafiles "$(omarchy-cmd-terminal-cwd)"\''
)
