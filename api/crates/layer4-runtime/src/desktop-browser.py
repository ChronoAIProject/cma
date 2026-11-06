#!/usr/bin/env python3
"""Desktop-owned Chromium launcher. Exit: 0 ready, 64 input, 69 owner, 70 start, 124 timeout."""
import ctypes
import fcntl
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time

RUNTIME = Path('/tmp/cma-desktop-runtime')
PROFILE = Path.home() / '.cma/chrome'
TIMEOUT = 20


def identity(pid):
    try:
        fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
        return fields[19] if fields[0] not in ('Z', 'X') else None
    except (FileNotFoundError, ProcessLookupError):
        return None


def profile_browser(cmdline):
    argument = os.fsencode('--user-data-dir=' + str(PROFILE))
    args = cmdline.rstrip(b'\0').split(b'\0')
    if len(args) == 1:
        # Chromium's Linux SetProcessTitleFromCommandLine replaces argv with
        # one space-separated title. Match whole flags, including a profile
        # path containing spaces; a renderer's title must never become an owner.
        title = b' ' + args[0] + b' '
        return (b'chrom' in args[0].split(b' ', 1)[0]
                and b' ' + argument + b' ' in title and b' --type=' not in title)
    return (argument in args and b'chrom' in args[0]
            and not any(a.startswith(b'--type=') for a in args))


def browsers():
    found = []
    for entry in Path('/proc').iterdir():
        if not entry.name.isdigit():
            continue
        try:
            if (profile_browser((entry / 'cmdline').read_bytes())
                    and identity(int(entry.name))):
                found.append(int(entry.name))
        except (FileNotFoundError, ProcessLookupError):
            pass
    return found


def run(args, deadline, env):
    return subprocess.run(args, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                          stderr=subprocess.DEVNULL, close_fds=True, env=env,
                          timeout=max(.01, min(1, deadline - time.monotonic())),
                          text=True)


