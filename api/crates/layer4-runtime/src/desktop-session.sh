#!/bin/bash
# One desktop belongs to one Sandbox; all user and Agent input uses this display.
set -euo pipefail
export DISPLAY=:99
mkdir -p "$HOME/.cma/chrome"
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
trap 'trap - INT TERM EXIT; kill "${children[@]}" 2>/dev/null || true; wait "${children[@]}" 2>/dev/null || true' INT TERM EXIT
require_process() {
  if ! kill -0 "$1" 2>/dev/null; then
    tail -c 4000 "$2" >&2
    exit 1
  fi
}
Xvfb "$DISPLAY" -screen 0 1280x800x24 -nolisten tcp -ac >/tmp/cma-xvfb.log 2>&1 &
children+=("$!")
for ((attempt=0; attempt<100; attempt++)); do
  xdotool getdisplaygeometry >/dev/null 2>&1 && break
  require_process "${children[0]}" /tmp/cma-xvfb.log
  sleep 0.1
done
xdotool getdisplaygeometry >/dev/null
openbox >/tmp/cma-openbox.log 2>&1 &
children+=("$!")
chromium --user-data-dir="$HOME/.cma/chrome" --no-first-run --disable-gpu --disable-dev-shm-usage about:blank >/tmp/cma-chromium.log 2>&1 &
chrome_pid=$!
children+=("$chrome_pid")
for ((attempt=0; attempt<200; attempt++)); do
  xdotool search --onlyvisible --class chromium >/dev/null 2>&1 && break
  require_process "$chrome_pid" /tmp/cma-chromium.log
  sleep 0.1
done
xdotool search --onlyvisible --class chromium >/dev/null
x11vnc -display "$DISPLAY" -rfbport 5900 -localhost -forever -shared -nopw >/tmp/cma-x11vnc.log 2>&1 &
children+=("$!")
websockify --web=/usr/share/novnc 127.0.0.1:6080 127.0.0.1:5900 >/tmp/cma-websockify.log 2>&1 &
children+=("$!")
# Any failed desktop component tears down the whole session instead of leaving
# a blank, apparently connected desktop behind.
wait -n "${children[@]}"
exit 1
