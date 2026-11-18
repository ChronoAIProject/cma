"""Bound the full subprocess group, suppress its output, and always join it."""
import os
import signal
import subprocess


class ProcessTimeout(subprocess.TimeoutExpired):
    """Retain the controller's interrupted outcome after its bounded cleanup."""

    def __init__(self, args, timeout, returncode):
        super().__init__(args, timeout)
        self.returncode = returncode


def terminate(process, grace_seconds):
    if grace_seconds:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            return process.wait(timeout=grace_seconds)
        except subprocess.TimeoutExpired:
            pass  # The reviewed cleanup allowance is now exhausted.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass  # The entire process group has already exited.
    return process.wait(timeout=1)


def run(args, env, timeout, *, cwd=None, grace_seconds=0):
    with subprocess.Popen(args, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                          start_new_session=True) as process:
        try:
            return process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            code = terminate(process, grace_seconds)
            raise ProcessTimeout(args, timeout, code) from None
        except BaseException:
            # Killing only the parent can strand pip, Git or controller workers.
            # Even if the parent just exited, the process group may still live.
            terminate(process, grace_seconds)
            raise
