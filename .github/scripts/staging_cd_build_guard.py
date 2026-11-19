"""Interrupt only the owned Buildx invocation when its reservation changes."""
import subprocess
import tempfile
import time

from staging_cd_canary_process import quiesce, subreaper, wait_empty
from staging_cd_common import require
from staging_cd_reservations import ReservationSuperseded


def run(args, *, cwd, env, timeout, check_owner, interval=30):
    check_owner()
    previous = subreaper(True)
    try:
        # Build progress can outlive the CLI when a helper inherits a pipe.
        # Private files let us wait for the process and cancel its whole group.
        with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr, subprocess.Popen(
            args, cwd=cwd, env=env, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr, start_new_session=True,
        ) as child:
            deadline = time.monotonic() + timeout
            try:
                while child.poll() is None:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise subprocess.TimeoutExpired(args, timeout)
                    try:
                        child.wait(timeout=min(interval, remaining))
                    except subprocess.TimeoutExpired:
                        if time.monotonic() >= deadline:
                            raise subprocess.TimeoutExpired(args, timeout) from None
                        try:
                            check_owner()
                        except ReservationSuperseded:
                            # Preserve a genuine failure that completed during the read.
                            if child.poll() in (None, 0):
                                raise
                require(wait_empty(child, 0), "Build finished with a surviving helper")
                stdout.seek(0)
                stderr.seek(0)
                return subprocess.CompletedProcess(args, child.returncode, stdout.read(), stderr.read())
            except BaseException:
                # Disconnect the Buildx session and let the plugin cancel its solve.
                require(quiesce(child, 2), "Build process group did not exit")
                raise
    finally:
        if previous is not None:
            subreaper(previous)
