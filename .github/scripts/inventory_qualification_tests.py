"""Synthetic evidence rejects incomplete or failed runs, even with exit zero."""
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from contextlib import redirect_stdout

import inventory_qualification_build as compiled
import inventory_qualification_fixture as fixture
import inventory_qualification_results as results


def records(mode):
    rows = []
    for (case, scenario), details in results.expected(mode).items():
        observation = case == results.OBSERVATION
        agents = 100 if details.get("many") else 1
        commands = 1200 if case == results.ORGANIZATION else 900 if details.get("route") == "provenance" else 800
        row = {"schema": "cma-inventory-qualification-v1", "case": case, "scenario": scenario,
            "details": details, "debug_assertions": False, "runtime_workers": 1 if observation else 4,
            "completed": True, "database_cleanup": True, "started_ms": 1000, "finished_ms": 1_802_000,
            "elapsed_us": 1_801_000_000, "warmup_completed": details.get("expected_warmup", 0),
            "samples": [{"reader": reader, "status": 200, "elapsed_us": 10_000 + reader,
                         "deadline": False, "semantic_match": True} for reader in range(100)],
            "commands": {"fixture/find": commands}, "failed_commands": 0}
        if observation:
            row.update(stable_started_ms=2000, stable_finished_ms=1_802_000,
                stable_elapsed_us=1_800_000_000, active_observers=100, agents=agents, head_agents=agents,
                snapshots=1800 * agents, max_head_refreshes=1800, minimum_refresh_gap_us=1_000_000,
                opening_commands={"fixture/find": 100})
        if case == results.AVAILABILITY:
            row.update(diagnostic_complete=True, status_counts={"200": 100})
        rows.append(row)
    return rows


def listing(mode, binary):
    return {"rust-suites": {"cma::bin/cma": {"package-name": "cma", "binary-id": "cma::bin/cma",
        "binary-name": "cma", "kind": "bin", "status": "listed", "binary-path": str(binary),
        "testcases": {name: {"ignored": True, "filter-match": {"status": "matches"}} for name in results.MODES[mode]}}}}


class ResultTests(unittest.TestCase):
    def test_complete_modes_keep_instrumentation_and_all_scenarios(self):
        for mode in results.MODES:
            with self.subTest(mode=mode):
                actual = results.validate(mode, records(mode))
                self.assertEqual(len(actual), len(results.expected(mode)))

    def test_zero_exit_cannot_replace_coverage_status_semantics_or_cleanup(self):
        mutations = {
            "missing scenario": lambda rows: rows.pop(),
            "duplicate scenario": lambda rows: rows.append(copy.deepcopy(rows[0])),
            "wrong mode": lambda rows: rows[0]["details"].update(mode="unknown"),
            "wrong route": lambda rows: rows[0]["details"].update(route="unknown"),
            "instrumentation": lambda rows: rows[0]["details"].update(instrumentation="mongo-profiler"),
            "missing reader": lambda rows: rows[0]["samples"].pop(),
            "duplicate reader": lambda rows: rows[0]["samples"][0].update(reader=1),
            "failed response": lambda rows: rows[0]["samples"][0].update(status=503),
            "deadline": lambda rows: rows[0]["samples"][0].update(deadline=True),
            "late completion": lambda rows: rows[0]["samples"][0].update(elapsed_us=60_000_000),
            "semantic drift": lambda rows: rows[0]["samples"][0].update(semantic_match=False),
            "debug binary": lambda rows: rows[0].update(debug_assertions=True),
            "missing cleanup": lambda rows: rows[0].pop("database_cleanup"),
            "failed cleanup": lambda rows: rows[0].update(database_cleanup=False),
            "incomplete": lambda rows: rows[0].update(completed=False),
            "warmup": lambda rows: rows[0].update(warmup_completed=99),
            "failed command": lambda rows: rows[0].update(failed_commands=1),
            "status histogram": lambda rows: rows[0].update(status_counts={"200": 99, "504": 1}),
        }
        for label, mutate in mutations.items():
            with self.subTest(failure=label), self.assertRaises(ValueError):
                rows = records("availability")
                mutate(rows)
                results.validate("availability", rows)

    def test_existing_latency_and_command_budgets_remain_exact(self):
        for mode in ["agent-lists", "agent-list-profile", "organization-inventory"]:
            for change in ["p95", "p99", "commands"]:
                with self.subTest(mode=mode, failure=change), self.assertRaises(ValueError):
                    rows = records(mode)
                    if change == "commands":
                        rows[0]["commands"]["fixture/find"] += 1
                    else:
                        for sample in rows[0]["samples"]:
                            sample["elapsed_us"] = 1_000_000 if change == "p95" else 2_000_000
                    results.validate(mode, rows)

    def test_observation_requires_all_three_full_windows_and_original_budgets(self):
        for field, value in [("stable_elapsed_us", 1_799_999_999), ("stable_finished_ms", 1000),
                ("active_observers", 99), ("snapshots", 0), ("max_head_refreshes", 1802),
                ("minimum_refresh_gap_us", 999_999), ("commands", {"fixture/find": 180001}),
                ("commands", {"agent_log/find": 1}), ("head_agents", 0), ("opening_commands", {})]:
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                rows = records("observation-budget")
                rows[0][field] = value
                results.validate("observation-budget", rows)

    def test_truncated_or_ambiguous_output_is_refused(self):
        for text in [results.MARKER + '{', results.MARKER + '{"case":1,"case":2}', 'prefix ' + results.MARKER + '{}']:
            with self.subTest(text=text), self.assertRaises(ValueError):
                results.parse(text)
        with self.assertRaises(ValueError):
            results.validate("availability", results.parse("test result: ok"))


