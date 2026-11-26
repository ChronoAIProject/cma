"""Fixed public timing categories; captured commands and values never escape."""
import contextlib
import json
import math
import time

from staging_cd_reservations import ReservationSuperseded

STAGES = frozenset({
    "source", "reuse", "cache-restore", "cache-export", "cache-export-wait",
    "build", "inspect", "scan", "runtime-smoke", "registry-check", "login", "push", "verify",
})


class Timings:
    def __init__(self, clock=time.monotonic):
        self.clock, self.rows = clock, []

    @contextlib.contextmanager
    def stage(self, name):
        if name not in STAGES:
            raise ValueError("Unknown publication timing category")
        start, result = self.clock(), "failure"
        try:
            yield
            result = "success"
        except ReservationSuperseded:
            result = "reservation_superseded"
            raise
        finally:
            elapsed = self.clock() - start
            if not math.isfinite(elapsed) or elapsed < 0:
                elapsed = 0
            self.rows.append({"event": "publication-stage", "stage": name,
                              "seconds": round(elapsed, 3), "result": result})

    def detector_cache(self, stats):
        # Only fixed numeric counters may cross the private publication boundary.
        fields = ("hits", "misses", "withheld")
        if isinstance(stats, dict) and all(type(stats.get(k)) is int and 0 <= stats[k] <= 10**9 for k in fields):
            self.rows.append({"event": "publication-detector-cache", **{k: stats[k] for k in fields}})

    def emit(self, output, *, scan_only=False):
        for row in self.rows:
            if scan_only and row.get("stage", "scan") != "scan":
                continue
            print(json.dumps(row, sort_keys=True), file=output, flush=True)