class Manager:
    def __init__(self, owner, wayland, timeout=TIMEOUT):
        self.owner, self.owner_identity = owner, identity(owner)
        self.wayland, self.timeout = wayland, timeout
        self.children = []
        self.stopping = False
        self.env = dict(os.environ, DISPLAY=':99', XDG_RUNTIME_DIR=str(RUNTIME))
        self.env.pop('CMA_DESKTOP_TOKEN', None)
        if wayland:
            self.env['WAYLAND_DISPLAY'] = 'wayland-1'

    def alive(self):
        return (not self.stopping and self.owner_identity is not None
                and identity(self.owner) == self.owner_identity)

    def window(self, pid, deadline, focus=False):
        try:
            if self.wayland:
                instance = (RUNTIME / 'cma-hyprland-instance').read_text().strip()
                command = ['hyprctl', '-i', instance]
                result = run(command + ['-j', 'clients'], deadline, self.env)
                windows = [w for w in json.loads(result.stdout)
                           if w.get('pid') == pid and 'chrom' in w.get('class', '').lower()]
                if windows and focus:
                    # Omarchy's pinned Hyprland uses Lua dispatcher expressions.
                    selector = json.dumps('address:' + windows[0]['address'])
                    run(command + ['dispatch', 'hl.dsp.focus({window = ' + selector + '})'],
                        deadline, self.env)
            else:
                result = run(['xdotool', 'search', '--all', '--onlyvisible', '--pid', str(pid),
                              '--class', 'chromium'], deadline, self.env)
                windows = result.stdout.splitlines() if result.returncode == 0 else []
                if windows and focus:
                    run(['xdotool', 'windowactivate', '--sync', windows[0]], deadline, self.env)
            return bool(windows)
        except (OSError, ValueError, subprocess.TimeoutExpired):
            return False

    def spawn(self, args):
        command = ['chromium']
        if self.wayland:
            command.append('--ozone-platform=wayland')
        command += ['--user-data-dir=' + str(PROFILE), '--no-first-run',
                    '--disable-gpu', '--disable-dev-shm-usage', *args]
        with open('/tmp/cma-chromium.log', 'ab', buffering=0) as log:
            # start_new_session invokes setsid; close_fds drops every inherited
            # tool/desktop descriptor, including the desktop's flock descriptor.
            child = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=log,
                                     stderr=log, close_fds=True, start_new_session=True,
                                     env=self.env)
        self.children.append(child)
        return child

    def stop_child(self, child):
        # Do not reap before signaling: the retained PID reserves this group ID.
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait(timeout=2)
        self.children.remove(child)

    def launch(self, args):
        deadline = time.monotonic() + self.timeout
        existing = browsers()
        if len(existing) > 1:
            return 70, 'multiple browsers own the desktop profile'
        if (existing and not args and self.window(existing[0], deadline, focus=True)
                and self.alive() and identity(existing[0])):
            return 0, 'desktop browser ready (reused)'
        try:
            # Chromium's singleton protocol delivers URLs to the existing profile.
            child = self.spawn(args or ['--new-window', 'about:blank'])
        except OSError:
            return 70, 'desktop browser failed to start'
        while time.monotonic() < deadline:
            if not self.alive():
                self.stop_child(child)
                return 69, 'desktop session stopped'
            owners = browsers()
            # waitid observes exit without reaping, preserving the group for cleanup.
            exited = os.waitid(os.P_PID, child.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
            if exited and (exited.si_status != 0 or not owners):
                self.stop_child(child)
                return 70, 'desktop browser exited before readiness'
            if (not existing or exited) and any(
                    self.window(pid, deadline) and identity(pid) for pid in owners):
                return 0, 'desktop browser ready'
            time.sleep(.1)
        self.stop_child(child)
        return 124, 'desktop browser readiness timed out'

    def reap(self):
        for child in self.children[:]:
            if os.waitid(os.P_PID, child.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT):
                self.stop_child(child)

    def close(self):
        # Give Chromium a short chance to flush its profile on desktop shutdown.
        for child in self.children:
            try:
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        deadline = time.monotonic() + 1
        while any(identity(child.pid) for child in self.children) and time.monotonic() < deadline:
            time.sleep(.05)
        for child in self.children[:]:
            self.stop_child(child)
        # Subreaper adoption includes helpers that escaped Chromium's group.
        # Kill without reaping first so no PID can be reused before signaling.
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            children = Path(f'/proc/self/task/{os.getpid()}/children').read_text().split()
            if not children:
                return
            for pid in children:
                try:
                    os.kill(int(pid), signal.SIGKILL)
                except ProcessLookupError:
                    pass
            while True:
                try:
                    if os.waitpid(-1, os.WNOHANG)[0] == 0:
                        break
                except ChildProcessError:
                    break
            time.sleep(.01)


def packet(connection):
    data = bytearray()
    while len(data) <= 65536:
        chunk = connection.recv(4096)
        if not chunk:
            break
        data.extend(chunk)
        if b'\n' in chunk:
            return json.loads(data)
    raise ValueError('invalid launch request')


def serve(owner, wayland):
    # This manager remains a desktop descendant for recovery/writer fencing, but
    # survives the runtime's group SIGKILL long enough to clean detached browsers.
    os.setsid()
    for entry in Path('/proc/self/fd').iterdir():
        if int(entry.name) > 2:
            try:
                os.close(int(entry.name))
            except OSError:
                pass
    # Adopt crash handlers/helpers that double-fork or create their own session.
    if ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), 'cannot establish browser ownership')
    manager = Manager(owner, wayland)
    def stop(_signum, _frame):
        manager.stopping = True
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    path = RUNTIME / 'browser.sock'
    os.umask(0o077)
    lock = open(RUNTIME / 'browser.lock', 'w')
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        lock.close()
        return
    try:
        path.unlink(missing_ok=True)
        with socket.socket(socket.AF_UNIX) as listener:
            listener.bind(str(path))
            listener.listen(16)
            listener.settimeout(.1)
            while manager.alive():
                manager.reap()
                try:
                    connection, _ = listener.accept()
                except socket.timeout:
                    continue
                with connection:
                    connection.settimeout(1)
                    try:
                        args = packet(connection)
                        if not isinstance(args, list) or not all(isinstance(a, str) and '\0' not in a for a in args):
                            raise ValueError('invalid launch arguments')
                        # Profile/platform are owned by this desktop, never callers.
                        if any(a.startswith(('--user-data-dir', '--ozone-platform')) for a in args):
                            raise ValueError('desktop profile/platform cannot be overridden')
                        result = manager.launch(args)
                    except (ValueError, OSError):
                        result = (64, 'invalid desktop browser request')
                    try:
                        connection.sendall(json.dumps(result).encode() + b'\n')
                    except (BrokenPipeError, ConnectionResetError):
                        pass
    finally:
        manager.close()
        path.unlink(missing_ok=True)
        lock.close()


def request(args):
    deadline = time.monotonic() + TIMEOUT + 3
    while True:
        try:
            with socket.socket(socket.AF_UNIX) as connection:
                connection.settimeout(max(.01, deadline - time.monotonic()))
                connection.connect(str(RUNTIME / 'browser.sock'))
                connection.sendall(json.dumps(args).encode() + b'\n')
                code, message = packet(connection)
                print(message, file=sys.stdout if code == 0 else sys.stderr)
                return code
        except (FileNotFoundError, ConnectionRefusedError):
            if time.monotonic() >= deadline:
                print('desktop browser manager unavailable', file=sys.stderr)
                return 69
            time.sleep(.1)
        except TimeoutError:
            print('desktop browser request timed out', file=sys.stderr)
            return 124
        except (OSError, ValueError):
            print('desktop browser request failed', file=sys.stderr)
            return 69


if __name__ == '__main__':
    if len(sys.argv) >= 3 and sys.argv[1] == '--serve':
        serve(int(sys.argv[2]), '--wayland' in sys.argv[3:])
    else:
        sys.exit(request(sys.argv[1:]))
