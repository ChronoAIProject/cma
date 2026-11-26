"""Exact shard coverage and fail-closed required-check regressions."""

import copy
import io
import json
import os
import runpy
import signal
import subprocess
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).parent
PARTITIONS = runpy.run_path(str(ROOT / "mongo-contract-partitions.py"))
GATE = runpy.run_path(str(ROOT / "mongo-contract-gate.py"))
SCHEDULE = PARTITIONS["SCHEDULE"]
DiscoveryTests = runpy.run_path(str(ROOT / "mongo-contract-discovery-tests.py"))["DiscoveryTests"]
ListGuardTests = runpy.run_path(str(ROOT / "mongo-list-guard-tests.py"))["ListGuardTests"]


class SchedulingTests(unittest.TestCase):
    def test_weighted_matrix_keeps_every_partition_exactly_once(self):
        for count in (1, 2, 7, 8, 9, 43, 44, 101):
            costs = [1 + (index * 79) % 1300 for index in range(count)]
            actual = []
            for index in range(1, 9):
                special, assigned = PARTITIONS["shard_work"](count, f"{index}/8", costs)
                self.assertEqual(special, ())
                actual.extend(assigned)
            self.assertEqual(sorted(actual), list(range(1, count + 1)))
            self.assertEqual(len(actual), len(set(actual)))
            self.assertEqual(PARTITIONS["shard_work"](count, "isolated", costs),
                             (PARTITIONS["ISOLATED_TESTS"], []))
            self.assertEqual(PARTITIONS["shard_work"](count, None, costs)[1], list(range(1, count + 1)))

    def test_expensive_partitions_start_first_on_separate_runners(self):
        costs = [20] * 44
        costs[10], costs[26] = 600, 1200
        assigned = SCHEDULE["assign_partitions"](costs, 8, 2)
        self.assertEqual(assigned[0][0], 27)
        self.assertEqual(assigned[1][0], 11)

        def duration(partitions):
            slots = [0, 0]
            for partition in partitions:
                worker = min(range(2), key=lambda index: slots[index])
                slots[worker] += costs[partition - 1]
            return max(slots)

        previous = [list(range(index, 45, 8)) for index in range(1, 9)]
        self.assertLess(max(map(duration, assigned)), max(map(duration, previous)))
        self.assertEqual(assigned, SCHEDULE["assign_partitions"](costs, 8, 2))

    def test_uniform_estimates_preserve_round_robin_assignment(self):
        self.assertEqual(SCHEDULE["assign_partitions"]([40] * 43, 8, 2),
                         [list(range(index, 44, 8)) for index in range(1, 9)])

    def test_nextest_count_order_resets_for_each_binary(self):
        def cases(names):
            return {name: {"filter-match": {"status": "matches"}} for name in names}
        metadata = {"rust-suites": {"first": {"testcases": cases(["c", "z", "b", "a"])},
                                   "second": {"testcases": cases(["d"])}}}
        metadata["rust-suites"]["first"]["testcases"]["unselected"] = {"filter-match": {"status": "mismatch"}}
        model = {"default_seconds": 1, "fixture_seconds": 5, "tests": {"a": 100, "b": 10, "d": 20, "stale": 999}}
        self.assertEqual(SCHEDULE["partition_costs"](metadata, 2, {"z"}, model), [126, 17])
        model["tests"] = {}
        self.assertEqual(SCHEDULE["partition_costs"](metadata, 2, {"z"}, model), [8, 8])

    def test_invalid_models_and_bounds_refuse(self):
        model = {"default_seconds": 1, "fixture_seconds": 15, "tests": {"synthetic": 10}}
        metadata = {"rust-suites": {}}
        for bad in (None, {}, dict(model, unexpected=1), dict(model, tests={"": 10})):
            with self.subTest(model=bad), self.assertRaises(ValueError):
                SCHEDULE["partition_costs"](metadata, 1, set(), bad)
        with self.assertRaises(TypeError):
            SCHEDULE["partition_costs"](metadata, 1, set(), dict(model, tests=[]))
        for bad in (0, -1, True, 1.5, float("nan"), "20"):
            for key in ("default_seconds", "fixture_seconds"):
                with self.subTest(key=key, value=bad), self.assertRaises(ValueError):
                    SCHEDULE["partition_costs"](metadata, 1, set(), dict(model, **{key: bad}))
            with self.assertRaises(ValueError):
                SCHEDULE["partition_costs"](metadata, 1, set(), dict(model, tests={"synthetic": bad}))
            with self.assertRaises(ValueError):
                SCHEDULE["assign_partitions"]([1], bad, 2)
            with self.assertRaises(ValueError):
                SCHEDULE["assign_partitions"]([1], 8, bad)
            with self.assertRaises(ValueError):
                SCHEDULE["assign_partitions"]([bad], 8, 2)
        for costs in ([], None, "invalid"):
            with self.assertRaises(ValueError):
                SCHEDULE["assign_partitions"](costs, 8, 2)
        with self.assertRaises(ValueError):
            PARTITIONS["shard_work"](2, "1/8", [10])


