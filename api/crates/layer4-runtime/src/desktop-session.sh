#!/bin/bash
# One desktop belongs to one Sandbox; all user and Agent input uses this display.
set -euo pipefail
export DISPLAY=:99
if [[ ${1:-} == --browser ]]; then
  exec chromium --user-data-dir="$HOME/.cma/chrome" --no-first-run --disable-gpu --disable-dev-shm-usage about:blank
fi
[[ ${CMA_DESKTOP_TOKEN:-} =~ ^[0-9a-f]{32}$ ]]
# A web page in Chromium must not bypass CMA by connecting to a loopback
# WebSocket. Only the supervisor knows this per-boot transport token.
umask 077
printf '%s: 127.0.0.1:5900\n' "$CMA_DESKTOP_TOKEN" >/tmp/cma-desktop-token
unset CMA_DESKTOP_TOKEN
mkdir -p "$HOME/.cma/chrome"
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
x11vnc -display "$DISPLAY" -rfbport 5900 -localhost -forever -shared -nopw >/tmp/cma-x11vnc.log 2>&1 &
children+=("$!")
websockify --host-token --token-plugin=TokenFile --token-source=/tmp/cma-desktop-token 127.0.0.1:6080 >/tmp/cma-websockify.log 2>&1 &
children+=("$!")
# Closing the browser is normal. Keep the desktop available, and let the user
# reopen Chromium from the desktop's right-click menu (or ask their Agent).
# Display/window-manager/stream failure still tears down the session.
wait -n "${children[0]}" "${children[1]}" "${children[3]}" "${children[4]}"
exit 1
