#!/usr/bin/env python3
"""Real Playwright regression: a stopped worker cannot orphan detached Chrome."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from web_image_processes import OwnedProcesses, snapshot

spec = importlib.util.spec_from_file_location(
    "web_image_smoke", Path(__file__).with_name("web-image-smoke.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)
ROOT = Path(__file__).resolve().parents[2]


def supervise(directory):
    smoke.run_browser(
        ["node", "node_modules/@playwright/test/cli.js", "test", "--config",
         "e2e/image-watchdog.config.ts"], cwd=ROOT / "web",
        env={**os.environ, "CMA_WEB_IMAGE_EVIDENCE_DIR": str(directory / "evidence"),
             "CMA_WEB_WATCHDOG_READY": str(directory / "ready.json")},
        # Leave cold CI browser startup room; the full production 30s grace is used.
        evidence=directory / "evidence", timeout=30)


def regression():
    with tempfile.TemporaryDirectory(prefix="web-watchdog-regression-") as temporary:
        directory = Path(temporary)
        # This sibling is deliberately outside the supervised tree, even with a
        # separate session of its own. Ownership-based cleanup must leave it alone.
        control = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(120)"],
                                   start_new_session=True)
        driver = subprocess.Popen([sys.executable, __file__, "--supervise", str(directory)])
        owned = OwnedProcesses(driver.pid)
        try:
            deadline = time.monotonic() + 25
            while not (directory / "ready.json").exists():
                if driver.poll() is not None or time.monotonic() >= deadline:
                    raise AssertionError("Browser fixture did not reach its real-page assertion")
                time.sleep(.05)
            worker = json.loads((directory / "ready.json").read_text())["worker"]
            before = owned.capture()
            assert worker in before, "stopped worker missing from owned tree"
            detached = [p for p in before.values()
                        if p.pid != driver.pid and p.pid == p.group and p.parent == worker]
            assert detached, "probe must exercise Playwright's detached browser group"
            browsers = {p.pid for p in detached}
            assert driver.wait(timeout=80) != 0, "forced timeout must fail"
            evidence = directory / "evidence"
            result = json.loads((evidence / "browser-process.json").read_text())
            assert result["timed_out"] and result["forced_kill"]
            assert result["returncode"] < 0
            assert result["interrupt_grace_seconds"] == 30
            assert 60 <= result["elapsed_seconds"] < 80, result
            assert not result["process_cleanup"]["remaining"], result
            recorded = {p["pid"] for p in result["process_cleanup"]["owned"]}
            assert set(before) - {driver.pid} <= recorded
            current = snapshot()
            assert all(pid not in current or current[pid].identity != p.identity
                       for pid, p in before.items()), "a browser descendant survived"
            events = [json.loads(line) for line in
                      (evidence / "browser-events.jsonl").read_text().splitlines()]
            assert any(e["event"] == "test-begin" for e in events)
            assert any(e["event"] == "step-end" and e["category"] == "expect" for e in events)
            assert control.poll() is None, "unrelated sibling was signalled"
            print(json.dumps({"real_browser_watchdog": "passed", "browser_groups": len(browsers),
                              "owned_processes": len(recorded),
                              "elapsed_seconds": result["elapsed_seconds"],
                              "grace_seconds": result["interrupt_grace_seconds"],
                              "diagnostic_events": len(events), "remaining_processes": 0,
                              "unrelated_control_alive": True}), flush=True)
        finally:
            # Test cleanup also runs when testing a broken supervisor.
            owned.terminate()
            driver.kill()
            driver.wait()
            control.kill()
            control.wait()


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--supervise":
        supervise(Path(sys.argv[2]))
    elif len(sys.argv) == 1:
        regression()
    else:
        raise SystemExit("usage: web-image-watchdog-tests.py")
