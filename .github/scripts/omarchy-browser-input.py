"""Drive the real desktop, waiting for page acknowledgements between actions."""
import subprocess
import sys
import time


def run(*args):
    subprocess.run(['xdotool', *args], check=True)


def wait_title(expected):
    deadline = time.monotonic() + 5
    while True:
        title = subprocess.check_output(
            ['python3', '/opt/probes/omarchy-active-title.py'], text=True).strip()
        if title == expected + ' - Chromium':
            return
        if time.monotonic() >= deadline:
            raise AssertionError(f'Expected browser acknowledgement {expected!r}, got {title!r}')
        time.sleep(0.1)


def main(origin):
    # A mapped Chromium window is not yet a loaded page. Wait for the initial
    # launcher URL before touching its address bar; the next navigation has a
    # different title so the first page cannot acknowledge the second load.
    wait_title('cma-desktop-ready')
    run('key', '--clearmodifiers', '--delay', '25', 'ctrl+l')
    run('type', '--clearmodifiers', '--delay', '25', origin + '/desktop-input')
    run('key', '--clearmodifiers', 'Return')
    wait_title('cma-desktop-input-ready')
    # Require Chromium's viewport, not just the window manager, to fill the
    # streamed 1280x800 display before using page-relative screen coordinates.
    run('key', '--clearmodifiers', 'F11')
    wait_title('cma-desktop-fullscreen')
    run('mousemove', '120', '120', 'click', '1')
    wait_title('cma-input-focused')
    run('type', '--clearmodifiers', '--delay', '25', 'cma-codex')
    wait_title('cma-input:cma-codex')
    run('mousemove', '120', '220', 'click', '1')
    wait_title('cma-ui:cma-codex')
    print('browser-navigation-fullscreen-and-input: pass')


if __name__ == '__main__':
    main(sys.argv[1])
