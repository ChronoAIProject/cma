"""Close the actual Wayland browser through Omarchy and await profile flush."""
import json
import os
from pathlib import Path
import subprocess
import time

os.environ['HYPRLAND_INSTANCE_SIGNATURE'] = Path(
    '/tmp/cma-desktop-runtime/cma-hyprland-instance').read_text().strip()


def browser_windows():
    clients = json.loads(subprocess.check_output(['hyprctl', '-j', 'clients']))
    return [client for client in clients if 'chromium' in client['class'].lower()]


assert browser_windows(), 'No browser window to close'
viewer = subprocess.check_output(['pgrep', '-x', 'vncviewer'], text=True).strip()
window = subprocess.check_output(
    ['xdotool', 'search', '--onlyvisible', '--pid', viewer], text=True).splitlines()[0]
subprocess.run(['xdotool', 'windowfocus', '--sync', window], check=True)
# The native input probe entered browser fullscreen. Restore normal placement
# before closing so the retained profile starts in the same ordinary state.
subprocess.run(['xdotool', 'key', '--clearmodifiers', 'F11'], check=True)
time.sleep(0.3)
# Omarchy binds SUPER+W to a normal xdg_toplevel.close request.
subprocess.run(['xdotool', 'key', '--clearmodifiers', 'super+w'], check=True)
deadline = time.monotonic() + 10
while browser_windows():
    assert time.monotonic() < deadline, 'Chromium window did not close'
    time.sleep(0.1)

# Window disappearance alone does not establish a clean browser/profile exit.
argument = b'--user-data-dir=/home/cma/.cma/chrome'
while True:
    live = False
    for path in Path('/proc').glob('[0-9]*/cmdline'):
        try:
            args = path.read_bytes().split(b'\0')
        except (FileNotFoundError, ProcessLookupError):
            continue
        if argument in args and not any(arg.startswith(b'--type=') for arg in args):
            live = True
    if not live:
        break
    assert time.monotonic() < deadline, 'Chromium did not finish closing its profile'
    time.sleep(0.1)
print('browser-close-and-profile-exit: pass')
