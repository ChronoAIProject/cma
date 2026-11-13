"""Run all Mongo contracts with bounded, independent disposable fixtures."""

import json
import math
import os
from pathlib import Path
import runpy
import signal
import subprocess
import sys
import time

TESTS_PER_FIXTURE = 40
CONCURRENT_FIXTURES = 2
SCALE_TEST = ("store::usage_reports::tests::scale::"
              "metrics_over_twenty_thousand_sandboxes_generate_full_cohort_and_indexed_pages")
OWNER_ALLOCATION_TEST = ("store::usage_reports::tests::owner_allocation::"
                         "metrics_owner_allocations_full_cohort_at_400_and_20001_is_bounded_and_cursor_bound")
AUDIT_BOUNDS_TEST = ("store::agent_log_segments::mongo_tests::audit_bounds::"
                     "near_capacity_audit_keeps_source_bounds_and_limits_each_write_transaction")
DIAGNOSTICS_TEST = ("store::mongo_diagnostics::mongo_tests::"
                    "real_mongo_monitoring_privacy_and_overhead")
SCAN_COOPERATION_TEST = ("store::agent_log_telemetry::tests::cooperation::benchmark::"
                         "local_scan_scheduling_abba_reproduction")
# The audit keeps its 30-second bound; competing fixtures must not consume its
# resource window while it validates and promotes a full 10,000-row unit.
ISOLATED_TESTS = (("--scale", SCALE_TEST), ("--owner-allocation", OWNER_ALLOCATION_TEST),
                  ("--audit-bounds", AUDIT_BOUNDS_TEST), ("--diagnostics", DIAGNOSTICS_TEST),
                  ("--scan-cooperation", SCAN_COOPERATION_TEST))
CLEANUP_FAILURE = 125


def partition_count(script):
    listing = subprocess.run(
        ["bash", str(script), "--list"], check=True, stdout=subprocess.PIPE,
        text=True,
    )
    metadata = json.loads(listing.stdout)
    runpy.run_path(str(Path(__file__).with_name("mongo-contract-completeness.py")))["check_metadata"](metadata)
    selected = [
        (name, test)
        for suite in metadata["rust-suites"].values()
        for name, test in suite["testcases"].items()
        if test["filter-match"]["status"] == "matches"
    ]
    if not selected or not all(test["ignored"] for _, test in selected):
        raise ValueError("Mongo contracts must select a nonempty ignored-test set")
    for option, expected in ISOLATED_TESTS:
        if sum(name == expected for name, _ in selected) != 1:
            raise ValueError(f"Exactly one ignored {option} contract is required")
    ordinary = len(selected) - len(ISOLATED_TESTS)
    if ordinary == 0:
        raise ValueError("Ordinary Mongo contract selection must not be empty")
    count = math.ceil(ordinary / TESTS_PER_FIXTURE)
    print(f"Mongo contracts: {len(selected)} tests, {count} partitions, "
          f"at most {TESTS_PER_FIXTURE} tests per fixture, "
          f"{CONCURRENT_FIXTURES} concurrent ordinary fixtures; "
          "report scale, owner allocation, audit bounds, diagnostics and scan cooperation run alone first", flush=True)
    return count


def run(script):
    try:
        count = partition_count(script)
    except (OSError, subprocess.CalledProcessError, ValueError, KeyError, TypeError) as error:
        print(f"Cannot enumerate Mongo contracts: {error}", file=sys.stderr)
        return 1
    children = []
    interrupted = 0
    cleanup_failed = False

    def interrupt(number, _frame):
        nonlocal interrupted
        interrupted = number

    signal.signal(signal.SIGINT, interrupt)
    signal.signal(signal.SIGTERM, interrupt)
    try:
        # A fresh fixture gets the entire resource window. Its exit includes
        # owned-container/volume cleanup before any ordinary fixture starts.
        for option, _ in ISOLATED_TESTS:
            if interrupted:
                break
            isolated = subprocess.Popen(["bash", str(script), option], start_new_session=True)
            children.append(isolated)
            while not interrupted and isolated.poll() is None:
                time.sleep(0.1)
            if isolated.returncode == CLEANUP_FAILURE:
                # A surviving fixture could invalidate the next exclusive phase.
                return 1
        next_partition = 1
        # Test failures retain full coverage. Uncertain cleanup stops allocation.
        while not interrupted:
            statuses = [child.poll() for child in children]
            if CLEANUP_FAILURE in statuses:
                cleanup_failed = True
                break
            # Use one snapshot: a child finishing after this poll still owns its
            # slot until its cleanup outcome is checked on the next iteration.
            active = sum(status is None for status in statuses)
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
    if interrupted or cleanup_failed:
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
        return 128 + interrupted if interrupted else 1
    return 0 if len(children) == count + len(ISOLATED_TESTS) and all(child.returncode == 0 for child in children) else 1


if __name__ == "__main__":
    expected = Path(__file__).with_name("mongo-contract-tests.sh").resolve()
    if len(sys.argv) != 2 or Path(sys.argv[1]).resolve() != expected:
        raise SystemExit(2)
    raise SystemExit(run(expected))
