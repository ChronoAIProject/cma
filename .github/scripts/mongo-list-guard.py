#!/usr/bin/env python3
"""Bound nextest's binary listing inside each ordinary Mongo partition."""

from pathlib import Path
import runpy
import subprocess
import sys


listing = runpy.run_path(str(Path(__file__).with_name("mongo-contract-partitions.py")))["archived_listing"]


def main(arguments):
    # Only the two libtest discovery forms may be retried. This wrapper must
    # never execute a test body, even if attached to the wrong nextest phase.
    if len(arguments) < 2 or arguments[1:] not in (
        ["--list", "--format", "terse"],
        ["--list", "--format", "terse", "--ignored"],
    ):
        print("Mongo listing guard requires an exact libtest discovery command", file=sys.stderr)
        return 2
    try:
        output = listing(arguments)
    except subprocess.TimeoutExpired:
        print("Mongo binary listing remained stalled after bounded recovery", file=sys.stderr)
        return 124
    except subprocess.CalledProcessError as error:
        return error.returncode if error.returncode > 0 else 128 - error.returncode
    except (OSError, UnicodeError):
        print("Mongo binary listing could not complete", file=sys.stderr)
        return 1
    sys.stdout.write(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
