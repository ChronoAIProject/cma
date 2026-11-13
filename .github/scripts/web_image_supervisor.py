#!/usr/bin/env python3
"""Isolated watchdog: its only launched workload is the browser runner.

Linux adopts orphan descendants before launching anything, including processes
never seen in a snapshot. macOS samples descendants every 50ms (plus snapshot
cost); it is best effort if a parent dies between samples. Always clean up after
success, failure and timeout. No process-name or machine-wide group matching.
"""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
from web_image_processes import OwnedProcesses, child_subreaper


def supervise_browser(command, *, cwd, env, evidence, timeout, grace):
    started = time.monotonic()
    process = subprocess.Popen(command, cwd=cwd, env=env, start_new_session=True)
    owned = OwnedProcesses(process.pid, adopter=os.getpid() if sys.platform == "linux" else None)
    timed_out = False
    forced = False
    cleanup_complete = False

    def record():
        evidence.mkdir(parents=True, exist_ok=True)
        (evidence / "browser-process.json").write_text(json.dumps({
            "timeout_seconds": timeout, "interrupt_grace_seconds": grace,
            "elapsed_seconds": round(time.monotonic() - started, 3),
            "timed_out": timed_out, "forced_kill": forced,
            "returncode": process.returncode,
            "cleanup_complete": cleanup_complete,
            "process_cleanup": owned.evidence(),
        }, indent=2) + "\n")

    def wait_until(deadline):
        while True:
            # Capture before polling/reaping the runner, throughout normal execution.
            owned.capture()
            if process.poll() is not None:
                return True
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return False
            time.sleep(min(.05, remaining))

    try:
        try:
            if not wait_until(started + timeout):
                raise subprocess.TimeoutExpired(command, timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            # Include newly adopted children even if their original parent has exited.
            owned.capture()
            # Save evidence before signalling, even if the runner cannot respond.
            record()
            print(f"Web browser watchdog expired after {timeout}s; "
                  f"interrupting Playwright with {grace}s to retain evidence", flush=True)
            process.send_signal(signal.SIGINT)
            if not wait_until(time.monotonic() + grace):
                forced = True
                print("Playwright did not stop; terminating its owned descendants", flush=True)
            # A timeout remains a failure even if an interrupted runner exits zero.
            raise subprocess.TimeoutExpired(command, timeout)
        if process.returncode:
            raise subprocess.CalledProcessError(process.returncode, command)
    finally:
        try:
            owned.terminate()
        finally:
            try:
                process.kill()
                process.wait(timeout=5)
                owned.reap()
                cleanup_complete = True
            finally:
                record()


if __name__ == "__main__":
    evidence, timeout, grace, *command = sys.argv[1:]
    with child_subreaper():
        supervise_browser(command, cwd=Path.cwd(), env=os.environ.copy(),
                          evidence=Path(evidence), timeout=float(timeout), grace=float(grace))