class ShardTests(unittest.TestCase):
    def test_matrix_union_matches_local_execution_without_duplicates(self):
        select = PARTITIONS["shard_work"]
        for count in (1, 2, 7, 8, 9, 43, 44, 101):
            expected_isolated, expected = select(count, None)
            isolated, partitions = [], []
            for shard in ["isolated", *(f"{i}/8" for i in range(1, 9))]:
                special, assigned = select(count, shard)
                isolated.extend(special)
                partitions.extend(assigned)
            self.assertEqual(isolated, list(expected_isolated))
            self.assertEqual(sorted(partitions), expected)
            self.assertEqual(len(partitions), len(set(partitions)))

    def test_coordinates_do_not_silently_change_matrix_cardinality(self):
        for shard in ("", "0/8", "9/8", "1/7", "1/9", "1/08", "01/8", "all", "1/8\n"):
            with self.subTest(shard=shard), self.assertRaises(ValueError):
                PARTITIONS["shard_work"](43, shard)

    def test_historical_selection_and_fixture_bounds_are_preserved(self):
        self.assertEqual(PARTITIONS["TESTS_PER_FIXTURE"], 40)
        self.assertEqual(PARTITIONS["CONCURRENT_FIXTURES"], 2)
        self.assertEqual(PARTITIONS["CI_SHARDS"], 8)
        self.assertEqual(PARTITIONS["ISOLATED_TESTS"], (
            ("--scale", PARTITIONS["SCALE_TEST"]),
            ("--owner-allocation", PARTITIONS["OWNER_ALLOCATION_TEST"]),
            ("--audit-bounds", PARTITIONS["AUDIT_BOUNDS_TEST"]),
            ("--diagnostics", PARTITIONS["DIAGNOSTICS_TEST"]),
            ("--scan-cooperation", PARTITIONS["SCAN_COOPERATION_TEST"]),
            ("--bson-ceiling", PARTITIONS["BSON_CEILING_TEST"]),
            ("--first-response", PARTITIONS["FIRST_RESPONSE_TEST"]),
            ("--history", "store::usage_reports::tests::history_batch::seven_day_history_generates_and_reuses_complete_421_member_report"),
        ))
        self.assertEqual(len(PARTITIONS["ISOLATED_TESTS"]), 8)
        script = (ROOT / "mongo-contract-tests.sh").read_text()
        self.assertIn("--test-threads 1 --no-fail-fast", script)
        self.assertEqual(script.count("readiness_deadline=$((SECONDS + 60))"), 2)
        self.assertEqual(script.count("sleep 0.25"), 2)
        self.assertNotIn("sleep 1", script)

    def test_partition_enumeration_audits_complete_ignored_inventory(self):
        exceptions = json.loads((ROOT / "mongo-contract-exceptions.json").read_text())
        for ordinary, expected in ((1, 1), (40, 1), (41, 2), (1715, 43)):
            cases = {name: {"ignored": True, "filter-match": {"status": "mismatch"}} for name in exceptions}
            cases.update({name: {"ignored": True, "filter-match": {"status": "matches"}}
                          for _, name in PARTITIONS["ISOLATED_TESTS"]})
            cases.update({f"synthetic::contract_{i}": {"ignored": True, "filter-match": {"status": "matches"}}
                          for i in range(ordinary)})
            metadata = {"rust-suites": {"cma::bin/cma": {"package-name": "cma", "binary-name": "cma",
                                                        "kind": "bin", "testcases": cases}}}
            completed = subprocess.CompletedProcess([], 0, json.dumps(metadata))
            with patch.object(subprocess, "run", return_value=completed), redirect_stdout(io.StringIO()):
                count, costs = PARTITIONS["partition_plan"](ROOT / "mongo-contract-tests.sh")
                self.assertEqual(count, expected)
                self.assertEqual(len(costs), expected)
                self.assertTrue(all(cost > 0 for cost in costs))
            del cases[PARTITIONS["SCALE_TEST"]]
            completed.stdout = json.dumps(metadata)
            with patch.object(subprocess, "run", return_value=completed), redirect_stdout(io.StringIO()), self.assertRaises(ValueError):
                PARTITIONS["partition_plan"](ROOT / "mongo-contract-tests.sh")


