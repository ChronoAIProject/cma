"""Run the existing Gateway budgets once and retain that invocation's evidence."""

from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import time


ROOT = Path(__file__).resolve().parents[2]
SELECTION = "package(=cmaeg) and test(~latency_budget)"
REPORTS = ("cmaeg-latency.txt", "cmaeg-latency-console.txt")


def invoke(command, cwd):
    child = None
    interrupted = 0
    deadline = None

    def forward(number, _frame):
        nonlocal interrupted, deadline
        if not interrupted:
            interrupted = number
            deadline = time.monotonic() + 5
        if child is not None:
            try:
                os.killpg(child.pid, number)
            except ProcessLookupError:
                pass

    previous = {number: signal.signal(number, forward)
                for number in (signal.SIGINT, signal.SIGTERM)}
    try:
        child = subprocess.Popen(command, cwd=cwd, start_new_session=True,
                                 env=dict(os.environ, CARGO_INCREMENTAL="0"))
        if interrupted:
            forward(interrupted, None)
        while True:
            try:
                return child.wait(timeout=0.1), interrupted
            except subprocess.TimeoutExpired:
                if deadline is not None and time.monotonic() >= deadline:
                    try:
                        os.killpg(child.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
    finally:
        if interrupted and child is not None:
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        for number, handler in previous.items():
            signal.signal(number, handler)


def main():
    api = ROOT / "api"
    target = api / "target"
    output = target / "cmaeg-latency-results/attempt-1"
    output.mkdir(parents=True, exist_ok=False)
    checkout = subprocess.check_output(
        ["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True).strip()
    for name in REPORTS:
        (target / name).unlink(missing_ok=True)
    started = datetime.now(timezone.utc).isoformat()
    args = ["run", "--run-ignored", "only", "-E", SELECTION, "--success-output", "final"]
    returncode, interrupted = invoke(
        ["bash", str(ROOT / ".github/scripts/backend-nextest.sh"), *args], api)
    test_exit = returncode if returncode >= 0 else 128 - returncode
    gate_exit = 128 + interrupted if interrupted else test_exit
    reports = {}
    for name in REPORTS:
        source = target / name
        if source.is_file():
            shutil.copyfile(source, output / name)
            reports[name] = {"status": "present",
                             "sha256": hashlib.sha256((output / name).read_bytes()).hexdigest()}
        else:
            reports[name] = {"status": "missing"}
            if gate_exit == 0:
                gate_exit = 1
    result = {
        "checkout": checkout,
        "workflow_run_id": os.environ.get("GITHUB_RUN_ID"),
        "workflow_attempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
        "invocation": 1,
        "selection": SELECTION,
        "nextest_args": args,
        "started_at": started,
        "finished_at": datetime.now(timezone.utc).isoformat(),
        "test_returncode": returncode,
        "test_exit_code": test_exit,
        "gate_exit_code": gate_exit,
        "signal": interrupted or (-returncode if returncode < 0 else None),
        "reports": reports,
    }
    (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(f"Gateway latency invocation 1: test exit {test_exit}, gate exit {gate_exit}", flush=True)
    return gate_exit


if __name__ == "__main__":
    raise SystemExit(main())
