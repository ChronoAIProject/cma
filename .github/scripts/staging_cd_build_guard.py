"""Reap owned build and reservation probe processes before forwarding cancellation."""
import signal
import subprocess
import tempfile
import time

from staging_cd_canary_process import quiesce, subreaper, wait_empty
from staging_cd_common import require
from staging_cd_reservations import ReservationSuperseded


class _BuildInterrupted(BaseException):
    """Unwind all owned process groups at a safe checkpoint."""


def _wait(child, timeout, cancelled, *, check_owner, interval):
    start = time.monotonic()
    deadline = start + timeout
    next_check = start + interval if check_owner is not None else deadline
    while child.poll() is None:
        if cancelled():
            raise _BuildInterrupted()
        now = time.monotonic()
        if now >= deadline:
            raise subprocess.TimeoutExpired(child.args, timeout)
        if check_owner is not None and now >= next_check:
            try:
                check_owner()
            except ReservationSuperseded:
                # Preserve a genuine failure that completed during the read.
                if child.poll() in (None, 0):
                    raise
            next_check = time.monotonic() + interval
            continue
        time.sleep(min(.05, deadline - now, next_check - now))


def _run_process(args, *, cwd, env, timeout, cancelled, text=False, check_owner=None, interval=30, grace):
    if cancelled():
        raise _BuildInterrupted()
    # Files avoid hanging on a pipe held open by a helper.
    mode = "w+t" if text else "w+b"
    with tempfile.TemporaryFile(mode=mode) as stdout, tempfile.TemporaryFile(mode=mode) as stderr, subprocess.Popen(
        args, cwd=cwd, env=env, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr, start_new_session=True,
    ) as child:
        try:
            _wait(child, timeout, cancelled, check_owner=check_owner, interval=interval)
            require(wait_empty(child, 0), "Owned command finished with a surviving helper")
            stdout.seek(0)
            stderr.seek(0)
            return subprocess.CompletedProcess(args, child.returncode, stdout.read(), stderr.read())
        except BaseException:
            require(quiesce(child, grace), "Owned process group did not exit")
            raise


def run(args, *, cwd, env, timeout, check_owner, interval=30):
    previous = subreaper(True)
    saved_handlers = {}
    interrupted = None

    def interrupt(signum, _frame):
        nonlocal interrupted
        if interrupted is None:
            interrupted = signum

    def cancelled():
        return interrupted is not None

    def probe(command, *, cwd=None, env=None, timeout, input=None, capture_output, text, check):
        require(input is None and capture_output and text and not check, "Invalid ownership probe options")
        return _run_process(command, cwd=cwd, env=env, timeout=timeout, cancelled=cancelled, text=True, grace=.5)

    def verify_owner():
        check_owner(probe)

    try:
        # Raising from a signal handler can strand Popen's waitpid lock.
        # Both the build and its probes check for signals between process operations.
        for signum in (signal.SIGINT, signal.SIGTERM):
            saved_handlers[signum] = signal.signal(signum, interrupt)
        verify_owner()
        return _run_process(args, cwd=cwd, env=env, timeout=timeout, cancelled=cancelled,
                            check_owner=verify_owner, interval=interval, grace=2)
    finally:
        try:
            if previous is not None:
                subreaper(previous)
        finally:
            for signum, handler in saved_handlers.items():
                signal.signal(signum, handler)
            # An external cancellation takes precedence over a superseded skip.
            if interrupted is not None:
                signal.raise_signal(interrupted)
                raise SystemExit(128 + interrupted)
