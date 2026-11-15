#!/usr/bin/env python3
"""Real Playwright regressions: stopped/crashed workers cannot orphan Chrome."""
import importlib.util
from datetime import datetime, timezone
import json
import os
import signal
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
# #1858 measured 17.236s launch + 4.043s page creation and readiness at 25.296s.
# 60s allows over twice that complete startup, independently of fault execution.
STARTUP_SECONDS = 60
SUPERVISOR_SECONDS = STARTUP_SECONDS + 30 + 30 + 20


def supervise(directory, mode):
    smoke.run_browser(
        ["node", "--require", str(Path(__file__).with_name("web-image-phases.cjs").resolve()),
                 "node_modules/@playwright/test/cli.js", "test", "--config",
         "e2e/image-watchdog.config.ts"], cwd=ROOT / "web",
        env={**os.environ, "CMA_WEB_IMAGE_EVIDENCE_DIR": str(directory / "evidence"),
             "CMA_WEB_WATCHDOG_READY": str(directory / "ready.json"),
             "CMA_WEB_WATCHDOG_STARTUP": str(directory / "fixture-startup.jsonl"),
             "CMA_WEB_WATCHDOG_MODE": mode},
        evidence=directory / "evidence", timeout=30,
        ready_file=directory / "ready.json", startup_timeout=STARTUP_SECONDS)


