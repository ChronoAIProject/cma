#!/usr/bin/env python3
"""Require phase coverage and intact failure traces from real browser runs."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import zipfile

spec = importlib.util.spec_from_file_location(
    "smoke", Path(__file__).with_name("web-image-smoke.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)
ROOT = Path(__file__).resolve().parents[2]


def regression(mode):
    with tempfile.TemporaryDirectory(prefix="web-phase-regression-") as temporary:
        directory = Path(temporary)
        evidence = directory / "evidence"
        failed = False
        try:
            smoke.run_browser(
                ["node", "--require", str(Path(__file__).with_name("web-image-phases.cjs").resolve()),
                 "node_modules/@playwright/test/cli.js", "test", "--config", "e2e/image-watchdog.config.ts"],
                cwd=ROOT / "web", env={**os.environ,
                    "CMA_WEB_IMAGE_EVIDENCE_DIR": str(evidence),
                    "CMA_WEB_WATCHDOG_READY": str(directory / "ready.json"),
                    "CMA_WEB_WATCHDOG_MODE": mode}, evidence=evidence, timeout=30, grace=5)
        except subprocess.CalledProcessError:
            failed = True
        assert failed == (mode == "phase-fail")
        result = json.loads((evidence / "browser-process.json").read_text())
        assert not result["timed_out"] and result["cleanup_complete"], result
        events = [json.loads(line) for path in evidence.glob("browser-phases-*.jsonl")
                  for line in path.read_text().splitlines()]
        begins = {(e["pid"], e["id"]): e for e in events if e["event"] == "phase-begin"}
        ends = {(e["pid"], e["id"]): e for e in events if e["event"] == "phase-end"}
        assert begins.keys() == ends.keys(), "phase evidence must cover completion"
        for key, end in ends.items():
            assert begins[key]["at"] <= end["at"] and end["durationMs"] >= 0
            assert end["outcome"] == "completed", end
        required = {"reporter-onEnd", "trace-finalization", "worker-teardown",
                    "worker-host-stop", "browser-close", "worker-start", "browser-launch",
                    "context-create", "page-create"}
        traces = list(evidence.rglob("trace.zip"))
        if failed:
            required.add("zip-finalization")
            assert traces and list(evidence.rglob("*-failed-*.png"))
            for trace in traces:
                with zipfile.ZipFile(trace) as archive:
                    assert archive.testzip() is None and "test.trace" in archive.namelist()
        else:
            assert not traces, "passing runs must not retain failure traces"
        assert required <= {e["phase"] for e in events}, events
        print(json.dumps({"real_browser_phases": "passed", "mode": mode,
                          "phase_calls": len(begins), "phases": sorted(required),
                          "elapsed_seconds": result["elapsed_seconds"],
                          "valid_retained_traces": len(traces)}), flush=True)


if __name__ == "__main__":
    for mode in ("phase-pass", "phase-fail"):
        regression(mode)
