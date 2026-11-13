"""Track one runner's descendants across sessions, including detached browsers."""
from contextlib import contextmanager
import ctypes
from dataclasses import dataclass
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


@dataclass(frozen=True)
class Process:
    pid: int
    parent: int
    group: int
    started: str
    state: str

    @property
    def identity(self):
        return self.pid, self.started


def snapshot():
    """Read identifiers only; never inspect or log another process's arguments."""
    result = {}
    if sys.platform == "linux":
        for path in Path("/proc").glob("[0-9]*/stat"):
            try:
                # comm can contain spaces and parentheses; the suffix starts at state.
                fields = path.read_text().rsplit(")", 1)[1].split()
            except (FileNotFoundError, ProcessLookupError):
                continue  # The process exited while the table was read.
            pid = int(path.parent.name)
            result[pid] = Process(pid, int(fields[1]), int(fields[2]), fields[19], fields[0])
    else:
        table = subprocess.check_output(
            ["ps", "-axo", "pid=,ppid=,pgid=,lstart=,stat="], text=True,
            env={**os.environ, "LC_ALL": "C"}, timeout=5)
        for line in table.splitlines():
            fields = line.split()
            pid, parent, group = map(int, fields[:3])
            result[pid] = Process(pid, parent, group, " ".join(fields[3:8]), fields[8])
    return result


@contextmanager
def child_subreaper():
    """Reap tracked orphan descendants on Linux instead of leaving zombies to PID 1."""
    if sys.platform != "linux":
        yield
        return
    libc = ctypes.CDLL(None, use_errno=True)
    previous = ctypes.c_int()

    def prctl(option, value):
        if libc.prctl(option, value, 0, 0, 0) != 0:
            error = ctypes.get_errno()
            raise OSError(error, os.strerror(error))

    prctl(37, ctypes.byref(previous))  # PR_GET_CHILD_SUBREAPER
    prctl(36, 1)  # PR_SET_CHILD_SUBREAPER; scoped to this supervisor process.
    try:
        yield
    finally:
        prctl(36, previous.value)


class OwnedProcesses:
    def __init__(self, pid):
        self.root = pid
        self.owned = {}
        self.killed = []
        self.remaining = []
        process = snapshot().get(pid)
        if process:
            self.owned[pid] = process

    def capture(self):
        table = snapshot()
        live = {pid: table[pid] for pid, process in self.owned.items()
                if pid in table and table[pid].identity == process.identity}
        # Keep identities after reparenting; only add processes linked to an owned parent.
        while True:
            children = {pid: process for pid, process in table.items()
                        if process.parent in live and pid not in live}
            if not children:
                break
            live.update(children)
            self.owned.update(children)
        return live

    def terminate(self):
        # Freeze the tree before killing parents: otherwise a detached browser can
        # fork/reparent between discovery and cleanup. Repeat to capture racing forks.
        stopped = set()
        deadline = time.monotonic() + 5
        while True:
            live = self.capture()
            additions = [p for p in live.values() if p.identity not in stopped]
            if not additions and all(p.state.startswith(("T", "Z")) for p in live.values()):
                break
            for process in additions:
                self.send(process, signal.SIGSTOP)
                stopped.add(process.identity)
            if time.monotonic() >= deadline:
                # Still kill the recorded tree if a process is stuck in the kernel;
                # reap() reports any survivor as a cleanup failure.
                break
            time.sleep(.01)
        for process in reversed(list(self.capture().values())):
            if self.send(process, signal.SIGKILL):
                self.killed.append(process.pid)

    @staticmethod
    def send(process, number):
        current = snapshot().get(process.pid)
        if not current or current.identity != process.identity:
            return False  # An exited/reused PID is no longer ours.
        try:
            os.kill(process.pid, number)
            return True
        except ProcessLookupError:
            return False  # Exit between identity check and signal.

    def reap(self, timeout=5):
        deadline = time.monotonic() + timeout
        while True:
            # Popen owns/reaps the root; wait only on explicitly tracked descendants.
            for pid in self.capture():
                if pid == self.root:
                    continue
                try:
                    os.waitpid(pid, os.WNOHANG)
                except ChildProcessError:
                    pass  # Still parented elsewhere, or already reaped.
            self.remaining = sorted(self.capture())
            if not self.remaining:
                return
            if time.monotonic() >= deadline:
                raise RuntimeError(f"Owned browser processes survived cleanup: {self.remaining}")
            time.sleep(.05)

    def evidence(self):
        return {"owned": [vars(p) for p in self.owned.values()],
                "killed": self.killed, "remaining": self.remaining}