class BuildTests(unittest.TestCase):
    def test_source_or_binary_drift_is_not_qualification(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "binary"
            path.write_bytes(b"synthetic executable")
            source = {"head": "a" * 40, "tree": "b" * 40}
            identity = {"source": source, "binary": str(path), "binary_sha256": compiled.digest(path)}
            with patch.object(compiled, "source", return_value=source):
                compiled.verify(identity, Path(directory))
                path.write_bytes(b"changed executable")
                with self.assertRaisesRegex(ValueError, "binary changed"):
                    compiled.verify(identity, Path(directory))
            with patch.object(compiled, "source", return_value={"head": "c" * 40, "tree": "b" * 40}):
                with self.assertRaisesRegex(ValueError, "source changed"):
                    compiled.verify(identity, Path(directory))

    def test_exact_real_inventory_requires_the_compiled_binary_and_every_case(self):
        mode, binary = "agent-list-profile", Path("/synthetic/cma")
        identity = {"binary": str(binary)}
        self.assertEqual(compiled.inventory(json.dumps(listing(mode, binary)), mode, identity), sorted(results.MODES[mode]))
        for change in ["missing", "ordinary", "path", "selection"]:
            with self.subTest(change=change), self.assertRaises(ValueError):
                data = listing(mode, binary)
                target = data["rust-suites"]["cma::bin/cma"]
                case = results.MODES[mode][0]
                if change == "missing": del target["testcases"][case]
                elif change == "ordinary": target["testcases"][case]["ignored"] = False
                elif change == "path": target["binary-path"] = "/other/binary"
                else: target["testcases"][case]["filter-match"]["status"] = "mismatch"
                compiled.inventory(json.dumps(data), mode, identity)

    def test_profile_is_proved_by_compiler_output(self):
        for opt, assertions, succeeds in [("3", False, True), ("0", False, False), ("3", True, False)]:
            with self.subTest(opt=opt, assertions=assertions), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / "api").mkdir()
                (root / "api/rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.96.1"\n')
                binary = root / "binary"
                binary.write_bytes(b"synthetic executable")
                artifact = {"reason": "compiler-artifact", "target": {"name": "cma", "kind": ["bin"]},
                    "profile": {"test": True, "opt_level": opt, "debug_assertions": assertions}, "executable": str(binary)}
                def compiler(command, **kwargs):
                    self.assertIn("--release", command)
                    self.assertIn("--locked", command)
                    kwargs["stdout"].write(json.dumps(artifact) + "\n")
                    return SimpleNamespace(returncode=0)
                with patch.object(compiled, "source", return_value={"head": "a" * 40}), \
                     patch.object(subprocess, "check_output", return_value="synthetic rustc"), \
                     patch.object(subprocess, "run", side_effect=compiler):
                    if succeeds:
                        self.assertEqual(compiled.build(root, root, {})["compiler_profile"], artifact["profile"])
                    else:
                        with self.assertRaises(ValueError): compiled.build(root, root, {})
        with self.assertRaisesRegex(ValueError, "archive"):
            compiled.build(Path("/unused"), Path("/unused"), {"CMA_NEXTEST_REUSE": "1"})


class FixtureTests(unittest.TestCase):
    def sample(self):
        return {"id": "a" * 64, "image": "sha256:" + "b" * 64, "started_at": "synthetic-start",
            "running": True, "oom_killed": False, "restarts": 0, "memory_bytes": 2147483648,
            "memory_swap_bytes": 2147483648, "nano_cpus": 2000000000, "pids_limit": 256,
            "wired_tiger_cache_gib": "0.25", "observed_ms": 1000,
            "image_reference": "mongo:8.0.13@sha256:cf340b1e5283843c63eb12999922f20c463ae31285f746d30f05dcc21cd1d47c"}

    def test_identity_resource_restart_and_cleanup_evidence(self):
        for field, value in [(None, None), ("restarts", 1), ("running", False), ("id", "c" * 64),
                             ("memory_bytes", 1073741824), ("image_reference", "mongo:latest")]:
            with self.subTest(field=field), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                with patch.object(fixture, "inspect", return_value=self.sample()):
                    fixture.update(directory, "start", name="synthetic-owned-fixture")
                sample = self.sample()
                if field: sample[field] = value
                with patch.object(fixture, "inspect", return_value=sample):
                    if field:
                        with self.assertRaises(ValueError): fixture.update(directory, "closing")
                    else:
                        fixture.update(directory, "closing")
                        fixture.update(directory, "closed", exit_code=0, test_exit=0)
                        result = json.loads((directory / "fixture.json").read_text())
                        self.assertTrue(result["owned_cleanup"])
                        self.assertEqual(len(result["samples"]), 2)
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            with patch.object(fixture, "inspect", return_value=self.sample()):
                fixture.update(directory, "start", name="synthetic-owned-fixture")
            fixture.update(directory, "closed", exit_code=125, test_exit=7)
            result = json.loads((directory / "fixture.json").read_text())
            self.assertFalse(result["owned_cleanup"])
            self.assertEqual(result["first_test_exit"], 7)


class CoordinatorTests(unittest.TestCase):
    def test_cancellation_stops_the_owned_process_and_preserves_failure(self):
        spec = importlib.util.spec_from_file_location("qualification_runner", Path(__file__).with_name("inventory-qualification.py"))
        runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runner)
        for number in (signal.SIGINT, signal.SIGTERM):
            with self.subTest(signal=number), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                class Process:
                    pid, returncode, signalled = 12345, None, False
                    def __init__(self, *_args, **_kwargs): pass
                    def poll(self):
                        if not self.signalled:
                            self.signalled = True
                            os.kill(os.getpid(), number)
                        return self.returncode
                    def wait(self, **_kwargs): self.returncode = 130
                listed = SimpleNamespace(returncode=0, stdout=json.dumps(listing("availability", "/synthetic/cma")))
                previous = signal.getsignal(number)
                with patch.object(compiled, "build", return_value={"binary": "/synthetic/cma"}), \
                     patch.object(compiled, "verify"), patch.object(subprocess, "run", return_value=listed), \
                     patch.object(subprocess, "Popen", Process), patch.object(os, "killpg") as stop, redirect_stdout(io.StringIO()):
                    self.assertEqual(runner.run("availability", directory, {}), 1)
                stop.assert_called_once_with(12345, signal.SIGINT)
                self.assertEqual(signal.getsignal(number), previous)
                receipt = json.loads((directory / "qualification.json").read_text())
                self.assertFalse(receipt["passed"])
                self.assertEqual(receipt["runner_exit"], 130)
                self.assertIn(f"signal {number}", receipt["failure"])

    def test_diagnostic_exit_zero_cannot_sign_a_bad_qualification(self):
        spec = importlib.util.spec_from_file_location("qualification_runner", Path(__file__).with_name("inventory-qualification.py"))
        runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runner)
        for failure in [None, "response", "cleanup", "process"]:
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                rows = records("availability")
                if failure == "response": rows[0]["samples"][0]["status"] = 503
                identity = {"binary": "/synthetic/cma"}
                state = {"phase": "closed", "owned_cleanup": failure != "cleanup", "errors": [],
                         "samples": [FixtureTests().sample(), FixtureTests().sample()], "first_test_exit": 0,
                         "runner_exit": 0, "name": "cma-mongo-ci-" + "a" * 24}
                class Process:
                    returncode = 7 if failure == "process" else 0
                    def __init__(self, _command, **kwargs):
                        kwargs["stdout"].write("\n".join(results.MARKER + json.dumps(row) for row in rows) + "\n")
                        kwargs["stdout"].flush()
                        (directory / "fixture.json").write_text(json.dumps(state))
                    def poll(self): return self.returncode
                listed = SimpleNamespace(returncode=0, stdout=json.dumps(listing("availability", "/synthetic/cma")))
                with patch.object(compiled, "build", return_value=identity), patch.object(compiled, "verify"), \
                     patch.object(subprocess, "run", return_value=listed), patch.object(subprocess, "Popen", Process), redirect_stdout(io.StringIO()):
                    code = runner.run("availability", directory, {})
                receipt = json.loads((directory / "qualification.json").read_text())
                self.assertEqual(code, 0 if failure is None else 1)
                self.assertEqual(receipt["passed"], failure is None)
                self.assertEqual(receipt["attempt"], 1)


if __name__ == "__main__":
    unittest.main()
