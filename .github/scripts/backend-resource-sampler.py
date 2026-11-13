#!/usr/bin/env python3
"""Best-effort Linux CI samples. Never read process arguments or environment."""

import ctypes
import json
import os
from pathlib import Path
import sys
import signal
import stat
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
    # The kernel kills us even while a probe or output is blocked. Refuse to
    # sample if this Linux guarantee cannot be installed. Check after prctl
    # to close the race where the shell died before registration.
    if not arm_parent_death(parent_pid):
        return
    descriptor = output_pipe()
    if descriptor is None:
        return
    try:
        collect(parent_pid, descriptor)
    finally:
        os.close(descriptor)


def arm_parent_death(parent_pid):
    if sys.platform != "linux":
        return False
    libc = ctypes.CDLL(None, use_errno=True)
    # PR_SET_PDEATHSIG = 1. SIGKILL cannot be deferred by Python handlers, a
    # blocked probe, the GIL, or a full log pipe.
    if libc.prctl(1, signal.SIGKILL, 0, 0, 0) != 0:
        return False
    return os.getppid() == parent_pid


def output_pipe():
    if not stat.S_ISFIFO(os.fstat(1).st_mode):
        return None
    # Opening /proc/self/fd creates a NEW open file description on Linux.
    # dup() / set_blocking(1, False) would change the gate's shared pipe flags.
    descriptor = os.open("/proc/self/fd/1", os.O_WRONLY | os.O_NONBLOCK | os.O_CLOEXEC)
    os.close(1)  # Only the independent, discardable output descriptor remains.
    return descriptor


def emit_record(descriptor, record):
    data = ("[backend-resource] " + json.dumps(record, sort_keys=True) + "\n").encode()
    # One <= PIPE_BUF write is atomic: backpressure drops the whole record,
    # never a truncated JSON fragment. No polling, queue or flush can block.
    if len(data) > os.fpathconf(descriptor, "PC_PIPE_BUF"):
        return
    try:
        os.write(descriptor, data)
    except OSError:
        pass


def collect(parent_pid, descriptor):
    started = time.monotonic()
    next_sample = started
    while os.getppid() == parent_pid:
        now = time.monotonic()
        if now >= next_sample:
            record = sample()
            record["step_elapsed_seconds"] = round(now - started, 3)
            emit_record(descriptor, record)
            next_sample = now + INTERVAL_SECONDS
        # The kernel parent-death signal is independent of this cadence loop.
        time.sleep(PARENT_CHECK_SECONDS)


if __name__ == "__main__":
    try:
        run(int(sys.argv[1]))
    except Exception:
        pass  # Telemetry is optional, including broken output / missing probes.
