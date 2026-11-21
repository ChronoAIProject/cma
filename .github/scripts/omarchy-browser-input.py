"""Drive the real desktop, waiting for page acknowledgements between actions.

Input reaches Chromium through Xvfb, the VNC viewer and the compositor, and
under runner load events can be lost. A stage whose acknowledgement is still
missing after the full first wait, where a single send would already have
failed, re-sends its input a bounded number of times. A late but working first
send is never repeated. Every re-send has the effect of one send: navigation
and typing replace rather than append, clicks repeat the same handler, and F11,
which toggles, is re-sent only while the compositor still reports a normal
window. Every run proves that compositor query before and after F11.
"""
import subprocess
import sys
import time

ACTIVE = ['python3', '/opt/probes/omarchy-active-title.py']
READY_SECONDS = 5
FIRST_ATTEMPT_SECONDS = 5
RETRY_SECONDS = 2.5
ATTEMPTS = 3
# Hyprland's internal fullscreen mode is a bit set: 1 maximized, 2 fullscreen.
FULLSCREEN = 2


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


def acknowledge(expected, send, resend=None, withhold=None):
    """Send until acknowledged; return how many re-sends that needed.

    withhold, when given, returns why a re-send would be unsafe, or None.
    """
    started = time.monotonic()
    sent = 0
    withheld = None
    for attempt in range(ATTEMPTS):
        if attempt == 0:
            send()
            sent += 1
        elif withhold is None or (withheld := withhold()) is None:
            (resend or send)()
            sent += 1
        title = observe(expected, FIRST_ATTEMPT_SECONDS if attempt == 0 else RETRY_SECONDS)
        if title == expected + ' - Chromium':
            return sent - 1
    reason = f'; {withheld}' if withheld else ''
    raise AssertionError(
        f'Expected browser acknowledgement {expected!r} after {sent} of {ATTEMPTS} input '
        f'attempts in {time.monotonic() - started:.1f}s; last title {title!r}{reason}')


def navigate(origin):
    # ctrl+l selects the whole address bar, so a re-send replaces any partial URL.
    run('key', '--clearmodifiers', '--delay', '25', 'ctrl+l')
    run('type', '--clearmodifiers', '--delay', '25', origin + '/desktop-input')
    run('key', '--clearmodifiers', 'Return')


def fullscreen_mode():
    """Read the active window's mode from Hyprland, failing if it is not an integer."""
    value = subprocess.check_output([*ACTIVE, '--fullscreen'], text=True).strip()
    if not value.isdigit():
        raise AssertionError(f'Hyprland reported no integer fullscreen mode: {value!r}')
    return int(value)


def f11_withheld():
    # A late first F11 may already have made the window fullscreen; another
    # would toggle it back out.
    mode = fullscreen_mode()
    return f'window already fullscreen (mode {mode}); F11 not re-sent' if mode & FULLSCREEN else None


def retype():
    # Replace whatever part of the value arrived instead of appending to it.
    run('key', '--clearmodifiers', 'ctrl+a')
    run('type', '--clearmodifiers', '--delay', '25', 'cma-codex')


def main(origin):
    resent = {}
    # A mapped Chromium window is not yet a loaded page. Wait for the initial
    # launcher URL before touching its address bar; the next navigation has a
    # different title so the first page cannot acknowledge the second load.
    wait_title('cma-desktop-ready')
    resent['navigation'] = acknowledge('cma-desktop-input-ready', lambda: navigate(origin))
    # The F11 re-send guard reads Hyprland's state; prove that query on every run.
    mode = fullscreen_mode()
    if mode & FULLSCREEN:
        raise AssertionError(f'Browser was already fullscreen (mode {mode}) before F11')
    # Require Chromium's viewport, not just the window manager, to fill the
    # streamed 1280x800 display before using page-relative screen coordinates.
    resent['fullscreen'] = acknowledge('cma-desktop-fullscreen',
                                       lambda: run('key', '--clearmodifiers', 'F11'),
                                       withhold=f11_withheld)
    mode = fullscreen_mode()
    if not mode & FULLSCREEN:
        raise AssertionError(f'Hyprland reported fullscreen mode {mode} after the fullscreen acknowledgement')
    resent['focus'] = acknowledge('cma-input-focused', lambda: run('mousemove', '120', '120', 'click', '1'))
    resent['typing'] = acknowledge('cma-input:cma-codex',
                                   lambda: run('type', '--clearmodifiers', '--delay', '25', 'cma-codex'), retype)
    resent['verify'] = acknowledge('cma-ui:cma-codex', lambda: run('mousemove', '120', '220', 'click', '1'))
    # Keep repeated input loss visible in the evidence even when it recovers.
    counts = ', '.join(f'{stage}={count}' for stage, count in resent.items() if count) or 'none'
    print(f'browser-navigation-fullscreen-and-input: pass (resent: {counts})')


if __name__ == '__main__':
    main(sys.argv[1])
