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
  printf 'foot.desktop\n' > "$HOME/.config/xdg-terminals.list"
  printf '[Default Applications]\nx-scheme-handler/http=chromium.desktop\nx-scheme-handler/https=chromium.desktop\ntext/html=chromium.desktop\n' > "$HOME/.config/mimeapps.list"
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
export WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=1 WLR_RENDERER=pixman
export WLR_RENDERER_ALLOW_SOFTWARE=1
for device in /dev/dri/renderD*; do
  if [[ -r "$device" && -w "$device" ]]; then
    export WLR_RENDERER=gles2 WLR_RENDER_DRM_DEVICE="$device"
    break
  fi
done
# VKMS has a primary node only. Its dumb buffers are supported by Mesa's
# software renderer; vgem's render node denies MODE_CREATE_DUMB.
for device in /dev/dri/card*; do
  [[ -r "$device" && -w "$device" ]] || continue
  driver=$(readlink -f "/sys/class/drm/${device##*/}/device/driver" || true)
  if [[ ${driver##*/} == vkms ]]; then
    export WLR_RENDERER=gles2 WLR_RENDER_DRM_DEVICE="$device"
    break
  fi
done
# The host only transports the actual Hyprland desktop. No host keybindings
# intercept the owner's input; every inner window occupies the whole output.
mkdir -p /tmp/cma-labwc
cat > /tmp/cma-labwc/rc.xml <<'XML'
<labwc_config>
  <keyboard><keybind key="W-F35"><action name="None" /></keybind></keyboard>
  <mouse><context name="Root"><mousebind button="Left" action="Press"><action name="None" /></mousebind></context></mouse>
  <windowRules><windowRule identifier="*" serverDecoration="no">
    <action name="ToggleFullscreen" />
  </windowRule></windowRules>
</labwc_config>
XML
labwc --config-dir /tmp/cma-labwc --debug >/tmp/cma-labwc.log 2>&1 &
children+=("$!")
for ((i=0;i<100;i++)); do [[ -S "$XDG_RUNTIME_DIR/wayland-0" ]] && break; sleep 0.1; done
[[ -S "$XDG_RUNTIME_DIR/wayland-0" ]]
export WAYLAND_DISPLAY=wayland-0
wlr-randr --output HEADLESS-1 --custom-mode 1280x800@30Hz
# Capture the Wayland output directly. CMA's authenticated relay is its only
# remote entry point; the X11 viewer is an in-Sandbox adapter for native tools.
wayvnc --config /dev/null --output HEADLESS-1 --max-fps 30 127.0.0.1 5900 >/tmp/cma-wayvnc.log 2>&1 &
children+=("$!")
for ((i=0;i<100;i++)); do
  if python3 -c 'import socket; s=socket.create_connection(("127.0.0.1",5900),.1); s.close()' 2>/dev/null; then break; fi
  kill -0 "${children[1]}"
  sleep 0.1
done
# FLTK must use X11 even though the compositor's client environment is Wayland.
env -u WAYLAND_DISPLAY vncviewer -Shared -SecurityTypes None -FullScreen \
  -RemoteResize=0 -ShortcutModifiers= -AcceptClipboard=0 -SendClipboard=0 \
  -geometry 1280x800+0+0 127.0.0.1:5900 >/tmp/cma-vncviewer.log 2>&1 &
children+=("$!")
for ((i=0;i<100;i++)); do
  window=$(xdotool search --onlyvisible --class 'Vncviewer' | head -1 || true)
  [[ -z "$window" ]] || break
  kill -0 "${children[2]}"
  sleep 0.1
done
[[ -n "$window" ]]
xdotool set_window --name CMA-Omarchy-Transport "$window"
xdotool windowsize "$window" 1280 800 windowmove "$window" 0 0
export WAYLAND_DISPLAY=wayland-0
Hyprland --config /usr/share/cma-omarchy/hyprland.lua >/tmp/cma-hyprland.log 2>&1 &
children+=("$!")
hypr_pid=$!
for ((i=0;i<300;i++)); do
  instance=$(hyprctl instances -j 2>/dev/null | jq -r '.[0].instance // empty' || true)
  [[ -z "$instance" ]] || break
  kill -0 "$hypr_pid"
  sleep 0.1
done
[[ -n "$instance" ]]
export HYPRLAND_INSTANCE_SIGNATURE="$instance"
printf '%s\n' "$instance" > "$XDG_RUNTIME_DIR/cma-hyprland-instance"
for ((i=0;i<300;i++)); do
  hyprctl -j clients 2>/dev/null | jq -e 'any(.[]; .class | ascii_downcase | contains("chromium"))' >/dev/null && break
  kill -0 "$hypr_pid"
  sleep 0.1
done
hyprctl -j clients | jq -e 'any(.[]; .class | ascii_downcase | contains("chromium"))' >/dev/null
xdotool windowfocus --sync "$window"
touch /tmp/cma-omarchy-ready
wait -n "${children[@]}"
exit 1
