#!/usr/bin/env python3
"""One first-attempt qualification, using the existing owned Mongo fixture."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

import inventory_qualification_build as compiled
import inventory_qualification_fixture as fixture
from inventory_qualification_results import MODES, parse, require, validate

ROOT = Path(__file__).resolve().parents[2]


def environment(directory):
    env = {key: value for key, value in os.environ.items() if key not in {"RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS",
        "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_RUSTFLAGS",
        "CMA_INVENTORY_QUALIFICATION_DIR"} and not key.endswith("_RUSTFLAGS")}
    env.update(CARGO_INCREMENTAL="0", CARGO_PROFILE_RELEASE_OPT_LEVEL="3", CARGO_PROFILE_RELEASE_DEBUG="0",
        CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS="false", CARGO_BUILD_JOBS="4", RUST_MIN_STACK="33554432",
        CMA_MONGO_CPUS="2", CMA_LIST_TEST_WORKERS="4", CMA_INVENTORY_QUALIFICATION_DIR=str(directory))
    return env


def run(mode, directory, env):
    def interrupted(number, _frame):
        raise ValueError(f"qualification interrupted by signal {number}")

    previous = {number: signal.signal(number, interrupted) for number in (signal.SIGINT, signal.SIGTERM)}
    try:
        return execute(mode, directory, env)
    finally:
        for number, handler in previous.items():
            signal.signal(number, handler)


def execute(mode, directory, env):
    runner = ["bash", str(ROOT / ".github/scripts/mongo-contract-tests.sh")]
    receipt = {"schema": "cma-inventory-run-v1", "mode": mode, "attempt": 1, "passed": False,
        "claim": "one candidate run against the original correctness and operation/latency budgets; no performance comparison",
        "host": {"cores": os.cpu_count(), "load_before": os.getloadavg(), "exclusive_host": False},
        "started_ms": time.time_ns() // 1_000_000}
    child = None
    phase = "build"
    try:
        require(mode in MODES, "unsupported qualification mode")
        require(env.get("CMA_LIST_TEST_MODE", "both") == "both" and env.get("CMA_LIST_TEST_ROUTE", "both") == "both",
                "partial availability settings are diagnostic only")
        identity = compiled.build(ROOT, directory, env)
        receipt["build"] = identity
        (directory / "build.json").write_text(json.dumps(identity, indent=2) + "\n")
        phase = "inventory"
        listed = subprocess.run(runner + ["--list-qualification", "--" + mode], cwd=ROOT, env=env,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        require(listed.returncode == 0, "test inventory failed")
        selected = compiled.inventory(listed.stdout, mode, identity)
        receipt["selected_tests"] = selected
        (directory / "inventory.json").write_text(listed.stdout)
        receipt["inventory_sha256"] = compiled.digest(directory / "inventory.json")
        compiled.verify(identity, ROOT)
        phase = "first execution"
        print("qualification first execution: " + mode, flush=True)
        with (directory / "execution.log").open("w") as output:
            child = subprocess.Popen(runner + ["--" + mode], cwd=ROOT, env=env, stdout=output,
                                     stderr=subprocess.STDOUT, start_new_session=True)
            started, sampled, reported = time.monotonic(), 0.0, 0.0
            while child.poll() is None:
                elapsed = time.monotonic() - started
                require(elapsed < 3600, "qualification exceeded its one-hour execution deadline")
                if elapsed - sampled >= 15 and (directory / "fixture.json").exists():
                    fixture.update(directory, "sample")
                    sampled = elapsed
                if elapsed - reported >= 60:
                    print(f"qualification running: {mode}; elapsed_seconds={int(elapsed)}", flush=True)
                    reported = elapsed
                time.sleep(1)
            receipt["runner_exit"] = child.returncode
        phase = "result validation"
        receipt["execution_log_sha256"] = compiled.digest(directory / "execution.log")
        records = parse((directory / "execution.log").read_text())
        receipt["scenarios"] = records
        compiled.verify(identity, ROOT)
        state = json.loads((directory / "fixture.json").read_text())
        receipt["fixture"] = state
        fixture.validate(state)
        require(child.returncode == 0, "first execution did not succeed")
        receipt["summary"] = validate(mode, records)
        receipt["passed"] = True
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        receipt["failure_phase"] = phase
        receipt["failure"] = str(error) if isinstance(error, ValueError) else type(error).__name__
    finally:
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        if child is not None and child.poll() is None:
            os.killpg(child.pid, signal.SIGINT)
            try:
                child.wait(timeout=30)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait()
            receipt["runner_exit"] = child.returncode
        if (directory / "fixture.json").exists():
            receipt["fixture"] = json.loads((directory / "fixture.json").read_text())
        if (directory / "execution.log").exists():
            receipt["execution_log_sha256"] = compiled.digest(directory / "execution.log")
            try:
                receipt["scenarios"] = parse((directory / "execution.log").read_text())
            except (ValueError, TypeError):
                receipt["truncated_scenario_output"] = True
        receipt.update(finished_ms=time.time_ns() // 1_000_000,
                       host={**receipt["host"], "load_after": os.getloadavg()})
        (directory / "qualification.json").write_text(json.dumps(receipt, indent=2) + "\n")
        print(json.dumps({"passed": receipt["passed"], "receipt": str(directory / "qualification.json")}), flush=True)
    return 0 if receipt["passed"] else 1


if __name__ == "__main__":
    if len(sys.argv) != 2 or sys.argv[1] not in {"--" + mode for mode in MODES}:
        sys.exit("expected one dedicated qualification mode")
    os.umask(0o077)
    parent = os.environ.get("CMA_QUALIFICATION_EVIDENCE_DIR", os.environ.get("RUNNER_TEMP"))
    directory = Path(tempfile.mkdtemp(prefix="cma-inventory-", dir=parent))
    print("qualification evidence: " + str(directory), flush=True)
    sys.exit(run(sys.argv[1][2:], directory, environment(directory)))
