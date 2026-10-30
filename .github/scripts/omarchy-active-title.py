"""Read the active native Wayland window without substituting an X11 browser."""
import json
import os
from pathlib import Path
import subprocess

os.environ['XDG_RUNTIME_DIR'] = '/tmp/cma-desktop-runtime'
os.environ['HYPRLAND_INSTANCE_SIGNATURE'] = Path('/tmp/cma-desktop-runtime/cma-hyprland-instance').read_text().strip()
value = json.loads(subprocess.check_output(['hyprctl', '-j', 'activewindow']))
print(value.get('title', ''))
