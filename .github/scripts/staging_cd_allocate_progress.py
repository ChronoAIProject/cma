"""Live, closed-vocabulary allocation progress across the private log boundary."""
import contextlib
import io
import math
import os
import sys
import time

from staging_cd_common import require
from staging_cd_versions import DEPLOYABLES

PHASES = frozenset({"final-inventory", "pending-inventory", "history", "owner-read",
                    "merged-pr", "reserve", "complete"})


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

        def mark(component, phase):
            require(component in {*DEPLOYABLES, "all"} and phase in PHASES,
                    "Unknown allocation progress category")
            elapsed = clock() - start
            require(math.isfinite(elapsed) and elapsed >= 0, "Invalid allocation elapsed time")
            print(f"PROGRESS allocation component={component} phase={phase} elapsed_seconds={int(elapsed)}",
                  file=output, flush=True)

        yield mark