class GateTests(unittest.TestCase):
    def needs(self, track):
        return {name: {"result": "success" if ((name == "mongo-eg") == (track == "eg")) else "skipped"}
                for name in ("mongo-harness", "mongo-shards", "mongo-eg")}

    def test_both_tracks_require_their_exact_execution_graph(self):
        for track in ("am", "eg"):
            GATE["evaluate"](track, self.needs(track))
            for name in self.needs(track):
                for status in ("failure", "cancelled", "pending", None):
                    value = self.needs(track)
                    value[name]["result"] = status
                    with self.subTest(track=track, name=name, status=status), self.assertRaises(ValueError):
                        GATE["evaluate"](track, value)
                value = self.needs(track)
                value[name]["result"] = "skipped" if value[name]["result"] == "success" else "success"
                with self.assertRaises(ValueError):
                    GATE["evaluate"](track, value)

    def test_missing_extra_and_malformed_jobs_refuse(self):
        good = self.needs("am")
        for name in good:
            missing = copy.deepcopy(good)
            del missing[name]
            with self.assertRaises(ValueError):
                GATE["evaluate"]("am", missing)
            malformed = copy.deepcopy(good)
            malformed[name] = None
            with self.assertRaises(ValueError):
                GATE["evaluate"]("am", malformed)
        for value in (None, [], {}, dict(good, unexpected={"result": "success"})):
            with self.assertRaises(ValueError):
                GATE["evaluate"]("am", value)
        with self.assertRaises(ValueError):
            GATE["evaluate"]("other", good)

    def test_entrypoint_exit_code_cannot_hide_failed_or_missing_shards(self):
        for track in ("am", "eg"):
            env = {"TRACK": track, "NEEDS": json.dumps(self.needs(track))}
            with patch.dict(os.environ, env, clear=True), redirect_stdout(io.StringIO()):
                self.assertEqual(GATE["main"](), 0)
        for env in ({}, {"TRACK": "am", "NEEDS": "invalid"}, {"TRACK": "am", "NEEDS": "{}"}):
            with patch.dict(os.environ, env, clear=True), redirect_stdout(io.StringIO()):
                self.assertEqual(GATE["main"](), 1)


class CoordinatorTests(unittest.TestCase):
    def exercise(self, shard, statuses, *, interrupt=False, enumeration_error=False, spawn_error=False):
        commands, children, signals = [], [], {}

        class Child:
            def __init__(self, status):
                self.returncode = status
                self.pid = len(children) + 100

            def poll(self):
                return self.returncode

            def wait(self, timeout=None):
                self.returncode = -signal.SIGTERM

        def spawn(command, **options):
            self.assertTrue(options["start_new_session"])
            if spawn_error:
                raise OSError("fixture spawn failed")
            commands.append(command)
            child = Child(statuses.get(command[-1], 0))
            children.append(child)
            if interrupt:
                signals[signal.SIGTERM](signal.SIGTERM, None)
            return child

        globals_ = PARTITIONS["run"].__globals__
        with patch.dict(globals_, partition_plan=lambda _: (_ for _ in ()).throw(ValueError("bad inventory")) if enumeration_error else (43, [1] * 43)), \
                patch.object(subprocess, "Popen", side_effect=spawn), \
                patch.object(signal, "signal", side_effect=lambda number, handler: signals.update({number: handler})), \
                patch.object(globals_["time"], "sleep"), patch.object(os, "killpg") as kill, \
                redirect_stdout(io.StringIO()):
            code = PARTITIONS["run"](ROOT / "mongo-contract-tests.sh", shard)
        return code, commands, children, kill.call_count

    def test_shard_attempts_every_assigned_partition_after_test_failure(self):
        for statuses, expected in (({}, 0), ({"count:1/43": 7}, 1)):
            code, commands, _, _ = self.exercise("1/8", statuses)
            self.assertEqual(code, expected)
            self.assertEqual([row[-1] for row in commands], [f"count:{i}/43" for i in (1, 9, 17, 25, 33, 41)])

    def test_isolated_and_local_execution_preserve_every_phase(self):
        code, commands, _, _ = self.exercise("isolated", {"--scale": 7})
        self.assertEqual(code, 1)
        self.assertEqual([row[-1] for row in commands], [option for option, _ in PARTITIONS["ISOLATED_TESTS"]])
        code, commands, _, _ = self.exercise(None, {})
        self.assertEqual(code, 0)
        self.assertEqual(len(commands), 43 + len(PARTITIONS["ISOLATED_TESTS"]))

    def test_cleanup_failure_stops_new_fixtures_and_cancellation_reaps_children(self):
        code, commands, _, _ = self.exercise("isolated", {"--scale": 125})
        self.assertEqual((code, len(commands)), (1, 1))
        code, commands, _, _ = self.exercise("1/8", {"count:1/43": 125})
        self.assertEqual(code, 1)
        self.assertLess(len(commands), 6)
        code, _, children, kills = self.exercise("1/8", {"count:1/43": None}, interrupt=True)
        self.assertEqual(code, 128 + signal.SIGTERM)
        self.assertEqual(kills, 1)
        self.assertTrue(all(child.returncode is not None for child in children))

    def test_bad_inventory_and_spawn_failures_never_succeed(self):
        code, commands, _, _ = self.exercise("1/8", {}, enumeration_error=True)
        self.assertEqual((code, commands), (1, []))
        code, _, _, _ = self.exercise("1/8", {}, spawn_error=True)
        self.assertEqual(code, 128 + signal.SIGTERM)


if __name__ == "__main__":
    unittest.main()
