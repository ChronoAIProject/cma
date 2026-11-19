"""Keep the required Mongo check stable while every matrix child is mandatory."""

import json
import os
import sys


def evaluate(track, results):
    expected = {"mongo-harness", "mongo-shards", "mongo-eg"}
    if track not in {"am", "eg"} or not isinstance(results, dict) or set(results) != expected:
        raise ValueError("Invalid Mongo track or job inventory")
    for name in sorted(expected):
        selected = (name == "mongo-eg") == (track == "eg")
        required = "success" if selected else "skipped"
        if not isinstance(results[name], dict) or results[name].get("result") != required:
            raise ValueError(f"{name} did not produce {required}")


def main():
    try:
        evaluate(os.environ["TRACK"], json.loads(os.environ["NEEDS"]))
    except (KeyError, TypeError, ValueError) as error:
        print(f"::error::{error}")
        return 1
    print("Every selected Mongo contract shard passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
