"""Run all Mongo contracts with bounded, independent disposable fixtures."""

import json
import math
import os
import runpy
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

TESTS_PER_FIXTURE = 40
CONCURRENT_FIXTURES = 2
CI_SHARDS = 8
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
BSON_CEILING_TEST = ("store::sandbox_mongo_integration_tests::resource_access_tests::"
                     "personal_inventory::failures::"
                     "personal_agent_list_preserves_sources_near_the_bson_document_ceiling")
FIRST_RESPONSE_TEST = ("store::usage_reports::tests::submission::"
                       "default_day_report_serves_four_hundred_sandboxes_without_polling")
HISTORY_TEST = ("store::usage_reports::tests::history_batch::"
                "seven_day_history_generates_and_reuses_complete_421_member_report")
RANKED_PROJECTION_TEST = ("store::usage_reports::tests::projection_scale::"
                          "aggregate_projection_ranges_measure_sparse_and_dense_frozen_sources")
RESPONSE_REPLAY_TEST = ("store::sandbox_mongo_integration_tests::resource_access_tests::"
                       "history_measurement::observation_budget::response_shared::"
                       "shared_adopted_response_complete_routes_meet_unchanged_budgets_and_exact_replay")
# The audit keeps its 30-second bound; competing fixtures must not consume its
# resource window while it validates and promotes a full 10,000-row unit.
# Ranked projection and response replay also measure fixed windows.
ISOLATED_TESTS = (("--scale", SCALE_TEST), ("--owner-allocation", OWNER_ALLOCATION_TEST),
                  ("--audit-bounds", AUDIT_BOUNDS_TEST), ("--diagnostics", DIAGNOSTICS_TEST),
                  ("--scan-cooperation", SCAN_COOPERATION_TEST), ("--bson-ceiling", BSON_CEILING_TEST),
                  ("--first-response", FIRST_RESPONSE_TEST), ("--history", HISTORY_TEST),
                  ("--ranked-projection", RANKED_PROJECTION_TEST),
                  ("--response-replay", RESPONSE_REPLAY_TEST))
CLEANUP_FAILURE = 125
DISCOVERY_TIMEOUT_SECONDS = 120
SCHEDULE = runpy.run_path(str(Path(__file__).with_name("mongo-contract-schedule.py")))


def discovery_diagnostics(group, proc=Path("/proc")):
    """Report owned process states, never arguments, environment or output."""
    rows = []
    try:
        for path in proc.iterdir():
            if not path.name.isdecimal():
                continue
            try:
                fields = (path / "stat").read_text().rsplit(")", 1)[1].split()
                if int(fields[2]) != group:
                    continue
                rows.append({"pid": int(path.name), "state": fields[0],
                             "name": (path / "comm").read_text().strip(),
                             "wait": (path / "wchan").read_text().strip()})
                if len(rows) == 64:
                    break
            except (OSError, ValueError, IndexError):
                continue
    except OSError:
        pass
    print(f"Mongo discovery processes: {json.dumps(rows)}", file=sys.stderr, flush=True)


def archived_listing(command):
    interrupted = 0

    def interrupt(number, _frame):
        nonlocal interrupted
        interrupted = number

    previous = {number: signal.signal(number, interrupt)
                for number in (signal.SIGINT, signal.SIGTERM)}
    try:
        for attempt in range(2):
            if interrupted:
                raise SystemExit(128 + interrupted)
            # A fresh file prevents partial inventory from a stalled attempt
            # reaching completeness checks, and avoids another output pipe.
            with tempfile.TemporaryFile(mode="w+") as output:
                child = subprocess.Popen(command, stdout=output, start_new_session=True)
                try:
                    try:
                        deadline = time.monotonic() + DISCOVERY_TIMEOUT_SECONDS
                        while child.poll() is None:
                            if interrupted:
                                raise SystemExit(128 + interrupted)
                            remaining = deadline - time.monotonic()
                            if remaining <= 0:
                                raise subprocess.TimeoutExpired(command, DISCOVERY_TIMEOUT_SECONDS)
                            try:
                                child.wait(timeout=min(0.1, remaining))
                            except subprocess.TimeoutExpired:
                                pass
                    except subprocess.TimeoutExpired:
                        print(f"Mongo discovery timed out (attempt {attempt + 1}/2)",
                              file=sys.stderr, flush=True)
                        discovery_diagnostics(child.pid)
                        if attempt == 1:
                            raise
                        continue
                    if child.returncode:
                        raise subprocess.CalledProcessError(child.returncode, command)
                    output.seek(0)
                    return output.read()
                finally:
                    # Discovery has no fixtures or test effects to settle.
                    # Kill the complete owned group before retry or cancellation.
                    try:
                        os.killpg(child.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    child.wait(timeout=5)
    finally:
        for number, handler in previous.items():
            signal.signal(number, handler)
        if interrupted:
            raise SystemExit(128 + interrupted)


def partition_plan(script):
    command = ["bash", str(script), "--list"]
    if os.environ.get("CMA_NEXTEST_REUSE") == "1":
        listing = archived_listing(command)
    else:
        # Local discovery may first compile the workspace. Its build time is
        # outside the short budget for listing already archived CI binaries.
        listing = subprocess.run(command, check=True, stdout=subprocess.PIPE, text=True).stdout
    metadata = json.loads(listing)
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
          f"{len(ISOLATED_TESTS)} isolated contracts run alone first", flush=True)
    with Path(__file__).with_name("mongo-contract-timings.json").open() as source:
        model = json.load(source)
    costs = SCHEDULE["partition_costs"](metadata, count, {name for _, name in ISOLATED_TESTS}, model)
    return count, costs


