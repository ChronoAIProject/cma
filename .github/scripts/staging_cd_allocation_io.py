"""Bound allocation reads and joins; remote writes are never retried."""
import subprocess
import time
from concurrent.futures import ThreadPoolExecutor

from staging_cd_common import Refusal, require
from staging_cd_github import GitHub, GitHubNotFound
from staging_cd_history import DevelopHistory


def parallel_read(operation, values):
    values = list(values)
    if not values:
        return []
    with ThreadPoolExecutor(max_workers=min(8, len(values))) as pool:
        futures = [pool.submit(operation, value) for value in values]
        try:
            return [future.result() for future in futures]
        except BaseException:
            for future in futures:
                future.cancel()
            raise


class AllocationIO:
    def __init__(self, runner=subprocess.run, *, clock=time.monotonic, sleep=time.sleep, seconds=30):
        self.runner, self.clock, self.sleep = runner, clock, sleep
        self.deadline = clock() + seconds

    def remaining(self):
        remaining = self.deadline - self.clock()
        require(remaining > 0, "Allocation deadline exceeded")
        return remaining

    def read(self, args, **kwargs):
        for attempt in range(2):
            kwargs["timeout"] = min(5, self.remaining())
            try:
                result = self.runner(args, **kwargs)
            except (OSError, subprocess.SubprocessError):
                if attempt:
                    raise Refusal("Allocation read failed") from None
            else:
                # Included HTTP status separates transient reads from auth,
                # missing provenance and invalid responses. Never retry writes.
                status = result.stdout.splitlines()[0] if result.stdout else ""
                code = status.split(" ", 2)[1] if status.startswith("HTTP/") else ""
                transient = result.returncode == 1 and (not code or code == "429" or code.startswith("5"))
                if not transient or attempt:
                    return result
            self.sleep(min(0.2, self.remaining()))

    def write(self, args, **kwargs):
        kwargs["timeout"] = min(10, self.remaining())
        try:
            return self.runner(args, **kwargs)
        except (OSError, subprocess.SubprocessError):
            # The remote may have committed. Retain its burned refs and let a
            # later allocation take a new snapshot; never repeat this push.
            raise Refusal("Allocation write settlement is uncertain") from None


class AllocationGitHub(GitHub):
    def __init__(self, token, io):
        super().__init__(token, runner=io.read)
        self.io = io

    def request(self, path, payload=None, *, allow_not_found=False):
        require(payload is None, "Allocation API only permits reads")
        try:
            return super().request(path, allow_not_found=True)
        except GitHubNotFound:
            if allow_not_found:
                raise
            raise Refusal("Allocation provenance could not be read") from None

    def history(self):
        if self._history is None:
            self._history = DevelopHistory.load(self.request, runner=self.io.read)
        return self._history
