#!/usr/bin/env python3
"""Expose the release stamp to the published executable's compiler only."""

import os
from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parents[1]
ENTRYPOINTS = {ROOT / f"crates/{service}/src/main.rs" for service in ("cma", "cmaeg")}
STAMP_VARIABLES = ("CHRONO_BUILD", "CHRONO_BUILD_SHA")


if __name__ == "__main__":
    rustc, *arguments = sys.argv[1:]
    environment = os.environ.copy()
    executable = ("--crate-type", "bin") in zip(arguments, arguments[1:])
    entrypoint = any(Path(arg).resolve() in ENTRYPOINTS for arg in arguments if arg.endswith("/main.rs"))
    if not (executable and entrypoint):
        for variable in STAMP_VARIABLES:
            environment.pop(variable, None)
    os.execvpe(rustc, [rustc, *arguments], environment)
