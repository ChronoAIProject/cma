"""Read the active native Wayland window without substituting an X11 browser."""
import json
import os
from pathlib import Path
import subprocess
import sys

os.environ['XDG_RUNTIME_DIR'] = '/tmp/cma-desktop-runtime'
os.environ['HYPRLAND_INSTANCE_SIGNATURE'] = Path('/tmp/cma-desktop-runtime/cma-hyprland-instance').read_text().strip()
value = json.loads(subprocess.check_output(['hyprctl', '-j', 'activewindow']))
# --fullscreen prints Hyprland's internal fullscreen mode: 0 is a normal window.
field = 'class' if '--class' in sys.argv else 'fullscreen' if '--fullscreen' in sys.argv else 'title'
print(value.get(field, ''))
