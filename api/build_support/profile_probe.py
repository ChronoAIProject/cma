#!/usr/bin/env python3
"""Vary optimization for the AM library in the hosted measurement."""

import os
import sys


TARGETS = {
    "lib": ("crates/cma/src/lib.rs", "/build/crates/cma/src/lib.rs"),
    "main": ("crates/cma/src/main.rs", "/build/crates/cma/src/main.rs"),
}


def option_index(args, key):
    prefix = key + "="
    matches = [index for index, option in enumerate(args)
               if option.startswith("-C" + prefix)
               or (option.startswith(prefix) and index and args[index - 1] == "-C")]
    if len(matches) != 1:
        raise SystemExit(f"Expected one rustc option for {key}")
    return matches[0]


def main():
    rustc, *args = sys.argv[1:]
    target = next((name for name, paths in TARGETS.items() if any(path in args for path in paths)), None)
    if target is not None:
        requested = os.environ.get("CMA_PROFILE_OPT_LEVEL")
        if requested not in {"1", "2"}:
            raise SystemExit("Invalid library optimization experiment")
        opt_index = option_index(args, "opt-level")
        units_index = option_index(args, "codegen-units")
        if args[opt_index].split("=", 1)[1] != "3" or args[units_index].split("=", 1)[1] != "256":
            raise SystemExit("Unexpected baseline release optimization or codegen units")
        effective = requested if target == "lib" else "3"
        if target == "lib":
            args[opt_index] = args[opt_index][:-1] + effective
        print(f"CMA_PROFILE_PROBE target={target} opt-level={effective} codegen-units=256", file=sys.stderr, flush=True)
    os.execvp(rustc, [rustc, *args])


if __name__ == "__main__":
    main()
