"""Exercise the real XFCE shell and retained preferences in the image fixture."""
import json
import os
from pathlib import Path
import subprocess
import time


def windows(name):
    result = subprocess.run(["xdotool", "search", "--onlyvisible", "--class", name],
                            text=True, capture_output=True)
    return result.stdout.splitlines() if result.returncode == 0 else []


def wait_window(name):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        found = windows(name)
        if found:
            return found[0]
        time.sleep(0.1)
    raise AssertionError(name + " did not become visible")


def run(*args):
    return subprocess.check_output(args, env=environment, text=True).strip()


session = []
for path in Path('/proc').iterdir():
    try:
        if path.name.isdigit() and (path / 'comm').read_text().strip() == 'xfce4-session':
            session.append(path)
    except FileNotFoundError:
        pass
assert len(session) == 1, 'one XFCE session must own the desktop'
environment = dict(os.environ)
for entry in (session[0] / 'environ').read_bytes().split(b'\0'):
    key, _, value = entry.partition(b'=')
    if key in (b'DBUS_SESSION_BUS_ADDRESS', b'XDG_RUNTIME_DIR'):
        environment[key.decode()] = value.decode()
assert 'DBUS_SESSION_BUS_ADDRESS' in environment
panel = wait_window('xfce4-panel')
desktop = wait_window('xfdesktop')
assert 'Name: Xfwm4' in run('wmctrl', '-m')
# The panel must be a visible dock and the background a real desktop window.
assert '_NET_WM_WINDOW_TYPE_DOCK' in run('xprop', '-id', panel, '_NET_WM_WINDOW_TYPE')
assert '_NET_WM_WINDOW_TYPE_DESKTOP' in run('xprop', '-id', desktop, '_NET_WM_WINDOW_TYPE')
clock = ['xfconf-query', '-c', 'xfce4-panel', '-p', '/plugins/plugin-7/digital-format']
expected = '%a %H:%M CMA'
if os.environ.get('CMA_DESKTOP_EXPECT_RETAINED') == '1':
    assert run(*clock) == expected, 'desktop customization was replaced on restart'
else:
    run(*clock, '-s', expected)
    deadline = time.monotonic() + 10
    config = Path.home() / '.config/xfce4/xfconf/xfce-perchannel-xml/xfce4-panel.xml'
    while expected not in config.read_text():
        assert time.monotonic() < deadline, 'XFCE preference did not reach retained home'
        time.sleep(0.1)

# Launch actual desktop applications, then close their windows before the
# independent recovery probe inventories the remaining desktop service tree.
folder = Path.home() / 'Documents'
assert folder.is_dir() and (Path.home() / 'Desktop').is_dir()
(folder / 'CMA desktop.txt').write_text('CMA desktop file manager proof\n')
run('thunar', str(folder))
file_window = wait_window('Thunar')
run('scrot', '/tmp/cma-desktop-file-manager.png')
run('wmctrl', '-i', '-c', file_window)
terminal = subprocess.Popen(['xfce4-terminal', '--disable-server', '--title=CMA desktop terminal'],
                            env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
try:
    terminal_window = wait_window('Xfce4-terminal')
    run('wmctrl', '-i', '-c', terminal_window)
    assert terminal.wait(timeout=10) == 0
finally:
    if terminal.poll() is None:
        terminal.terminate()
        terminal.wait(timeout=5)
run('scrot', '/tmp/cma-desktop-shell.png')
print(json.dumps({'checks': ['xfce-window-manager', 'visible-taskbar', 'desktop-icons-surface',
                             'file-manager-window', 'terminal-window', 'retained-desktop-preferences']}))
