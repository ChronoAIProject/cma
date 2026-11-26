"""Run one partition of the frozen scanner suite and validate Repo job results."""

import argparse
from collections import Counter
import json
import os
from pathlib import Path
import re
import sys
import unittest


BUNDLE = Path(__file__).resolve().parents[1] / "image-secret-scanner"
SHARDS = (0, 1)


def iter_cases(suite):
    for test in suite:
        if isinstance(test, unittest.TestSuite):
            yield from iter_cases(test)
        else:
            yield test


def partition(discovered, known_modules):
    indices = {module: index % len(SHARDS) for index, module in enumerate(sorted(known_modules))}
    shards = tuple([] for _ in SHARDS)
    seen = set()
    for test in discovered:
        # Discovery loads test_scanner under two names. Keep both copies,
        # including the live Docker fixture, on the same shard.
        module = type(test).__module__.removeprefix("tests.")
        if module not in indices:
            raise ValueError(f"unknown scanner test module: {module}")
        seen.add(module)
        shards[indices[module]].append(test)
    if seen != known_modules:
        raise ValueError("scanner discovery did not cover every frozen test module")
    validate_partition(discovered, shards)
    return shards


def validate_partition(discovered, shards):
    if len(shards) != len(SHARDS) or any(not shard for shard in shards):
        raise ValueError("every scanner shard must contain tests")
    expected = Counter(test.id() for test in discovered)
    actual = Counter()
    for shard in shards:
        counts = Counter(test.id() for test in shard)
        if actual.keys() & counts.keys():
            raise ValueError("scanner shards must be disjoint")
        actual.update(counts)
    if actual != expected:
        raise ValueError("scanner shards must cover the full test inventory")


def discover_shards(bundle):
    manifest = json.loads((bundle / "SOURCE-MANIFEST.json").read_text())
    known_modules = {
        Path(entry["path"]).stem
        for entry in manifest["files"]
        if re.fullmatch(r"tests/test_\w+\.py", entry["path"])
    }
    sys.path.insert(0, str(bundle))
    loader = unittest.TestLoader()
    discovered = list(iter_cases(loader.discover(str(bundle / "tests"))))
    if loader.errors:
        raise ValueError("frozen scanner test discovery failed")
    return partition(discovered, known_modules)


def validate_gate(needs, sha):
    required = {"structure", "secrets", "scanner-tests"}
    if set(needs) != required or any(needs[job].get("result") != "success" for job in required):
        raise ValueError("all repository checks and scanner shards must succeed")
    expected = {f"shard_{shard}": sha for shard in SHARDS}
    outputs = needs["scanner-tests"].get("outputs", {})
    if not re.fullmatch(r"[0-9a-f]{40}", sha) or outputs != expected:
        raise ValueError("both scanner shards must report success on the candidate SHA")


def main():
    parser = argparse.ArgumentParser()
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--shard", type=int, choices=SHARDS)
    mode.add_argument("--check-gate", action="store_true")
    args = parser.parse_args()
    try:
        if args.check_gate:
            validate_gate(json.loads(os.environ["NEEDS"]), os.environ["GITHUB_SHA"])
            return 0
        shard = discover_shards(BUNDLE)[args.shard]
        result = unittest.TextTestRunner(verbosity=2).run(unittest.TestSuite(shard))
        return 0 if result.wasSuccessful() and result.testsRun == len(shard) else 1
    except (KeyError, TypeError, ValueError, OSError) as error:
        print(f"Repository scanner checks failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