def shard_work(count, shard, costs=None):
    """Assign existing nextest partitions without changing their test filters."""
    if shard is None:
        return ISOLATED_TESTS, list(range(1, count + 1))
    if shard == "isolated":
        return ISOLATED_TESTS, []
    choices = {f"{index}/{CI_SHARDS}": index for index in range(1, CI_SHARDS + 1)}
    if shard not in choices:
        raise ValueError("Expected isolated or one of the eight ordinary CI shards")
    if costs is not None:
        if len(costs) != count:
            raise ValueError("Partition costs do not match the complete inventory")
        return (), SCHEDULE["assign_partitions"](costs, CI_SHARDS, CONCURRENT_FIXTURES)[choices[shard] - 1]
    return (), list(range(choices[shard], count + 1, CI_SHARDS))


def run(script, shard=None):
    try:
        shard_work(1, shard)  # Reject malformed coordinates before enumeration.
        count, costs = partition_plan(script)
        isolated_tests, partitions = shard_work(count, shard, costs)
    except (OSError, subprocess.SubprocessError, ValueError, KeyError, TypeError) as error:
        print(f"Cannot enumerate Mongo contracts: {error}", file=sys.stderr)
        return 1
    children = []
    interrupted = 0
    cleanup_failed = False
    fixture_env = dict(os.environ, CMA_MONGO_COORDINATOR_SHARD=shard or "all")
    print(f"Mongo shard {shard or 'all'}: {len(isolated_tests)} isolated contracts, "
          f"partitions {partitions} of {count}", flush=True)

    def interrupt(number, _frame):
        nonlocal interrupted
        interrupted = number

    signal.signal(signal.SIGINT, interrupt)
    signal.signal(signal.SIGTERM, interrupt)
    try:
        # A fresh fixture gets the entire resource window. Its exit includes
        # owned-container/volume cleanup before any ordinary fixture starts.
        for option, _ in isolated_tests:
            if interrupted:
                break
            isolated = subprocess.Popen(["bash", str(script), option], env=fixture_env, start_new_session=True)
            children.append(isolated)
            while not interrupted and isolated.poll() is None:
                time.sleep(0.1)
            if isolated.returncode == CLEANUP_FAILURE:
                # A surviving fixture could invalidate the next exclusive phase.
                return 1
        next_partition = 0
        # Test failures retain full coverage. Uncertain cleanup stops allocation.
        while not interrupted:
            statuses = [child.poll() for child in children]
            if CLEANUP_FAILURE in statuses:
                cleanup_failed = True
                break
            # Use one snapshot: a child finishing after this poll still owns its
            # slot until its cleanup outcome is checked on the next iteration.
            active = sum(status is None for status in statuses)
            while not interrupted and active < CONCURRENT_FIXTURES and next_partition < len(partitions):
                partition = f"count:{partitions[next_partition]}/{count}"
                children.append(subprocess.Popen(
                    ["bash", str(script), partition], env=fixture_env, start_new_session=True,
                ))
                next_partition += 1
                active += 1
            if next_partition == len(partitions) and active == 0:
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
    return 0 if len(children) == len(partitions) + len(isolated_tests) and all(child.returncode == 0 for child in children) else 1


if __name__ == "__main__":
    expected = Path(__file__).with_name("mongo-contract-tests.sh").resolve()
    if len(sys.argv) not in (2, 4) or Path(sys.argv[1]).resolve() != expected:
        raise SystemExit(2)
    if len(sys.argv) == 4 and sys.argv[2] != "--shard":
        raise SystemExit(2)
    raise SystemExit(run(expected, sys.argv[3] if len(sys.argv) == 4 else None))
