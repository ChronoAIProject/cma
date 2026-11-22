"""Observe only the Mongo fixture owned by the existing disposable runner."""
import fcntl
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import re

from inventory_qualification_results import require

IMAGE = "mongo:8.0.13@sha256:cf340b1e5283843c63eb12999922f20c463ae31285f746d30f05dcc21cd1d47c"


def check_sample(sample, first):
    require(all(sample[k] == first[k] for k in ["id", "image", "started_at", "memory_bytes", "memory_swap_bytes",
        "nano_cpus", "pids_limit", "wired_tiger_cache_gib", "image_reference"]), "fixture identity or resources changed")
    require(re.fullmatch(r"[0-9a-f]{64}", sample["id"]) and
            re.fullmatch(r"sha256:[0-9a-f]{64}", sample["image"]) and sample["started_at"], "missing fixture identity")
    require(sample["running"] is True and sample["oom_killed"] is False and sample["restarts"] == 0,
            "fixture stopped or restarted")
    require(sample["memory_bytes"] == 2147483648 and sample["nano_cpus"] == 2000000000 and
            sample["pids_limit"] == 256 and sample["memory_swap_bytes"] == 2147483648 and
            sample["wired_tiger_cache_gib"] == "0.25" and sample["image_reference"] == IMAGE,
            "fixture resource limits or image differ")


def validate(record):
    require(record.get("phase") == "closed" and record.get("owned_cleanup") is True and not record.get("errors"),
            "fixture observation or owned cleanup incomplete")
    require(re.fullmatch(r"cma-mongo-ci-[0-9a-f]{24}", record.get("name", "")), "fixture owner identity missing")
    require(record.get("first_test_exit") == 0 and record.get("runner_exit") == 0 and len(record.get("samples", [])) >= 2,
            "first execution or fixture observations incomplete")
    for sample in record["samples"]:
        check_sample(sample, record["samples"][0])


def inspect(name):
    rows = json.loads(subprocess.check_output(["docker", "inspect", name], text=True, timeout=15))
    require(len(rows) == 1, "fixture identity missing")
    row = rows[0]
    require(row["Config"]["Labels"].get("dev.chrono.cma.mongo-contract") == name, "fixture ownership differs")
    arguments = row["Config"]["Cmd"]
    cache = arguments[arguments.index("--wiredTigerCacheSizeGB") + 1]
    return {"id": row["Id"], "image": row["Image"], "started_at": row["State"]["StartedAt"],
        "running": row["State"]["Running"], "oom_killed": row["State"]["OOMKilled"], "restarts": row["RestartCount"],
        "memory_bytes": row["HostConfig"]["Memory"], "nano_cpus": row["HostConfig"]["NanoCpus"],
        "memory_swap_bytes": row["HostConfig"]["MemorySwap"], "wired_tiger_cache_gib": cache,
        "image_reference": row["Config"]["Image"],
        "pids_limit": row["HostConfig"]["PidsLimit"], "observed_ms": time.time_ns() // 1_000_000}


def update(directory, phase, name=None, exit_code=None, test_exit=None):
    path = directory / "fixture.json"
    with (directory / "fixture.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        record = json.loads(path.read_text()) if path.exists() else {"samples": [], "errors": []}
        if phase == "start":
            require(not path.exists() and name is not None, "fixture already recorded")
            record.update(name=name, phase="running", wired_tiger_cache_gib="0.25",
                          image_pin=IMAGE)
        if phase in {"start", "sample", "closing"}:
            if phase == "sample" and record.get("phase") != "running":
                return
            try:
                sample = inspect(record["name"])
                record["samples"].append(sample)
                first = record["samples"][0]
                check_sample(sample, first)
            except (KeyError, ValueError, OSError, subprocess.SubprocessError):
                record["errors"].append("fixture observation failed")
            if phase == "closing":
                record["phase"] = "closing"
        elif phase == "closed":
            record.update(phase="closed", runner_exit=int(exit_code), first_test_exit=int(test_exit),
                          owned_cleanup=int(exit_code) != 125)
        else:
            require(phase in {"start", "sample", "closing"}, "invalid fixture phase")
        temporary = path.with_suffix(".tmp")
        temporary.write_text(json.dumps(record, sort_keys=True) + "\n")
        os.replace(temporary, path)
        require(not record["errors"], "fixture observation failed")


if __name__ == "__main__":
    try:
        update(Path(os.environ["CMA_INVENTORY_QUALIFICATION_DIR"]), sys.argv[1],
               name=sys.argv[2] if len(sys.argv) > 2 else None,
               exit_code=sys.argv[2] if len(sys.argv) > 2 else None,
               test_exit=sys.argv[3] if len(sys.argv) > 3 else None)
    except (KeyError, ValueError, OSError, subprocess.SubprocessError):
        print("qualification fixture evidence failed", file=sys.stderr)
        sys.exit(125)
