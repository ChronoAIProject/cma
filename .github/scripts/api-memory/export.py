#!/usr/bin/env python3
"""The single scheduled offline exporter build, capped at 900 seconds and 6 GiB RSS."""
import datetime
import json
import os
from pathlib import Path
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parents[3]
API = ROOT / "api"
EVIDENCE = API / "target/api-memory-rca"
PRIVATE = EVIDENCE / "private"
COMMAND = ["/Users/shining/code/work/.tl-env/build-slot", "env", "RUSTC_WRAPPER=",
           "CARGO_BUILD_JOBS=2", "/Users/shining/.cargo/bin/cargo", "+1.96.1", "test",
           "--locked", "--offline", "--all-features", "-j", "2", "-p", "cma", "--bin", "cma",
           "store::sandbox_mongo_integration_tests::api_memory_fixture::export_finite_api_memory_dataset",
           "--", "--ignored", "--exact", "--nocapture"]


def utc():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def run():
    PRIVATE.mkdir(mode=0o700, parents=True, exist_ok=True)
    fixture = PRIVATE / "fixture.json"
    if fixture.exists():
        raise ValueError("retained fixture already exists")
    environment = os.environ.copy()
    environment.update(RUST_MIN_STACK="33554432", CARGO_INCREMENTAL="0",
                       CARGO_TARGET_DIR=str(API / "target/api-memory-export"),
                       CMA_MEMORY_FIXTURE_OUTPUT=str(fixture))
    for name in ("RUSTFLAGS", "RUSTDOCFLAGS", "CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_TEST_DEBUG",
                 "TL_BUILD_PRIORITY", "CHRONO_TEST_MONGODB_URI", "CHRONO_TEST_MONGODB_FAILPOINT_URI"):
        environment.pop(name, None)
    started = time.monotonic()
    report = {"started_utc": utc(), "command": COMMAND, "max_seconds": 900,
              "max_process_group_rss_kib": 6 * 1024 * 1024, "peak_group_rss_kib": 0,
              "outcome": "incomplete"}
    with open(PRIVATE / "export.log", "x", opener=lambda name, flags: os.open(name, flags, 0o600)) as log:
        process = subprocess.Popen(COMMAND, cwd=API, env=environment, stdout=log, stderr=subprocess.STDOUT,
                                   start_new_session=True)
        report["pid"] = process.pid
        try:
            while process.poll() is None:
                rows = subprocess.check_output(["ps", "-axo", "pid=,pgid=,rss="], text=True)
                rss = sum(int(row.split()[2]) for row in rows.splitlines()
                          if len(row.split()) == 3 and int(row.split()[1]) == process.pid)
                report["peak_group_rss_kib"] = max(report["peak_group_rss_kib"], rss)
                if rss > report["max_process_group_rss_kib"] or time.monotonic() - started > 900:
                    report["stop_reason"] = "RSS limit" if rss > report["max_process_group_rss_kib"] else "time limit"
                    os.killpg(process.pid, signal.SIGTERM)
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                    break
                time.sleep(2)
            report["exit_code"] = process.wait()
            if report["exit_code"] == 0 and fixture.is_file():
                report["outcome"] = "exported"
                report["fixture_bytes"] = fixture.stat().st_size
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            report["ended_utc"] = utc()
            report["elapsed_seconds"] = time.monotonic() - started
            (EVIDENCE / "export-result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"outcome": report["outcome"], "exit_code": report["exit_code"]}))
    return report["outcome"] == "exported"


if __name__ == "__main__":
    raise SystemExit(0 if run() else 1)
