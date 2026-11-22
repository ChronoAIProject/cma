"""Bound allocation reads and joins; remote writes are never retried."""
import random
import subprocess
import time
from concurrent.futures import ThreadPoolExecutor

import ci_retry
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


class TransientRead(Refusal, ci_retry.TransientError):
    """A read-only network operation can be attempted again within its budget."""


class UncertainPush(Refusal):
    """A timed-out remote transaction may already have committed."""


# Inventory reads grow with every published tag. At 30 s, hosted runs reached
# the reservation push with less than the 7 s it requires and failed.
ALLOCATION_SECONDS = 120
# GitHub can take longer than 10 s to accept an atomic push of several tags
# and claims, especially while tag rulesets are evaluated. A client that gives
# up earlier leaves the server to commit the push after it has stopped
# listening, so the write gets a longer bound.
PUSH_SECONDS = 30
# After a timed-out push, the server may still commit it. Settlement polls the
# exact refs with waits of 1, 2, 4 and 8 s, then 8 s each, for as long as the
# allocation budget allows. Every poll starts with this reserve still left: it
# covers that final read and the parallel owner readbacks of a settled batch.
VERIFY_RESERVE_SECONDS = 5
SETTLE_WAITS = (1, 2, 4, 8)
# A push never starts unless the full ladder of waits and the verification
# reserve still fit after it has used its whole bound: the guaranteed minimum.
PUSH_SETTLEMENT_SECONDS = sum(SETTLE_WAITS) + VERIFY_RESERVE_SECONDS
PUSH_RESERVE_SECONDS = PUSH_SETTLEMENT_SECONDS + 1


class AllocationIO:
    def __init__(self, runner=subprocess.run, *, clock=time.monotonic, sleep=time.sleep,
                 rand=random.random, seconds=ALLOCATION_SECONDS):
        self.runner, self.clock, self.sleep, self.rand = runner, clock, sleep, rand
        self.deadline = clock() + seconds
        # Set just before the reservation push runs. From then on the remote may
        # hold this run's refs, so no refusal may rerun the allocation.
        self.push_started = False

    @property
    def retry(self):
        # Reads share one short budget with the reservation push. Waits stay
        # sub-second, and no repeat may eat into the time the push reserves.
        return {"base": 0.5, "cap": 2.0, "deadline": self.deadline - PUSH_RESERVE_SECONDS, "clock": self.clock,
                "sleep": self.sleep, "rand": self.rand}

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
            # Keep the wait a throttled response asked for, for the caller.
            head = result.stdout.replace("\r\n", "\n").partition("\n\n")[0]
            raise TransientRead("Allocation read failed") from (
                ci_retry.status_cause(int(code), head) if code.isdigit() else None)
        # Git exits 128 for every fatal error. Only its diagnostic tells an
        # outage (an HTTP 502, a reset connection) from a refusal that persists.
        if (args[:1] == ["git"] and result.returncode == 128
                and ci_retry.cli_message(result.stderr) == ci_retry.TRANSIENT):
            raise TransientRead("Allocation read failed") from ci_retry.cli_cause(result.stderr)
        return result

    def read(self, args, **kwargs):
        return ci_retry.call(lambda: self.read_once(args, **kwargs), name="allocation-read",
                             classify=ci_retry.classify, **self.retry)

    def write(self, args, **kwargs):
        push = args[:2] == ["git", "push"]
        available = self.remaining() - (PUSH_SETTLEMENT_SECONDS if push else 0)
        require(available >= PUSH_RESERVE_SECONDS - PUSH_SETTLEMENT_SECONDS if push else available > 0,
                "Allocation deadline exceeded")
        kwargs["timeout"] = min(PUSH_SECONDS if push else 10, available)
        self.push_started = self.push_started or push
        try:
            return self.runner(args, **kwargs)
        except (OSError, subprocess.SubprocessError):
            if push:
                raise UncertainPush("Allocation write settlement is uncertain") from None
            raise Refusal("Allocation write settlement is uncertain") from None


class AllocationGitHub(GitHub):
    def __init__(self, token, io):
        super().__init__(token, runner=io.read, retry=io.retry)
        self.io = io

    def request(self, path, payload=None, *, allow_not_found=False, single_attempt=False):
        require(payload is None, "Allocation API only permits reads")
        try:
            # The inherited read policy repeats transient attempts inside the allocation deadline.
            return super().request(path, allow_not_found=True, runner=self.io.read_once,
                                   attempts=1 if single_attempt else 3)
        except GitHubNotFound:
            if allow_not_found:
                raise
            raise Refusal("Allocation provenance could not be read") from None

    def history(self):
        with self._history_lock:
            if self._history is None:
                self._history = DevelopHistory.load(self.request, runner=self.io.read)
            return self._history
