"""Run the installed launcher scripts against synthetic desktop commands."""

import os
from pathlib import Path
import shlex
import signal
import subprocess
import sys
import tempfile
import time


def executable(path, text):
    path.write_text(text)
    path.chmod(0o700)


def relocate(script, tools):
    return (
        script.replace("/tmp/", "${CMA_FIXTURE_TMP}/")
        .replace("/usr/local/bin/cma-omarchy-inner-session", shlex.quote(str(tools / "inner")))
        .replace(
            "export PATH=/home/cma/.local/bin:/usr/local/bin:/usr/share/omarchy/bin:/usr/bin",
            'export PATH="$CMA_FIXTURE_BIN:$PATH"',
        )
    )


def wait_for(path, process):
    deadline = time.monotonic() + 5
    while not path.exists() and process.poll() is None and time.monotonic() < deadline:
        time.sleep(0.02)
    assert path.exists(), (path, process.poll())


def stop(process):
    try:
        os.killpg(process.pid, signal.SIGTERM)
        process.wait(3)
    except ProcessLookupError:
        pass
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(3)


def launch_twice(outer, inner):
    with tempfile.TemporaryDirectory(prefix="cma-omarchy-test-") as temporary:
        root = Path(temporary)
        tools, home, local = (root / name for name in ("bin", "home", "local"))
        tools.mkdir()
        local.mkdir()
        (local / "cma-omarchy-ready").touch()
        (home / ".config/omarchy").mkdir(parents=True)
        (home / ".config/omarchy/shell.json").touch()
        theme = home / ".local/state/omarchy/current/theme.name"
        theme.parent.mkdir(parents=True)
        theme.write_text("fixture")
        executable(tools / "session", relocate(outer, tools))
        executable(tools / "inner", relocate(inner, tools))
        for name in ("Xvfb", "Hyprland"):
            executable(tools / name, "#!/bin/sh\nexec sleep 300\n")
        for name in ("xdpyinfo", "wlr-randr", "hyprctl", "xwininfo", "xprop"):
            executable(tools / name, "#!/bin/sh\nexit 0\n")
        executable(tools / "xdotool", '#!/bin/sh\n[ "$1" != search ] || echo 1\n')
        executable(tools / "dbus-run-session", '#!/bin/sh\nshift\nexec "$@"\n')
        executable(tools / "python3", """#!/bin/sh
case "$1" in
  -) cat >/dev/null;;
  *cma-desktop-browser) exec sleep 300;;
esac
""")
        executable(tools / "labwc", """#!/bin/sh
exec "$CMA_FIXTURE_PYTHON" -c 'import os,socket,time; s=socket.socket(socket.AF_UNIX); s.bind(os.environ["XDG_RUNTIME_DIR"]+"/wayland-0"); time.sleep(300)'
""")
        executable(tools / "jq", """#!/bin/sh
case "$*" in
  *instance*) echo fixture-instance;;
  *chromium*)
    touch "$CMA_FIXTURE_TMP/compositor-check"
    test -f "$CMA_FIXTURE_TMP/finish-chromium";;
esac
""")
        executable(tools / "wayvnc", """#!/bin/sh
while [ "$#" -gt 1 ]; do
  if [ "$1" = --unix-socket ]; then
    echo "$2" > "$CMA_FIXTURE_TMP/rfb-listener.pending"
    mv "$CMA_FIXTURE_TMP/rfb-listener.pending" "$CMA_FIXTURE_TMP/rfb-listener"
    exec sleep 300
  fi
  shift
done
exit 1
""")
        executable(tools / "vncviewer", """#!/bin/sh
for destination do :; done
echo "$destination" > "$CMA_FIXTURE_TMP/viewer-target.pending"
mv "$CMA_FIXTURE_TMP/viewer-target.pending" "$CMA_FIXTURE_TMP/viewer-target"
exec sleep 300
""")
        executable(tools / "websockify", """#!/bin/sh
test "$2" = --token-plugin=ReadOnlyTokenFile || exit 1
for arg do
  case "$arg" in
    --unix-listen=*)
      echo "${arg#*=}" > "$CMA_FIXTURE_TMP/proxy-started.pending"
      mv "$CMA_FIXTURE_TMP/proxy-started.pending" "$CMA_FIXTURE_TMP/proxy-started";;
  esac
done
exec sleep 300
""")

        for attempt in (1, 2):
            for name in ("compositor-check", "finish-chromium", "proxy-started", "rfb-listener",
                         "viewer-target", "cma-desktop-runtime/wayland-0"):
                (local / name).unlink(missing_ok=True)
            generation = root / f"generation-{attempt}"
            generation.mkdir(mode=0o700)
            env = dict(
                os.environ,
                HOME=str(home),
                PATH=f'{tools}:{os.environ["PATH"]}',
                CMA_FIXTURE_BIN=str(tools),
                CMA_FIXTURE_TMP=str(local),
                CMA_FIXTURE_PYTHON=sys.executable,
                CMA_DESKTOP_GENERATION_DIR=str(generation),
                CMA_DESKTOP_TOKEN_FILE=str(generation / "synthetic-token-source"),
                CMA_DESKTOP_TOKEN=f"{attempt:032x}",
            )
            with (local / "launcher-output").open("wb") as log:
                process = subprocess.Popen(
                    [str(tools / "session")], env=env, stdout=log, stderr=log, start_new_session=True
                )
                try:
                    wait_for(local / "compositor-check", process)
                    assert (local / "rfb-listener").read_text().strip() == str(generation / "rfb.sock")
                    assert (local / "viewer-target").read_text().strip() == str(generation / "rfb.sock")
                    if attempt == 2:
                        assert (root / "generation-1/omarchy-ready").is_file()
                    (generation / "token-ready").touch()
                    time.sleep(0.35)
                    assert process.poll() is None
                    assert not (local / "proxy-started").exists()
                    assert not (generation / "omarchy-ready").exists()
                    (local / "finish-chromium").touch()
                    wait_for(local / "proxy-started", process)
                    assert (generation / "omarchy-ready").is_file()
                    assert (local / "proxy-started").read_text().strip() == str(generation / "display.sock")
                except Exception:
                    sys.stderr.write((local / "launcher-output").read_text()[-4000:])
                    raise
                finally:
                    stop(process)


launch_twice(*sys.argv[1:])
