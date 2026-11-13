#!/usr/bin/env python3
"""Keep readiness-failure diagnostics through supervisor cleanup and staging."""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
from unittest.mock import patch
import zipfile


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


watchdog = load("watchdog", "web-image-watchdog-tests.py")
evidence = load("evidence", "web-image-evidence.py")


def events(path):
    return [json.loads(line) for line in path.read_text().splitlines()]


def regression():
    root = Path(os.environ.get("CMA_WEB_WATCHDOG_EVIDENCE_TEST_DIR")
                or tempfile.mkdtemp(prefix="web-readiness-evidence-test-")).resolve()
    root.mkdir(parents=True, exist_ok=True)
    # Even failure to launch the first child must retain observer/startup evidence.
    with patch.object(watchdog.subprocess, "Popen", side_effect=FileNotFoundError):
        try:
            watchdog.regression("launch-error", evidence_root=root)
        except FileNotFoundError:
            pass
        else:
            raise AssertionError("launch failure was hidden")
    launch = root / "launch-error"
    assert json.loads((launch / "observer-process.json").read_text())["driver_returncode"] is None
    assert events(launch / "startup.jsonl")[-1]["event"] == "observer-finished"

    # The real browser exists, but the ready marker is deliberately never written.
    # The observer's 25s assertion must fail while the 30s + 30s supervisor remains
    # responsible for SIGINT, trace retention, browser shutdown and process reaping.
    try:
        watchdog.regression("pre-ready-stall", evidence_root=root)
    except AssertionError as error:
        assert str(error) == "Browser fixture did not reach its real-page assertion"
    else:
        raise AssertionError("pre-ready stall unexpectedly passed")
    directory = root / "pre-ready-stall"
    startup = events(directory / "startup.jsonl")
    failed = next(e for e in startup if e["event"] == "readiness-failed")
    assert failed["deadline_expired"] and failed["driver_returncode"] is None
    assert 25 <= failed["elapsed_seconds"] < 30, failed
    assert [e["event"] for e in startup][-3:] == ["await-supervisor", "driver-exited", "observer-finished"]
    observer = json.loads((directory / "observer-process.json").read_text())
    assert not observer["fallback_kill"] and not observer["remaining"], observer
    assert observer["control_alive_before_cleanup"], observer
    result = json.loads((directory / "evidence/browser-process.json").read_text())
    assert result["timeout_seconds"] == 30 and result["interrupt_grace_seconds"] == 30
    assert result["timed_out"] and not result["forced_kill"] and result["cleanup_complete"], result
    assert 30 <= result["elapsed_seconds"] < 80 and not result["process_cleanup"]["remaining"]
    assert any(e["event"] == "injected-pre-ready-stall" for e in events(directory / "fixture-startup.jsonl"))
    reporter = events(directory / "evidence/browser-events.jsonl")
    assert any(e["event"] == "test-begin" for e in reporter)
    assert any(e["event"] == "suite-end" and e["status"] != "passed" for e in reporter)
    phases = [e for p in (directory / "evidence").glob("browser-phases-*.jsonl") for e in events(p)]
    required = {"worker-start", "browser-launch", "context-create", "page-create",
                "worker-teardown", "browser-close", "reporter-onEnd", "trace-finalization", "zip-finalization"}
    assert required <= {e["phase"] for e in phases if e["event"] == "phase-end"}
    traces = list(directory.rglob("trace.zip"))
    assert traces, "graceful readiness failure must retain the real-browser trace"
    for trace in traces:
        with zipfile.ZipFile(trace) as archive:
            assert archive.testzip() is None
    assert (directory / "runner.log").stat().st_size > 0
    destination = root.with_name(root.name + "-staged")
    assert evidence.stage(root, destination)
    for source in directory.rglob("*"):
        if source.is_file() and not any(p.startswith(".") for p in source.relative_to(root).parts):
            assert (destination / source.relative_to(root)).read_bytes() == source.read_bytes()
    print(json.dumps({"pre_ready_evidence": "passed", "readiness_failure_seconds": failed["elapsed_seconds"],
                      "supervisor_seconds": result["elapsed_seconds"], "fallback_kill": False,
                      "valid_traces": len(traces), "retained_directory": str(directory)}), flush=True)


if __name__ == "__main__":
    regression()
