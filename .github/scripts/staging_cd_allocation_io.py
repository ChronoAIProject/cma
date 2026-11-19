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


class TransientRead(Refusal):
    """A read-only network operation can be attempted again within its budget."""


class UncertainPush(Refusal):
    """A timed-out remote transaction may already have committed."""


class AllocationIO:
    def __init__(self, runner=subprocess.run, *, clock=time.monotonic, sleep=time.sleep, seconds=30):
        self.runner, self.clock, self.sleep = runner, clock, sleep
        self.deadline = clock() + seconds

    def remaining(self):
        remaining = self.deadline - self.clock()
        require(remaining > 0, "Allocation deadline exceeded")
        return remaining

    def read_once(self, args, **kwargs):
        kwargs["timeout"] = min(5, self.remaining())
        try:
            result = self.runner(args, **kwargs)
        except (OSError, subprocess.SubprocessError):
            raise TransientRead("Allocation read failed") from None
        status = result.stdout.partition("\n")[0]
        code = status.partition(" ")[2].partition(" ")[0] if status.startswith("HTTP/") else ""
        if result.returncode == 1 and (not code or code == "429" or code.startswith("5")):
            raise TransientRead("Allocation read failed")
        return result

    def read(self, args, **kwargs):
        try:
            return self.read_once(args, **kwargs)
        except TransientRead:
            self.sleep(min(0.2, self.remaining()))
        return self.read_once(args, **kwargs)

    def write(self, args, **kwargs):
        push = args[:2] == ["git", "push"]
        # A complete ref advertisement and parallel owner readbacks need time
        # after the single remote write. Never start a push in the final second.
        available = self.remaining() - (6 if push else 0)
        require(available >= 1 if push else available > 0, "Allocation deadline exceeded")
        kwargs["timeout"] = min(10, available)
        try:
            return self.runner(args, **kwargs)
        except (OSError, subprocess.SubprocessError):
            if push:
                raise UncertainPush("Allocation write settlement is uncertain") from None
            raise Refusal("Allocation write settlement is uncertain") from None


class AllocationGitHub(GitHub):
    def __init__(self, token, io):
        super().__init__(token, runner=io.read)
        self.io = io

    def request(self, path, payload=None, *, allow_not_found=False, single_attempt=False):
        require(payload is None, "Allocation API only permits reads")
        try:
            return super().request(path, allow_not_found=True, runner=self.io.read_once if single_attempt else self.io.read)
        except GitHubNotFound:
            if allow_not_found:
                raise
            raise Refusal("Allocation provenance could not be read") from None

    def history(self):
        if self._history is None:
            self._history = DevelopHistory.load(self.request, runner=self.io.read)
        return self._history
