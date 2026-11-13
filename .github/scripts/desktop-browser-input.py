"""Drive Debian Chromium only after bounded, observable UI acknowledgements."""
import argparse
from pathlib import Path
import re
import subprocess
import time

TIMEOUT = 5
VIEWPORT = (1280, 800)


def output(*args):
    return subprocess.check_output(args, text=True, timeout=1).strip()


def run(*args):
    subprocess.run(['xdotool', *args], check=True, timeout=3)


class Browser:
    def __init__(self):
        windows = output('xdotool', 'search', '--onlyvisible', '--class', 'chromium').splitlines()
        # Keep the fixture's existing first-visible-window selection, then bind
        # every acknowledgement to that exact window, even if another is mapped.
        assert windows and windows[0].isdigit(), 'fixture Chromium window is not visible'
        self.window = windows[0]
        run('windowactivate', '--sync', self.window)

    def observe(self):
        active = output('xdotool', 'getactivewindow') == self.window
        title = output('xdotool', 'getwindowname', self.window)
        state = output('xprop', '-id', self.window, '_NET_WM_STATE')
        match = re.fullmatch(r'(.*?)\|(\d+)x(\d+) - Chromium', title)
        return {
            'active': active, 'title': title,
            'stage': match[1] if match else None,
            'viewport': (int(match[2]), int(match[3])) if match else None,
            'fullscreen': '_NET_WM_STATE_FULLSCREEN' in
                [value.strip() for value in state.split('=', 1)[-1].split(',')],
        }

    def wait(self, stage, predicate):
        deadline = time.monotonic() + TIMEOUT
        observed = None
        while True:
            try:
                observed = self.observe()
                if observed['active'] and predicate(observed):
                    return observed
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
                observed = type(error).__name__
            if time.monotonic() >= deadline:
                raise AssertionError(f'Timed out waiting for {stage}; observed {observed!r}')
            time.sleep(0.1)


def fullscreen(observed):
    return observed['fullscreen'] and observed['viewport'] == VIEWPORT


def main(origin):
    browser = Browser()
    browser.wait('initial page cma-desktop-ready', lambda o: o['stage'] == 'cma-desktop-ready')
    run('key', '--clearmodifiers', '--delay', '25', 'ctrl+l')
    run('type', '--clearmodifiers', '--delay', '25', origin + '/desktop-input')
    run('key', '--clearmodifiers', 'Return')
    navigated = browser.wait('navigation to cma-desktop-input-ready',
                             lambda o: o['stage'] == 'cma-desktop-input-ready')
    # F11 toggles: a retained fullscreen window must not be toggled out of it.
    if not navigated['fullscreen']:
        run('key', '--clearmodifiers', 'F11')
    browser.wait('fullscreen viewport 1280x800',
                 lambda o: o['stage'] == 'cma-desktop-input-ready' and fullscreen(o))
    run('mousemove', '120', '120', 'click', '1')
    browser.wait('input focus', lambda o: o['stage'] == 'cma-input-focused' and fullscreen(o))
    run('type', '--clearmodifiers', '--delay', '25', 'cma-codex')
    browser.wait('exact input cma-codex', lambda o: o['stage'] == 'cma-input:cma-codex' and fullscreen(o))
    run('mousemove', '120', '220', 'click', '1')
    browser.wait('button acknowledgement cma-ui:cma-codex',
                 lambda o: o['stage'] == 'cma-ui:cma-codex' and fullscreen(o))
    print('browser-navigation-fullscreen-and-input: pass')


def windowed():
    browser = Browser()
    observed = browser.wait('completed shared input cma-ui:cma-user',
                            lambda o: o['stage'] == 'cma-ui:cma-user')
    if observed['fullscreen']:
        run('key', '--clearmodifiers', 'F11')
    browser.wait('normal browser viewport before closing', lambda o:
                 o['stage'] == 'cma-ui:cma-user' and not o['fullscreen']
                 and o['viewport'] is not None and 0 < o['viewport'][0] <= VIEWPORT[0]
                 and 0 < o['viewport'][1] < VIEWPORT[1])


def process_identity(pid):
    try:
        fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
        return fields[19] if fields[0] not in ('Z', 'X') else None
    except (FileNotFoundError, ProcessLookupError):
        return None


def wait_process_exit(pid, identity):
    deadline = time.monotonic() + TIMEOUT
    while process_identity(pid) == identity:
        if time.monotonic() >= deadline:
            raise AssertionError('Timed out waiting for closed browser process to exit and flush its profile')
        time.sleep(0.1)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('origin', nargs='?')
    parser.add_argument('--windowed', action='store_true')
    args = parser.parse_args()
    if args.windowed:
        windowed()
    else:
        assert args.origin, 'fixture origin required'
        main(args.origin)
