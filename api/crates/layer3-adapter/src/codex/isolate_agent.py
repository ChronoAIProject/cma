"""Whole-agent boundary, embedded in the signed launch rather than writable files.

The host process exposes three fixed credential proxies through private Unix
sockets. The provider runs in a new network namespace with only matching local
listeners. Neither the runtime control port nor arbitrary host destinations are
reachable. This deliberately does not provide direct Internet access to shell
commands; model and NyxID service tools use the existing fixed proxies.
"""
import contextlib
import os
import pathlib
import selectors
import signal
import socket
import socketserver
import subprocess
import sys
import tempfile
import threading


PROXIES = (
    ("model", "127.0.0.1", 9091),
    ("platform", "127.0.0.2", 9092),
    ("tools", "127.0.0.1", 9093),
)
BWRAP = "/usr/bin/bwrap"
PYTHON = "/usr/bin/python3"
SOCKET_ROOT = "/run/cma-agent-proxies"


def transfer(client, upstream):
    """Bounded duplex stream; no parsing permits a client-selected destination."""
    with selectors.DefaultSelector() as poll:
        poll.register(client, selectors.EVENT_READ, upstream)
        poll.register(upstream, selectors.EVENT_READ, client)
        while poll.get_map():
            for key, _ in poll.select():
                data = key.fileobj.recv(65536)
                if data:
                    key.data.sendall(data)
                else:
                    poll.unregister(key.fileobj)
                    with contextlib.suppress(OSError):
                        key.data.shutdown(socket.SHUT_WR)


class Relay(socketserver.BaseRequestHandler):
    def handle(self):
        try:
            if isinstance(self.server.destination, tuple):
                upstream = socket.create_connection(self.server.destination, timeout=10)
            else:
                upstream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                upstream.settimeout(10)
                upstream.connect(self.server.destination)
            with upstream:
                upstream.settimeout(None)
                transfer(self.request, upstream)
        except OSError:
            # Socket errors contain no request data and are returned as EOF.
            pass


class UnixServer(socketserver.ThreadingUnixStreamServer):
    daemon_threads = True
    block_on_close = False
    request_queue_size = 64


class TcpServer(socketserver.ThreadingTCPServer):
    daemon_threads = True
    block_on_close = False
    allow_reuse_address = True
    request_queue_size = 64


@contextlib.contextmanager
def relays(socket_root, inside):
    servers = []
    try:
        for name, host, port in PROXIES:
            path = str(pathlib.Path(socket_root) / (name + ".sock"))
            if inside:
                server = TcpServer((host, port), Relay)
                server.destination = path
            else:
                server = UnixServer(path, Relay)
                server.destination = (host, port)
            servers.append(server)
            threading.Thread(target=server.serve_forever, daemon=True).start()
        yield
    finally:
        for server in servers:
            server.shutdown()
            server.server_close()


def namespace_command(home, cwd, socket_root, source):
    # No / bind, no host /proc, and no shared writable directory. The namespace
    # starts from an empty root so new host mounts cannot become visible later.
    args = [
        BWRAP, "--unshare-user", "--unshare-pid", "--unshare-net",
        "--unshare-ipc", "--unshare-uts", "--die-with-parent", "--cap-drop", "ALL",
        "--ro-bind", "/usr", "/usr",
        "--symlink", "usr/bin", "/bin",
        "--symlink", "usr/sbin", "/sbin",
        "--symlink", "usr/lib", "/lib",
        "--ro-bind", "/etc", "/etc",
        "--proc", "/proc", "--dev", "/dev",
        "--tmpfs", "/tmp", "--dir", "/run",
        "--ro-bind", socket_root, SOCKET_ROOT,
        "--bind", home, home, "--bind", cwd, cwd,
    ]
    if os.path.exists("/usr/lib64"):
        args += ["--symlink", "usr/lib64", "/lib64"]
    # Only immutable/public tool installations and skills from the shared home
    # are included. Provider auth, other agents, Git objects and worktrees are
    # deliberately absent. Missing optional installations are not synthesized.
    for directory in (
        "/home/cma/.local/bin", "/home/cma/.local/lib",
        "/home/cma/.codex/skills",
    ):
        if os.path.isdir(directory):
            args += ["--ro-bind", directory, directory]
    args += [
        "--chdir", cwd, "--", PYTHON, "-I", "-c", source, "inner", home, cwd,
    ]
    return args


def wait_child(command, env=None):
    child = subprocess.Popen(command, env=env)
    previous = {}

    def stop(signum, _frame):
        # The runtime already owns the whole process session. Forward as well
        # for a parent-only signal and let bwrap tear down its PID namespace.
        with contextlib.suppress(ProcessLookupError):
            child.send_signal(signum)

    for signum in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        previous[signum] = signal.signal(signum, stop)
    try:
        return child.wait()
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)
        if child.poll() is None:
            child.kill()
            child.wait()


def inner(home, cwd):
    env = os.environ.copy()
    env["HOME"] = home
    env["CODEX_HOME"] = home
    env["TMPDIR"] = "/tmp"
    # The original worktree .git points at the shared repository, which is not
    # mounted. An independent persistent Git directory keeps normal Git commands
    # usable without making a sibling's committed objects or refs readable.
    git_dir = os.path.join(home, ".cma-private-git")
    if not os.path.exists(git_dir):
        subprocess.run(
            ["/usr/bin/git", "init", "--quiet", "--bare", git_dir],
            check=True, stdout=subprocess.DEVNULL,
        )
    env["GIT_DIR"] = git_dir
    env["GIT_WORK_TREE"] = cwd
    with relays(SOCKET_ROOT, inside=True):
        return wait_child(["/usr/local/bin/codex-acp"], env=env)


def main():
    mode, home, cwd = sys.argv[1:]
    if mode == "inner":
        return inner(home, cwd)
    if mode != "outer":
        raise ValueError("invalid isolation launch mode")
    # -c receives the trusted source in argv[2] of the parent command. CPython
    # exposes the same string via orig_argv without reading an agent-owned file.
    source = sys.orig_argv[sys.orig_argv.index("-c") + 1]
    with tempfile.TemporaryDirectory(prefix="cma-agent-proxies-", dir="/tmp") as root:
        os.chmod(root, 0o700)
        with relays(root, inside=False):
            return wait_child(namespace_command(home, cwd, root, source))


if __name__ == "__main__":
    sys.exit(main())
