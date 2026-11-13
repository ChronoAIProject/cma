"""Close fixture Chromium before testing the real native shell reopen command."""
import os
from pathlib import Path
import signal
import time

argument = os.fsencode('--user-data-dir=' + str(Path.home() / '.cma/chrome'))
pids = []
for entry in Path('/proc').glob('[0-9]*/cmdline'):
    try:
        args = entry.read_bytes().split(b'\0')
        if argument in args and b'chrom' in args[0] and not any(a.startswith(b'--type=') for a in args):
            pid = int(entry.parent.name)
            os.kill(pid, signal.SIGTERM)
            pids.append(pid)
    except (FileNotFoundError, ProcessLookupError):
        pass
assert pids, 'fixture browser was not running before reopen'
def running(pid):
    try:
        return bool(Path(f'/proc/{pid}/cmdline').read_bytes())
    except FileNotFoundError:
        return False


deadline = time.monotonic() + 10
while any(running(pid) for pid in pids):
    assert time.monotonic() < deadline, 'fixture browser did not close before reopen'
    time.sleep(.1)
