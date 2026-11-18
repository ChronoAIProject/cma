"""Fixed public timing categories; captured commands and values never escape."""
import contextlib
import json
import math
import time

STAGES = frozenset({
    "source", "reuse", "cache-restore", "cache-export",
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
        finally:
            elapsed = self.clock() - start
            if not math.isfinite(elapsed) or elapsed < 0:
                elapsed = 0
            self.rows.append({"stage": name, "seconds": round(elapsed, 3), "result": result})

    def emit(self, output):
        for row in self.rows:
            print(json.dumps({"event": "publication-stage", **row}, sort_keys=True), file=output, flush=True)
