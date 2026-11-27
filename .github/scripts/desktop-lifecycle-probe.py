"""Exercise live process ownership and clean browser closure in a fixture Pod."""
import base64
import errno
import importlib.util
import json
import os
from pathlib import Path
import runpy
import socket
import subprocess
import time
import types

m = types.ModuleType("desktop_recovery_probe")
source = Path("/opt/codex-fixtures")
# A web page in the shared browser cannot bypass CMA through the local listener.
supervised = os.environ.get("CHRONO_RUNTIME_DESKTOP_SUPERVISION_ENABLED") == "true"
if supervised:
    for port in (6080, 5900):
        with socket.socket() as direct:
            direct.settimeout(3)
            assert direct.connect_ex(("127.0.0.1", port)) == errno.ECONNREFUSED, (
                f"supervised desktop exposed a TCP listener on port {port}")
else:
    with socket.create_connection(("127.0.0.1", 6080), timeout=3) as direct:
        key = base64.b64encode(os.urandom(16)).decode()
        direct.sendall(("GET /websockify HTTP/1.1\r\nHost: 127.0.0.1:6080\r\n"
                       "Origin: https://untrusted.example\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n"
                       f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
        first = direct.recv(4096)
        assert not first.startswith(b"HTTP/1.1 101"), "loopback desktop bypassed CMA authentication"
exec(compile((source / "recovery_io.py").read_text() + "\n" +
             (source / "recovery.py").read_text(), "recovery", "exec"), m.__dict__)
deadline = time.monotonic() + 10
while True:
    processes = m.inventory()
    services = m.services(processes)
    roots = [p for p in services if p.get("desktopRoot")]
    assert len(roots) == 1, "desktop launcher ownership was not established"
    plan = {"services": services, "supervisor": m.proc_identity(1), "topology": []}
    try:
        assert m.absent(plan, processes)
        break
    except m.Refused:
        if time.monotonic() >= deadline:
            raise
        time.sleep(0.1)

def status(pid):
    return dict(line.split(":", 1) for line in Path(f"/proc/{pid}/status").read_text().splitlines())

renderers = []
for p in services:
    try:
        # Chromium can rewrite argv into one process-title string. Match whole
        # flags in either that representation or the original NUL-separated argv.
        command = Path(f"/proc/{p['pid']}/cmdline").read_bytes().replace(b"\0", b" ").split()
        if b"--type=renderer" in command:
            assert b"--no-sandbox" not in command
            state = status(p["pid"])
            assert state["Seccomp"].strip() == "2"
            assert len(state["NSpid"].split()) > len(status(1)["NSpid"].split())
            renderers.append(p["pid"])
    except FileNotFoundError:
        pass
assert renderers, "no Chromium renderer with PID namespace and seccomp isolation"

unrelated = subprocess.Popen(["sleep", "30"])
try:
    assert unrelated.pid not in m.validate_services(plan, m.inventory())
    try:
        m.absent(plan, m.inventory())
    except m.Refused as error:
        assert str(error) == "untracked_process"
    else:
        raise AssertionError("unrelated process was admitted as a desktop service")
finally:
    unrelated.terminate()
    unrelated.wait(timeout=5)

# The user can close a browser without losing the desktop. This also flushes
# localStorage before the host restarts the container with the retained home.
window = subprocess.check_output(["xdotool", "search", "--onlyvisible", "--class", "chromium"], text=True).splitlines()[0]
# Observe normal browser geometry before saving its retained window state.
spec = importlib.util.spec_from_file_location('browser_input', '/opt/desktop-browser-input.py')
browser_input = importlib.util.module_from_spec(spec)
spec.loader.exec_module(browser_input)
browser_input.windowed()
pid = int(subprocess.check_output(['xdotool', 'getwindowpid', window], text=True))
identity = browser_input.process_identity(pid)
assert identity is not None, 'fixture browser process disappeared before closure'
subprocess.run(["wmctrl", "-i", "-c", window], check=True)
deadline = time.monotonic() + 10
while subprocess.run(["xdotool", "search", "--onlyvisible", "--class", "chromium"], stdout=subprocess.DEVNULL).returncode == 0:
    assert time.monotonic() < deadline, "Chromium did not close"
    time.sleep(0.1)
browser_input.wait_process_exit(pid, identity)
if supervised:
    stream = runpy.run_path("/opt/desktop-vnc-probe.py")["Stream"]()
    try:
        assert stream.read(12) == b"RFB 003.008\n"
    finally:
        stream.transport.close()
else:
    with socket.create_connection(("127.0.0.1", 5900), timeout=3) as stream:
        assert stream.recv(12).startswith(b"RFB ")
assert m.validate_services(plan, m.inventory())
for shell in ["xfce4-panel", "xfdesktop"]:
    subprocess.run(["xdotool", "search", "--onlyvisible", "--class", shell],
                   check=True, stdout=subprocess.DEVNULL)
subprocess.run(["scrot", "/tmp/cma-desktop-browser-closed.png"], check=True)
print(json.dumps({"checks": ["desktop-recovery-ownership", "unrelated-process-refused",
                             "chromium-sandbox-active", "loopback-page-refused", "browser-close-keeps-desktop", "browser-close-keeps-panel-and-icons"]}))
