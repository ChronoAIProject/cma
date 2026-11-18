"""Fail a reusable workflow if any required child did not prove success."""

import json
import os
import sys


def evaluate(track, results):
    required = {"lint", "tests", "timing", "mongo", "isolation"}
    if track not in {"am", "eg"} or set(results) != required:
        raise ValueError("invalid track or child inventory")
    for job in required:
        expected = "skipped" if job == "isolation" and track == "eg" else "success"
        if results[job].get("result") != expected:
            raise ValueError(f"{job} did not produce {expected}")


def main():
    try:
        evaluate(os.environ["TRACK"], json.loads(os.environ["NEEDS"]))
    except (KeyError, TypeError, ValueError) as error:
        print(f"::error::{error}")
        return 1
    print("All required Rust children passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
