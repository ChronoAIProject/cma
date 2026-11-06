#!/usr/bin/env python3
"""Best-effort Linux CI samples. Never read process arguments or environment."""

import json
import os
from pathlib import Path
import sys
import time
from datetime import datetime, timezone

INTERVAL_SECONDS = 10
PARENT_CHECK_SECONDS = 0.2
MEMORY_FIELDS = {
    "MemTotal", "MemAvailable", "MemFree", "Buffers", "Cached",
    "SwapTotal", "SwapFree", "SwapCached",
}


def memory(proc):
    return {
        key: int(value.split()[0])
        for line in (proc / "meminfo").read_text().splitlines()
        for key, value in [line.split(":", 1)]
        if key in MEMORY_FIELDS
    }


def swap(proc):
    return {
        key: int(value)
        for line in (proc / "vmstat").read_text().splitlines()
        for key, value in [line.split()]
        if key in {"pswpin", "pswpout"}
    }


def top_processes(proc):
    rows = []
    page_kib = os.sysconf("SC_PAGE_SIZE") // 1024
    for entry in proc.iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            # comm is the kernel process name, not cmdline or ps args.
            name = (entry / "comm").read_text().strip()
            rss_kib = int((entry / "statm").read_text().split()[1]) * page_kib
            rows.append({"pid": int(entry.name), "name": name, "rss_kib": rss_kib})
        except (OSError, ValueError, IndexError):
            continue  # A process may exit between the two reads.
    return sorted(rows, key=lambda row: (-row["rss_kib"], row["pid"]))[:10]


def disk(path):
    stat = os.statvfs(path)
    return {"total_bytes": stat.f_blocks * stat.f_frsize,
            "available_bytes": stat.f_bavail * stat.f_frsize,
            "inodes_available": stat.f_favail}


def cgroup_memory(root):
    # Root cgroup v2 counters are labelled explicitly; not per-process RSS.
    values = {name: (root / name).read_text().strip()
              for name in ("memory.current", "memory.max", "memory.peak")}
    values["memory.events"] = {
        key: int(value)
        for line in (root / "memory.events").read_text().splitlines()
        for key, value in [line.split()]
    }
    return values


def sample(proc=Path("/proc"), cgroup=Path("/sys/fs/cgroup")):
    result = {"utc": datetime.now(timezone.utc).isoformat(timespec="milliseconds")}
    probes = {
        "memory_kib": lambda: memory(proc),
        "swap_pages": lambda: swap(proc),
        "load": lambda: [float(n) for n in (proc / "loadavg").read_text().split()[:3]],
        "disk_workspace": lambda: disk(os.environ.get("GITHUB_WORKSPACE", ".")),
        "disk_temp": lambda: disk(os.environ.get("RUNNER_TEMP", "/tmp")),
        "top_rss": lambda: top_processes(proc),
        "cgroup_root": lambda: cgroup_memory(cgroup),
    }
    for name, probe in probes.items():
        try:
            result[name] = probe()
        except Exception:
            # Exception messages can contain paths or other unwanted details.
            result[name] = "unavailable"
    return result


def run(parent_pid):
    started = time.monotonic()
    next_sample = started
    while os.getppid() == parent_pid:
        now = time.monotonic()
        if now >= next_sample:
            record = sample()
            record["step_elapsed_seconds"] = round(now - started, 3)
            print("[backend-resource] " + json.dumps(record, sort_keys=True), flush=True)
            next_sample = now + INTERVAL_SECONDS
        # Exit promptly on reparenting; never hold a finished step's log pipe
        # open for the ten-second sampling interval. No PID-reuse race.
        time.sleep(PARENT_CHECK_SECONDS)


if __name__ == "__main__":
    try:
        run(int(sys.argv[1]))
    except Exception:
        pass  # Telemetry is optional, including broken output / missing probes.
