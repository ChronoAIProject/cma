"""Interrupt only the owned Buildx invocation when its reservation changes."""
import signal
import subprocess
import tempfile
import time

from staging_cd_canary_process import quiesce, subreaper, wait_empty
from staging_cd_common import require
from staging_cd_reservations import ReservationSuperseded


class _BuildInterrupted(BaseException):
    """A received signal interrupts the wait after the child handle is owned."""


def _wait_for_build(child, timeout, check_owner, interval):
    deadline = time.monotonic() + timeout
    while child.poll() is None:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise subprocess.TimeoutExpired(child.args, timeout)
        try:
            child.wait(timeout=min(interval, remaining))
        except subprocess.TimeoutExpired:
            if time.monotonic() >= deadline:
                raise subprocess.TimeoutExpired(child.args, timeout) from None
            try:
                check_owner()
            except ReservationSuperseded:
                # Preserve a genuine failure that completed during the read.
                if child.poll() in (None, 0):
                    raise


def run(args, *, cwd, env, timeout, check_owner, interval=30):
    check_owner()
    previous = subreaper(True)
    saved_handlers = {}
    interrupted, waiting = None, False

    def interrupt(signum, _frame):
        nonlocal interrupted
        if interrupted is None:
            interrupted = signum
            if waiting:
                raise _BuildInterrupted()

    try:
        for signum in (signal.SIGINT, signal.SIGTERM):
            saved_handlers[signum] = signal.signal(signum, interrupt)
        # Keep only the signal number during spawn and cleanup. An exception
        # there could leave a process group whose leader we cannot reap.
        with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr, subprocess.Popen(
            args, cwd=cwd, env=env, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr, start_new_session=True,
        ) as child:
            try:
                try:
                    waiting = True
                    if interrupted is not None:
                        raise _BuildInterrupted()
                    _wait_for_build(child, timeout, check_owner, interval)
                finally:
                    waiting = False
                require(wait_empty(child, 0), "Build finished with a surviving helper")
                stdout.seek(0)
                stderr.seek(0)
                result = subprocess.CompletedProcess(args, child.returncode, stdout.read(), stderr.read())
            except BaseException:
                # Disconnect the Buildx session and let the plugin cancel its solve.
                require(quiesce(child, 2), "Build process group did not exit")
                raise
    finally:
        try:
            if previous is not None:
                subreaper(previous)
        finally:
            for signum, handler in saved_handlers.items():
                signal.signal(signum, handler)
            # Cancellation can arrive during cleanup for an existing error.
            # Deliver it after reaping the group on either exit path.
            if interrupted is not None:
                signal.raise_signal(interrupted)
                raise SystemExit(128 + interrupted)
    return result
