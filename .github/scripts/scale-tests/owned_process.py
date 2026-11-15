"""POSIX process-group custody for the disposable, non-daemonizing EG fixture.

A separate direct child reserves the group's ID until all other members exit.
Never poll/reap that anchor before the final group observation: even if killed,
its unreaped PID cannot be reused. Only this object may wait for that child.
This is not containment for executables that deliberately leave their group.
"""
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import threading
import time


class CustodyError(RuntimeError):
    """Fixed local custody failure, without external process details."""


def group_members(group_id):
    # Numeric fields only; never collect command lines or environment variables.
    result = subprocess.run(["/bin/ps", "-axo", "pid=,pgid="], capture_output=True, timeout=2)
    if result.returncode:
        raise CustodyError("group_observation_failed")
    rows = [tuple(map(int, line.split())) for line in result.stdout.splitlines()]
    if any(len(row) != 2 for row in rows):
        raise CustodyError("group_observation_invalid")
    return {pid for pid, pgid in rows if pgid == group_id}


class OwnedGroup:
    def __init__(self):
        self.anchor = None
        self.released = False
        self.confirmed = False

    def start(self):
        # preexec_fn is needed on supported Python 3.9. Refuse threaded callers
        # and auto-reaping SIGCHLD handlers rather than weaken the reservation.
        if threading.active_count() != 1 or signal.getsignal(signal.SIGCHLD) != signal.SIG_DFL:
            raise CustodyError("unsupported_process_custody")
        self.anchor = subprocess.Popen(
            [sys.executable, "-I", str(Path(__file__).resolve()), "--anchor"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            env={}, preexec_fn=os.setpgrp)
        ready, _, _ = select.select([self.anchor.stdout], [], [], 5)
        if not ready or os.read(self.anchor.stdout.fileno(), 6) != b"READY\n":
            raise CustodyError("group_anchor_not_ready")

    def reserved(self):
        if self.anchor is None or self.released or self.anchor.returncode is not None:
            return False
        if signal.getsignal(signal.SIGCHLD) != signal.SIG_DFL:
            return False
        # Custody means an unreaped direct child, not liveness. Darwin may
        # return ESRCH from getpgid for a zombie, but cannot reuse its PID until
        # we wait. This single-threaded program has no other child reaper.
        return True

    def spawn(self, arguments, **options):
        if not self.reserved():
            raise CustodyError("group_ownership_unconfirmed")
        return subprocess.Popen(arguments, preexec_fn=lambda: os.setpgid(0, self.anchor.pid), **options)

    def signal(self, value):
        if not self.reserved():
            raise CustodyError("group_ownership_unconfirmed")
        # The unreaped direct-child anchor reserves this PGID across the call.
        try:
            os.killpg(self.anchor.pid, value)
            return True
        except ProcessLookupError:
            # No group exists. Its ID is still reserved by the unreaped child;
            # cleanup must independently observe emptiness before reaping it.
            return False

    def remaining(self, process):
        if not self.reserved():
            raise CustodyError("group_ownership_unconfirmed")
        if process is not None:
            process.poll()  # Reap only the Rust parent, never the anchor.
        return group_members(self.anchor.pid) - {self.anchor.pid}

    def wait_empty(self, process, seconds):
        deadline = time.monotonic() + seconds
        while True:
            if not self.remaining(process):
                return True
            if time.monotonic() >= deadline:
                return False
            time.sleep(min(0.05, max(0, deadline - time.monotonic())))

    def cleanup(self, process=None, grace=10, kill_wait=10):
        receipt = {"process_group_cleanup": False, "group_term_sent": False,
                   "group_kill_sent": False, "group_observation_failed": False}
        if self.confirmed or self.anchor is None:
            receipt["process_group_cleanup"] = True
            return receipt
        try:
            try:
                empty = not self.remaining(process)
            except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired):
                empty = False
                receipt["group_observation_failed"] = True
            if not empty:
                receipt["group_term_sent"] = self.signal(signal.SIGTERM)
                try:
                    empty = self.wait_empty(process, grace)
                except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired):
                    receipt["group_observation_failed"] = True
                if not empty:
                    receipt["group_kill_sent"] = self.signal(signal.SIGKILL)
                    empty = self.wait_empty(process, kill_wait)
            if not empty:
                raise CustodyError("group_exit_unconfirmed")
            # Only the inert anchor remains; it never spawns application work.
            # It kills its own still-reserved group on EOF, then we reap it.
            self.anchor.stdin.close()
            self.anchor.wait(timeout=kill_wait)
            self.released = True
            self.anchor.stdout.close()
            self.confirmed = True
            receipt["process_group_cleanup"] = True
        except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired):
            receipt["group_observation_failed"] = True
            # Closing control is a last bounded cleanup attempt: the anchor can
            # safely signal its own group even if the caller lost its evidence.
            # Do not reap/reuse its identity or upgrade this result to success.
            self.anchor.stdin.close()
        return receipt


def anchor_main():
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    os.write(1, b"READY\n")
    # Parent EOF also handles unexpected runner termination. No credentials,
    # arbitrary commands, threads or application code enter the anchor.
    while os.read(0, 1):
        pass
    os.killpg(os.getpgrp(), signal.SIGKILL)


if __name__ == "__main__" and sys.argv[1:] == ["--anchor"]:
    anchor_main()
