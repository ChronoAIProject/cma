#!/bin/bash
# One desktop belongs to one Sandbox; all user and Agent input uses this display.
set -euo pipefail
export DISPLAY=:99
if [[ ${1:-} == --browser ]]; then
  shift
  exec chromium --user-data-dir="$HOME/.cma/chrome" --no-first-run --disable-gpu --disable-dev-shm-usage "${@:-about:blank}"
fi
[[ ${CMA_DESKTOP_TOKEN:-} =~ ^[0-9a-f]{32}$ ]]
# A web page in Chromium must not bypass CMA by connecting to a loopback
# WebSocket. Only the supervisor knows this per-boot transport token.
umask 077
printf '%s: 127.0.0.1:5900\n' "$CMA_DESKTOP_TOKEN" >/tmp/cma-desktop-token
unset CMA_DESKTOP_TOKEN
mkdir -p "$HOME/.cma/chrome" "$HOME/Desktop" "$HOME/Documents" "$HOME/Downloads"
export XDG_RUNTIME_DIR=/tmp/cma-desktop-runtime
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
export XDG_CURRENT_DESKTOP=XFCE XDG_SESSION_TYPE=x11 DESKTOP_SESSION=xfce
# Seed desktop preferences only on first use; customization belongs to the home.
config="$HOME/.config/xfce4/xfconf/xfce-perchannel-xml"
mkdir -p "$config"
for source in /usr/share/cma-desktop/*.xml; do
  target="$config/${source##*/}"
  if [[ ! -e "$target" && ! -L "$target" ]]; then
    cp --no-clobber "$source" "$target"
  fi
done
# /tmp is an empty writable mount. Xvfb cannot create the conventional socket
# directory itself as a non-root user, but accepts an existing shared directory.
mkdir -p /tmp/.X11-unix
chmod 1777 /tmp/.X11-unix
# Retained Chromium profiles carry singleton symlinks naming the previous Pod.
# Admit one supervisor and prove no live browser owns this exact profile before
# removing those three stale links; profile contents and cookies remain intact.
exec 9>"$HOME/.cma/desktop.lock"
flock -n 9
python3 - <<'PY'
import os
from pathlib import Path
profile = Path(os.environ['HOME']) / '.cma/chrome'
argument = ('--user-data-dir=' + str(profile)).encode()
for entry in Path('/proc').iterdir():
    if not entry.name.isdigit():
        continue
    try:
        args = (entry / 'cmdline').read_bytes().split(b'\0')
        if argument in args and b'chromium' in args[0]:
            raise SystemExit('desktop profile already has a live browser')
    except (FileNotFoundError, ProcessLookupError):
        pass
for name in ('SingletonLock', 'SingletonCookie', 'SingletonSocket'):
    path = profile / name
    if path.is_symlink():
        path.unlink()
PY
children=()
desktop_stage=display
stop_desktop() {
  local result=$?
  trap - INT TERM EXIT
  if ((result != 0)); then
    printf 'desktop stopped during %s (exit %s)\n' "$desktop_stage" "$result" >&2
    tail -c 4000 /tmp/cma-{xvfb,xfce,chromium,x11vnc,websockify}.log >&2 2>/dev/null || true
  fi
  kill "${children[@]}" 2>/dev/null || true
  wait "${children[@]}" 2>/dev/null || true
}
trap stop_desktop INT TERM EXIT
require_process() {
  if ! kill -0 "$1" 2>/dev/null; then
    tail -c 4000 "$2" >&2
    exit 1
  fi
}
# The readiness client disconnects before the window manager starts. Keep the
# same display alive across that gap instead of resetting all X11 state.
Xvfb "$DISPLAY" -screen 0 1280x800x24 -nolisten tcp -ac -noreset -s 0 >/tmp/cma-xvfb.log 2>&1 &
children+=("$!")
for ((attempt=0; attempt<100; attempt++)); do
  xdotool getdisplaygeometry >/dev/null 2>&1 && break
  require_process "${children[0]}" /tmp/cma-xvfb.log
  sleep 0.1
done
xdotool getdisplaygeometry >/dev/null
desktop_stage=desktop-shell
dbus-run-session -- xfce4-session >/tmp/cma-xfce.log 2>&1 &
children+=("$!")
wm_ready=0
for ((attempt=0; attempt<300; attempt++)); do
  if wmctrl -m >/dev/null 2>&1 \
    && xdotool search --onlyvisible --class xfce4-panel >/dev/null 2>&1 \
    && xdotool search --onlyvisible --class xfdesktop >/dev/null 2>&1; then
    wm_ready=1
    break
  fi
  require_process "${children[1]}" /tmp/cma-xfce.log
  sleep 0.1
done
[[ "$wm_ready" == 1 ]]
desktop_stage=browser
"$0" --browser >/tmp/cma-chromium.log 2>&1 &
chrome_pid=$!
children+=("$chrome_pid")
chrome_ready=0
for ((attempt=0; attempt<200; attempt++)); do
  if xdotool search --onlyvisible --class chromium >/dev/null 2>&1; then
    chrome_ready=1
    break
  fi
  require_process "$chrome_pid" /tmp/cma-chromium.log
  sleep 0.1
done
# Chromium can replace startup windows between X11 queries. Do not repeat a
# successful observation outside the retry loop, where BadWindow would tear
# down an otherwise healthy display under set -e.
[[ "$chrome_ready" == 1 ]]
desktop_stage=listeners
x11vnc -display "$DISPLAY" -rfbport 5900 -localhost -forever -shared -nopw >/tmp/cma-x11vnc.log 2>&1 &
children+=("$!")
websockify --host-token --token-plugin=TokenFile --token-source=/tmp/cma-desktop-token 127.0.0.1:6080 >/tmp/cma-websockify.log 2>&1 &
children+=("$!")
# Closing the browser is normal. Keep the desktop available, and let the user
# reopen applications from the persistent XFCE panel, menu or desktop icons.
# Display/window-manager/stream failure still tears down the session.
desktop_stage=running
wait -n "${children[0]}" "${children[1]}" "${children[3]}" "${children[4]}"
exit 1
