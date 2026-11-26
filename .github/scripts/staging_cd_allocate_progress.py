"""Live, closed-vocabulary allocation progress across the private log boundary."""
import contextlib
import io
import math
import os
import sys
import threading
import time

from staging_cd_common import matches, require
from staging_cd_versions import DEPLOYABLES

PHASES = frozenset({"final-inventory", "pending-inventory", "history", "owner-read",
                    "merged-pr", "reserve", "complete", "planned", "tag-inventory",
                    "annotation-read", "reservation-push", "reservation-readback",
                    "replan", "stale-source", "readback-transient", "readback-missing", "readback-conflict",
                    "settle-poll", "settle-transient", "settled", "settle-refused"})


@contextlib.contextmanager
def allocation_progress(clock=time.monotonic):
    # Capture only this reporter's public descriptor before run_private redirects
    # ordinary/native/child output. Never restore those descriptors mid-operation.
    with contextlib.ExitStack() as stack:
        output = sys.stdout
        output.flush()
        try:
            descriptor = output.fileno()
        except (AttributeError, io.UnsupportedOperation):
            pass  # In-memory test streams retain their original object.
        else:
            output = stack.enter_context(os.fdopen(os.dup(descriptor), "w", encoding="utf-8"))
        start = clock()
        counts, lock = {}, threading.Lock()

        def mark(component, phase):
            require(component in {*DEPLOYABLES, "all"} and phase in PHASES,
                    "Unknown allocation progress category")
            with lock:
                elapsed = clock() - start
                require(math.isfinite(elapsed) and elapsed >= 0, "Invalid allocation elapsed time")
                counts[phase] = counts.get(phase, 0) + 1
                print(f"PROGRESS allocation component={component} phase={phase} count={counts[phase]} "
                      f"elapsed_seconds={int(elapsed)}", file=output, flush=True)

        def retry(reason, attempt, attempts, wait):
            # The caller passes a fixed transient code; nothing else can be printed.
            require(matches(r"[A-Z][A-Z0-9_]{0,47}", reason) and type(attempt) is int and type(attempts) is int
                    and 1 < attempt <= attempts and isinstance(wait, (int, float)) and math.isfinite(wait)
                    and wait >= 0, "Invalid allocation retry line")
            with lock:
                print(f"RETRY allocation reason={reason} attempt={attempt}/{attempts} wait={wait:.1f}s",
                      file=output, flush=True)

        mark.retry = retry
        yield mark
