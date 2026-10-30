#!/bin/bash
# A genuine Omarchy desktop nested into CMA's existing shared X11 transport.
set -euo pipefail
ulimit -c 0
export DISPLAY=:99 OMARCHY_PATH=/usr/share/omarchy
export XDG_RUNTIME_DIR=/tmp/cma-desktop-runtime
if [[ ${1:-} == --browser ]]; then
  shift
  export WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-wayland-1}
  exec chromium --ozone-platform=wayland --user-data-dir="$HOME/.cma/chrome" --no-first-run --disable-gpu --disable-dev-shm-usage "${@:-about:blank}"
fi
[[ ${CMA_DESKTOP_TOKEN:-} =~ ^[0-9a-f]{32}$ ]]
umask 077
mkdir -p "$XDG_RUNTIME_DIR" "$HOME/.cma/chrome" "$HOME/Desktop" "$HOME/Documents" "$HOME/Downloads" /tmp/.X11-unix
chmod 700 "$XDG_RUNTIME_DIR"
chmod 1777 /tmp/.X11-unix
exec 9>"$HOME/.cma/desktop.lock"
flock -n 9
printf '%s: 127.0.0.1:5900\n' "$CMA_DESKTOP_TOKEN" >/tmp/cma-desktop-token
unset CMA_DESKTOP_TOKEN
# Reuse the baseline's guarded stale-browser-lock recovery.
python3 - <<'PY'
import os
from pathlib import Path
p = Path(os.environ['HOME']) / '.cma/chrome'
argument = ('--user-data-dir=' + str(p)).encode()
for entry in Path('/proc').iterdir():
    if entry.name.isdigit():
        try:
            args = (entry / 'cmdline').read_bytes().split(b'\0')
            if argument in args and b'chrom' in args[0]:
                raise SystemExit('desktop profile already has a live browser')
        except (FileNotFoundError, ProcessLookupError):
            pass
for name in ('SingletonLock', 'SingletonCookie', 'SingletonSocket'):
    if (p / name).is_symlink():
        (p / name).unlink()
PY
children=()
stop() {
  local status=$?
  trap - EXIT INT TERM
  if ((status != 0)); then
    for log in /tmp/cma-*.log "$XDG_RUNTIME_DIR"/hypr/*/hyprland.log; do
      [[ -f "$log" ]] || continue
      printf '\nDesktop diagnostic: %s\n' "$log" >&2
      head -c 5000 "$log" >&2 || true
      tail -c 15000 "$log" >&2 || true
    done
  fi
  kill "${children[@]}" 2>/dev/null || true
  wait "${children[@]}" 2>/dev/null || true
}
trap stop EXIT INT TERM
Xvfb :99 -screen 0 1280x800x24 -nolisten tcp -ac -noreset -s 0 >/tmp/cma-xvfb.log 2>&1 &
children+=("$!")
for ((i=0;i<100;i++)); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done
xdpyinfo >/dev/null
# wlroots' nested X11 backend looks up these names with only_if_exists.
# A bare Xvfb server has no window manager to initialize them.
python3 - <<'PY'
import ctypes
x = ctypes.CDLL('libX11.so.6')
x.XOpenDisplay.argtypes = [ctypes.c_char_p]
x.XOpenDisplay.restype = ctypes.c_void_p
x.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
x.XInternAtom.restype = ctypes.c_ulong
x.XCloseDisplay.argtypes = [ctypes.c_void_p]
d = x.XOpenDisplay(b':99')
assert d
for name in ('WM_PROTOCOLS', 'WM_DELETE_WINDOW', '_NET_WM_NAME', 'UTF8_STRING', '_VARIABLE_REFRESH'):
    assert x.XInternAtom(d, name.encode(), 0)
x.XCloseDisplay(d)
PY
export LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe LP_NUM_THREADS=2
dbus-run-session -- /usr/local/bin/cma-omarchy-inner-session >/tmp/cma-omarchy.log 2>&1 &
children+=("$!")
for ((i=0;i<600;i++)); do
  [[ -f /tmp/cma-omarchy-ready ]] && break
  kill -0 "${children[1]}"
  sleep 0.1
done
[[ -f /tmp/cma-omarchy-ready ]]
x11vnc -display :99 -rfbport 5900 -localhost -forever -shared -nopw >/tmp/cma-x11vnc.log 2>&1 &
children+=("$!")
websockify --host-token --token-plugin=TokenFile --token-source=/tmp/cma-desktop-token 127.0.0.1:6080 >/tmp/cma-websockify.log 2>&1 &
children+=("$!")
wait -n "${children[@]}"
exit 1
