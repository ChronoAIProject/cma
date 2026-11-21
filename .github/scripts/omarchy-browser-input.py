"""Drive the real desktop, waiting for page acknowledgements between actions.

Input reaches Chromium through Xvfb, the VNC viewer and the compositor, and
under runner load one event can be dropped. Each stage therefore re-sends its
input, a bounded number of times, while its exact acknowledgement is missing.
Every re-send has the effect of one send: navigation and typing replace rather
than append, clicks repeat the same handler, and F11, which toggles, is re-sent
only while the compositor still reports a normal window.
"""
import subprocess
import sys
import time

ACTIVE = ['python3', '/opt/probes/omarchy-active-title.py']
READY_SECONDS = 5
ATTEMPTS = 3
ATTEMPT_SECONDS = 2.5


def run(*args):
    subprocess.run(['xdotool', *args], check=True)


def observe(expected, seconds):
    """Return the title once it is the acknowledgement, or the last one seen."""
    deadline = time.monotonic() + seconds
    while True:
        title = subprocess.check_output(ACTIVE, text=True).strip()
        if title == expected + ' - Chromium' or time.monotonic() >= deadline:
            return title
        time.sleep(0.1)


def wait_title(expected):
    title = observe(expected, READY_SECONDS)
    if title != expected + ' - Chromium':
        raise AssertionError(f'Expected browser acknowledgement {expected!r}, got {title!r}')


def acknowledge(expected, send, resend=None, may_resend=lambda: True):
    started = time.monotonic()
    sent = 0
    for attempt in range(ATTEMPTS):
        if attempt == 0:
            send()
            sent += 1
        elif may_resend():
            (resend or send)()
            sent += 1
        title = observe(expected, ATTEMPT_SECONDS)
        if title == expected + ' - Chromium':
            return
    raise AssertionError(
        f'Expected browser acknowledgement {expected!r} after {sent} of {ATTEMPTS} input '
        f'attempts in {time.monotonic() - started:.1f}s; last title {title!r}')


def navigate(origin):
    # ctrl+l selects the whole address bar, so a re-send replaces any partial URL.
    run('key', '--clearmodifiers', '--delay', '25', 'ctrl+l')
    run('type', '--clearmodifiers', '--delay', '25', origin + '/desktop-input')
    run('key', '--clearmodifiers', 'Return')


def windowed():
    # A late first F11 has already made the window fullscreen; another would
    # toggle it back out. Hyprland reports internal fullscreen mode 0 otherwise.
    return subprocess.check_output([*ACTIVE, '--fullscreen'], text=True).strip() == '0'


def retype():
    # Replace whatever part of the value arrived instead of appending to it.
    run('key', '--clearmodifiers', 'ctrl+a')
    run('type', '--clearmodifiers', '--delay', '25', 'cma-codex')


def main(origin):
    # A mapped Chromium window is not yet a loaded page. Wait for the initial
    # launcher URL before touching its address bar; the next navigation has a
    # different title so the first page cannot acknowledge the second load.
    wait_title('cma-desktop-ready')
    acknowledge('cma-desktop-input-ready', lambda: navigate(origin))
    # Require Chromium's viewport, not just the window manager, to fill the
    # streamed 1280x800 display before using page-relative screen coordinates.
    acknowledge('cma-desktop-fullscreen', lambda: run('key', '--clearmodifiers', 'F11'),
                may_resend=windowed)
    acknowledge('cma-input-focused', lambda: run('mousemove', '120', '120', 'click', '1'))
    acknowledge('cma-input:cma-codex',
                lambda: run('type', '--clearmodifiers', '--delay', '25', 'cma-codex'), retype)
    acknowledge('cma-ui:cma-codex', lambda: run('mousemove', '120', '220', 'click', '1'))
    print('browser-navigation-fullscreen-and-input: pass')


if __name__ == '__main__':
    main(sys.argv[1])