def regression(mode, *, evidence_root=None):
    root = Path(evidence_root or os.environ.get("CMA_WEB_WATCHDOG_EVIDENCE_DIR")
                or tempfile.mkdtemp(prefix="web-watchdog-evidence-"))
    directory = root.resolve() / mode
    directory.mkdir(parents=True, exist_ok=False)
    started = time.monotonic()
    driver = control = owned = None
    fallback = False

    def record(event, **fields):
        with (directory / "startup.jsonl").open("a") as stream:
            stream.write(json.dumps({"event": event, "mode": mode,
                "at": datetime.now(timezone.utc).isoformat(),
                "elapsed_seconds": round(time.monotonic() - started, 3), **fields}) + "\n")

    record("regression-start", readiness_seconds=STARTUP_SECONDS, execution_seconds=30, grace_seconds=30)
    print(f"Watchdog regression evidence: {directory}", flush=True)
    console = (directory / "runner.log").open("w")
    try:
        # This sibling is deliberately outside the supervised tree, even with a
        # separate session of its own. Ownership-based cleanup must leave it alone.
        control = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(200)"],
                                   start_new_session=True)
        driver = subprocess.Popen([sys.executable, __file__, "--supervise", str(directory), mode],
                                  stdout=console, stderr=subprocess.STDOUT)
        supervisor_deadline = time.monotonic() + SUPERVISOR_SECONDS
        record("driver-started", driver_pid=driver.pid, control_pid=control.pid)
        owned = OwnedProcesses(driver.pid)
        # The supervisor owns startup, SIGINT grace and cleanup. The observer only
        # has a final emergency bound; it cannot reject a valid late handshake.
        while not (directory / "ready.armed.json").exists():
            owned.capture()
            if driver.poll() is not None or time.monotonic() >= supervisor_deadline:
                record("readiness-failed", driver_returncode=driver.returncode,
                       deadline_expired=time.monotonic() >= supervisor_deadline)
                raise AssertionError("Browser fixture did not reach its real-page assertion")
            time.sleep(.05)
        record("execution-armed", **json.loads((directory / "ready.armed.json").read_text()))
        worker = json.loads((directory / "ready.json").read_text())["worker"]
        before = owned.capture()
        assert worker in before, "worker missing from owned tree"
        detached = [p for p in before.values()
                    if p.pid != driver.pid and p.pid == p.group and p.parent == worker]
        assert detached, "probe must exercise Playwright's detached browser group"
        browsers = {p.pid for p in detached}
        os.kill(worker, signal.SIGSTOP if mode == "stopped" else signal.SIGKILL)
        record("fault-injected", worker=worker)
        assert driver.wait(timeout=max(.001, supervisor_deadline - time.monotonic())) != 0, \
            "worker failure must fail supervision"
        evidence = directory / "evidence"
        result = json.loads((evidence / "browser-process.json").read_text())
        if mode == "stopped":
            assert result["timed_out"] and result["forced_kill"]
            assert result["returncode"] < 0
            assert result["timeout_phase"] == "execution"
            assert 60 <= result["execution_elapsed_seconds"] < 80, result
        else:
            assert not result["timed_out"] and not result["forced_kill"]
            assert result["returncode"] != 0
            assert result["execution_elapsed_seconds"] < 30, result
        assert result["startup_timeout_seconds"] == STARTUP_SECONDS
        assert 0 < result["startup_seconds"] < STARTUP_SECONDS, result
        if mode == "slow-start":
            assert result["startup_seconds"] >= 27, result
        assert result["interrupt_grace_seconds"] == 30
        assert result["cleanup_complete"], result
        assert not result["process_cleanup"]["remaining"], result
        recorded = {p["pid"] for p in result["process_cleanup"]["owned"]}
        supervisors = {p.pid for p in before.values() if p.parent == driver.pid}
        runners = {p.pid for p in before.values()
                   if p.parent in supervisors and p.pid == p.group}
        assert runners, "runner must be observed in the isolated supervisor"
        expected = set(runners)
        while True:
            children = {p.pid for p in before.values() if p.parent in expected}
            if children <= expected:
                break
            expected.update(children)
        assert browsers <= expected
        assert expected <= recorded, {"missing": sorted(expected - recorded), "result": result}
        current = snapshot()
        assert all(pid not in current or current[pid].identity != p.identity
                   for pid, p in before.items()), "a browser descendant survived"
        assert all(p["pid"] not in current or current[p["pid"]].started != p["started"]
                   for p in result["process_cleanup"]["owned"]), "recorded identity survived"
        events = [json.loads(line) for line in
                  (evidence / "browser-events.jsonl").read_text().splitlines()]
        assert any(e["event"] == "test-begin" for e in events)
        assert any(e["event"] == "step-end" and e["category"] == "expect" for e in events)
        assert control.poll() is None, "unrelated sibling was signalled"
        print(json.dumps({"real_browser_watchdog": "passed", "mode": mode, "browser_groups": len(browsers),
                          "owned_processes": len(recorded),
                          "elapsed_seconds": result["elapsed_seconds"],
                          "startup_seconds": result["startup_seconds"],
                          "execution_seconds": result["execution_elapsed_seconds"],
                          "grace_seconds": result["interrupt_grace_seconds"],
                          "diagnostic_events": len(events), "remaining_processes": 0,
                          "unrelated_control_alive": True}), flush=True)
    except BaseException as error:
        record("regression-failed", error_type=type(error).__name__)
        raise
    finally:
        try:
            if driver is not None:
                # Let the bounded supervisor retain evidence and clean up before
                # the emergency fallback removes its tree, including during startup.
                if driver.poll() is None:
                    record("await-supervisor")
                while driver.poll() is None and time.monotonic() < supervisor_deadline:
                    if owned is not None:
                        owned.capture()
                    try:
                        driver.wait(timeout=min(.1, max(.001, supervisor_deadline - time.monotonic())))
                    except subprocess.TimeoutExpired:
                        continue
                if driver.poll() is None:
                    fallback = True
                    record("fallback-kill")
                if owned is not None:
                    owned.terminate()
                driver.kill()
                driver.wait(timeout=5)
                record("driver-exited", returncode=driver.returncode, fallback_kill=fallback)
        finally:
            try:
                remaining = sorted(owned.capture()) if owned is not None else []
                (directory / "observer-process.json").write_text(json.dumps({
                    "driver_returncode": driver.returncode if driver is not None else None,
                    "fallback_kill": fallback, "remaining": remaining,
                    "observed": owned.evidence() if owned is not None else None,
                    "control_alive_before_cleanup": control.poll() is None if control is not None else None,
                }, indent=2) + "\n")
            finally:
                if control is not None:
                    control.kill()
                    control.wait(timeout=5)
                console.close()
                record("observer-finished")


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--supervise":
        supervise(Path(sys.argv[2]), sys.argv[3])
    elif len(sys.argv) == 1:
        for mode in ("crashed", "stopped"):
            regression(mode)
    else:
        raise SystemExit("usage: web-image-watchdog-tests.py")
