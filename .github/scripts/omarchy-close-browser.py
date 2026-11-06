"""Close the actual Wayland browser through Omarchy and await profile flush."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--normal-window', action='store_true',
                    help='The startup browser has not entered fullscreen yet')
options = parser.parse_args()

os.environ['XDG_RUNTIME_DIR'] = '/tmp/cma-desktop-runtime'
os.environ['HYPRLAND_INSTANCE_SIGNATURE'] = Path(
    '/tmp/cma-desktop-runtime/cma-hyprland-instance').read_text().strip()


def browser_windows():
    clients = json.loads(subprocess.check_output(['hyprctl', '-j', 'clients']))
    return [client for client in clients if 'chromium' in client['class'].lower()]


windows = browser_windows()
assert len(windows) == 1, 'Expected one fixture browser window to close'
# The terminal probe may have owned focus. Select the actual Wayland browser,
# independently of the native tool shell's HOME or Chromium's argv[0]. Hyprland
# 0.56.2 with our Lua config wraps dispatch arguments in hl.dispatch(...).
focus = 'hl.dsp.focus({window = ' + json.dumps('address:' + windows[0]['address']) + '})'
subprocess.run(['hyprctl', 'dispatch', focus], check=True)
active = json.loads(subprocess.check_output(['hyprctl', '-j', 'activewindow']))
assert active.get('address') == windows[0]['address'], 'Fixture browser did not gain focus'
viewer = subprocess.check_output(['pgrep', '-x', 'vncviewer'], text=True).strip()
window = subprocess.check_output(
    ['xdotool', 'search', '--onlyvisible', '--pid', viewer], text=True).splitlines()[0]
subprocess.run(['xdotool', 'windowfocus', '--sync', window], check=True)
# The native input probe entered browser fullscreen. Restore normal placement
# before closing so the retained profile starts in the same ordinary state.
if not options.normal_window:
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
