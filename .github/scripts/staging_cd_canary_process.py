"""Quiesce an isolated canary process group before Docker reconciliation."""
import ctypes
import os
import signal
import subprocess
import sys
import tempfile
import threading
import time

from staging_cd_common import Refusal


class CanaryProcessFailure(Refusal):
    def __init__(self, reason, quiesced):
        super().__init__("Runtime canary process failed")
        self.reason = reason
        self.quiesced = quiesced


def subreaper(enabled):
    # Linux otherwise reparents grandchildren to PID 1, whose reaping behavior
    # is outside our control. This affects this process only, not the machine.
    if not sys.platform.startswith("linux"):
        return None
    libc = ctypes.CDLL(None, use_errno=True)
    current = ctypes.c_int()
    if libc.prctl(37, ctypes.byref(current), 0, 0, 0) != 0:
        raise CanaryProcessFailure("subreaper_unavailable", True)
    if libc.prctl(36, int(enabled), 0, 0, 0) != 0:
        raise CanaryProcessFailure("subreaper_unavailable", True)
    return current.value


def signal_group(pid, sig):
    try:
        os.killpg(pid, sig)
        return True
    except ProcessLookupError:
        return False


def reap_group(child):
    child.poll()
    # Only this invocation's adopted descendants; never consume another
    # caller's child status. Popen owns/reaps the leader before this loop.
    if child.returncode is not None:
        while True:
            try:
                pid, _ = os.waitpid(-child.pid, os.WNOHANG)
            except ChildProcessError:
                break
            if pid == 0:
                break


def wait_empty(child, seconds):
    end = time.monotonic() + seconds
    while True:
        reap_group(child)
        try:
            exists = signal_group(child.pid, 0)
        except PermissionError:
            exists = True
        if not exists:
            return True
        if time.monotonic() >= end:
            return False
        time.sleep(min(0.02, max(0, end - time.monotonic())))


def quiesce(child, grace):
    signal_group(child.pid, signal.SIGTERM)
    if not wait_empty(child, grace):
        signal_group(child.pid, signal.SIGKILL)
    empty = wait_empty(child, 2)
    try:
        child.wait(timeout=1)
    except subprocess.TimeoutExpired:
        return False
    return empty


def run(args, env, timeout, *, cwd=None, grace=0.5):
    if threading.current_thread() is not threading.main_thread():
        raise CanaryProcessFailure("main_thread_required", True)
    if timeout <= 0 or grace < 0:
        raise CanaryProcessFailure("invalid_deadline", True)
    cancelled = False
    def cancel(*_):
        nonlocal cancelled
        cancelled = True
    previous = {sig: signal.getsignal(sig) for sig in (signal.SIGTERM, signal.SIGINT)}
    prior_reaper, child = None, None
    reason, quiet = None, False
    try:
        prior_reaper = subreaper(True)
        for sig in previous:
            signal.signal(sig, cancel)
        # Files avoid communicate() waiting forever on pipes inherited by a
        # descendant. The entire output remains private and bounded on return.
        with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
            child = subprocess.Popen(args, env=env, cwd=cwd, stdin=subprocess.DEVNULL,
                                     stdout=stdout, stderr=stderr, start_new_session=True)
            deadline = time.monotonic() + timeout
            while child.poll() is None and not cancelled and time.monotonic() < deadline:
                time.sleep(min(0.02, max(0, deadline - time.monotonic())))
            reason = "cancelled" if cancelled else "timeout" if child.returncode is None else None
            if reason is None and child.returncode != 0:
                reason = "abnormal_exit"
            if reason is None and not wait_empty(child, 0):
                reason = "surviving_helper"
            quiet = quiesce(child, grace) if reason else True
            if reason or not quiet:
                raise CanaryProcessFailure(reason or "cleanup_incomplete", quiet)
            stdout.seek(0)
            output = stdout.read(1048577)
            if len(output) > 1048576:
                raise CanaryProcessFailure("output_limit", True)
            return output
    except BaseException as error:
        if child is not None and not quiet:
            try:
                quiet = quiesce(child, grace)
            except (OSError, subprocess.TimeoutExpired):
                quiet = False
        if isinstance(error, CanaryProcessFailure):
            raise CanaryProcessFailure(error.reason, quiet if child is not None else error.quiesced) from None
        raise CanaryProcessFailure("interrupted_or_failed", quiet or child is None) from None
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)
        if prior_reaper is not None:
            subreaper(prior_reaper)
