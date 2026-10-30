#!/bin/bash
set -euo pipefail
export OMARCHY_PATH=/usr/share/omarchy
export PATH=/home/cma/.local/bin:/usr/local/bin:/usr/share/omarchy/bin:/usr/bin
export XDG_CURRENT_DESKTOP=Hyprland XDG_SESSION_TYPE=wayland LANG=en_US.UTF-8
export QT_QPA_PLATFORM=wayland QT_QUICK_BACKEND=software QSG_RHI_BACKEND=software
export XCURSOR_SIZE=24
mkdir -p "$HOME/.config" "$HOME/.local/state/omarchy" "$HOME/.local/share/applications"
if [[ ! -e "$HOME/.config/omarchy/shell.json" ]]; then
  cp -an /usr/share/omarchy/config/. "$HOME/.config/"
  python3 - <<'PY'
import json, os
from pathlib import Path
p = Path(os.environ['HOME']) / '.config/omarchy/shell.json'
d = json.loads(p.read_text()); d['idle'] = {'screensaver': 0, 'lock': 0}
p.write_text(json.dumps(d))
PY
fi
if [[ ! -s "$HOME/.local/state/omarchy/current/theme.name" ]]; then
  OMARCHY_THEME_HEADLESS=1 omarchy-theme-set 'Tokyo Night'
  ln -snf /usr/share/cma-desktop/wallpaper.png "$HOME/.local/state/omarchy/current/background"
fi
children=()
stop() { trap - EXIT INT TERM; kill "${children[@]}" 2>/dev/null || true; wait "${children[@]}" 2>/dev/null || true; }
trap stop EXIT INT TERM
renderer=pixman
for device in /dev/dri/renderD*; do
  if [[ -r "$device" && -w "$device" ]]; then renderer=gl; break; fi
done
weston --backend=x11 --renderer="$renderer" --shell=kiosk-shell.so --width=1280 --height=800 --socket=cma-host --idle-time=0 >/tmp/cma-weston.log 2>&1 &
children+=("$!")
for ((i=0;i<100;i++)); do [[ -S "$XDG_RUNTIME_DIR/cma-host" ]] && break; sleep 0.1; done
[[ -S "$XDG_RUNTIME_DIR/cma-host" ]]
export WAYLAND_DISPLAY=cma-host
Hyprland --config /usr/share/cma-omarchy/hyprland.lua >/tmp/cma-hyprland.log 2>&1 &
children+=("$!")
for ((i=0;i<300;i++)); do
  instance=$(hyprctl instances -j 2>/dev/null | jq -r '.[0].instance // empty' || true)
  [[ -z "$instance" ]] || break
  kill -0 "${children[1]}"
  sleep 0.1
done
[[ -n "$instance" ]]
export HYPRLAND_INSTANCE_SIGNATURE="$instance"
printf '%s\n' "$instance" > "$XDG_RUNTIME_DIR/cma-hyprland-instance"
for ((i=0;i<300;i++)); do
  hyprctl -j clients 2>/dev/null | jq -e 'any(.[]; .class | ascii_downcase | contains("chromium"))' >/dev/null && break
  kill -0 "${children[1]}"
  sleep 0.1
done
hyprctl -j clients | jq -e 'any(.[]; .class | ascii_downcase | contains("chromium"))' >/dev/null
window=$(xdotool search --onlyvisible --class weston | head -1)
xdotool windowfocus --sync "$window"
touch /tmp/cma-omarchy-ready
wait -n "${children[@]}"
exit 1
