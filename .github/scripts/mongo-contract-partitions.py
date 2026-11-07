"""Run all Mongo contracts with bounded, independent disposable fixtures."""

import json
import math
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

TESTS_PER_FIXTURE = 40
CONCURRENT_FIXTURES = 2
SCALE_TEST = ("store::usage_reports::tests::scale::"
              "metrics_over_twenty_thousand_sandboxes_generate_full_cohort_and_indexed_pages")


def partition_count(script):
    listing = subprocess.run(
        ["bash", str(script), "--list"], check=True, stdout=subprocess.PIPE,
        text=True,
    )
    metadata = json.loads(listing.stdout)
    selected = [
        (name, test)
        for suite in metadata["rust-suites"].values()
        for name, test in suite["testcases"].items()
        if test["filter-match"]["status"] == "matches"
    ]
    if not selected or not all(test["ignored"] for _, test in selected):
        raise ValueError("Mongo contracts must select a nonempty ignored-test set")
    if sum(name == SCALE_TEST for name, _ in selected) != 1:
        raise ValueError("Exactly one ignored reporting scale contract is required")
    ordinary = len(selected) - 1
    if ordinary == 0:
        raise ValueError("Ordinary Mongo contract selection must not be empty")
    count = math.ceil(ordinary / TESTS_PER_FIXTURE)
    print(f"Mongo contracts: {len(selected)} tests, {count} partitions, "
          f"at most {TESTS_PER_FIXTURE} tests per fixture, "
          f"{CONCURRENT_FIXTURES} concurrent ordinary fixtures; scale runs alone first", flush=True)
    return count


def run(script):
    try:
        count = partition_count(script)
    except (OSError, subprocess.CalledProcessError, ValueError, KeyError, TypeError) as error:
        print(f"Cannot enumerate Mongo contracts: {error}", file=sys.stderr)
        return 1
    children = []
    interrupted = 0

    def interrupt(number, _frame):
        nonlocal interrupted
        interrupted = number

    signal.signal(signal.SIGINT, interrupt)
    signal.signal(signal.SIGTERM, interrupt)
    try:
        # A fresh fixture gets the entire resource window. Its exit includes
        # owned-container/volume cleanup before any ordinary fixture starts.
        scale = subprocess.Popen(["bash", str(script), "--scale"], start_new_session=True)
        children.append(scale)
        while not interrupted and scale.poll() is None:
            time.sleep(0.1)
        next_partition = 1
        # Failures remain failures, while every remaining partition still runs.
        while not interrupted:
            active = sum(child.poll() is None for child in children)
            while not interrupted and active < CONCURRENT_FIXTURES and next_partition <= count:
                partition = f"count:{next_partition}/{count}"
                children.append(subprocess.Popen(
                    ["bash", str(script), partition], start_new_session=True,
                ))
                next_partition += 1
                active += 1
            if next_partition > count and active == 0:
                break
            time.sleep(0.1)
    except OSError:
        interrupted = signal.SIGTERM
    if interrupted:
        for child in children:
            if child.poll() is None:
                try:
                    os.killpg(child.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
        # Allow container teardown and owned disk-volume removal to finish.
        # A second TERM must not interrupt the child's cleanup trap.
        deadline = time.monotonic() + 30
        for child in children:
            try:
                child.wait(timeout=max(0, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait()
        return 128 + interrupted
    return 0 if len(children) == count + 1 and all(child.returncode == 0 for child in children) else 1


if __name__ == "__main__":
    expected = Path(__file__).with_name("mongo-contract-tests.sh").resolve()
    if len(sys.argv) != 2 or Path(sys.argv[1]).resolve() != expected:
        raise SystemExit(2)
    raise SystemExit(run(expected))
