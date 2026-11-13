#!/usr/bin/env python3
"""Best-effort capacity preparation for the ephemeral Backend hosted runner."""

import json
import os
from pathlib import Path
import subprocess
import sys
import time


GIB = 1024**3
# Audited against Backend's tools; see docs/CI_Contracts.md. Never accept paths
# from workflow inputs, environment variables, or command-line arguments.
CLEANUP_PATHS = (
    Path("/usr/local/lib/android"),
    Path("/usr/share/dotnet"),
    Path("/opt/hostedtoolcache/CodeQL"),
)
SWAP_PATH = Path("/cma-backend.swap")


def emit(event, **fields):
    print("backend-capacity " + json.dumps({"event": event, **fields}), flush=True)


def available_bytes():
    disk = os.statvfs("/")
    return disk.f_bavail * disk.f_frsize


def memory_bytes():
    wanted = {"MemAvailable", "SwapTotal", "SwapFree"}
    return {
        key.rstrip(":"): int(value) * 1024
        for line in Path("/proc/meminfo").read_text().splitlines()
        for key, value, *_ in [line.split()]
        if key.rstrip(":") in wanted
    }


def hosted_runner():
    return (
        sys.platform == "linux"
        and os.geteuid() == 0
        and os.environ.get("GITHUB_ACTIONS") == "true"
        and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted"
        and os.environ.get("RUNNER_OS") == "Linux"
        and os.stat(".").st_dev == os.stat("/").st_dev
    )


def safe_target(path):
    # Reject symlinks (including ancestors) and mounted SDKs. GNU rm also
    # refuses to cross into a different filesystem below the target.
    return (
        path in CLEANUP_PATHS
        and path.resolve(strict=True) == path
        and path.is_dir()
        and not path.is_mount()
        and path.stat().st_dev == os.stat("/").st_dev
    )


def run(command, timeout=30):
    result = subprocess.run(command, capture_output=True, timeout=timeout, check=False)
    if result.returncode:
        # Do not dump arbitrary subprocess output or its environment.
        raise RuntimeError(f"{command[0]} exited {result.returncode}")


def reclaim(path):
    started = time.monotonic()
    before = available_bytes()
    status = "absent"
    try:
        if path.exists() or path.is_symlink():
            if not safe_target(path):
                status = "unsafe-skipped"
            else:
                run(["rm", "-rf", "--one-file-system", "--", str(path)], timeout=120)
                status = "removed"
    except Exception as error:
        status = type(error).__name__
    after = available_bytes()
    emit("cleanup", path=str(path), status=status, before_bytes=before,
         after_bytes=after, freed_bytes=after - before,
         duration_seconds=round(time.monotonic() - started, 3))


def swap_budget(initial, current):
    reclaimed = max(0, current - initial)
    # Preserve at least half the measured gain AND 4 GiB of net disk gain.
    # Never spend pre-existing free space. Round down to whole GiB.
    return max(0, min(8 * GIB, reclaimed // 2, reclaimed - 4 * GIB)) // GIB * GIB


def swap_active():
    return any(line.split()[0] == str(SWAP_PATH)
               for line in Path("/proc/swaps").read_text().splitlines()[1:])


def add_swap(size):
    if size < GIB:
        emit("swap", status="insufficient-reclamation", allocated_bytes=0)
        return
    # Exclusive creation avoids changing any existing swapfile or symlink.
    try:
        descriptor = os.open(SWAP_PATH, os.O_CREAT | os.O_EXCL | os.O_RDWR, 0o600)
    except FileExistsError:
        emit("swap", status="existing-path-skipped", allocated_bytes=0)
        return
    operation = "allocate"
    try:
        with os.fdopen(descriptor, "wb") as swap:
            os.posix_fallocate(swap.fileno(), 0, size)
            os.fsync(swap.fileno())
        operation = "mkswap"
        run(["mkswap", str(SWAP_PATH)])
        operation = "swapon"
        run(["swapon", str(SWAP_PATH)])
        emit("swap", status="activated", allocated_bytes=size)
    except Exception as error:
        # If activation timed out, it may have succeeded. Never unlink active
        # swap or turn off the runner's original swap. Unknown state is kept.
        cleanup = "kept-unknown"
        try:
            if swap_active():
                cleanup = "kept-active"
            else:
                SWAP_PATH.unlink()
                cleanup = "removed-inactive"
        except OSError:
            pass
        emit("swap", status=type(error).__name__, operation=operation, cleanup=cleanup)


def prepare():
    if not hosted_runner():
        emit("skip", reason="requires-root-on-ephemeral-Linux-hosted-runner")
        return
    initial = available_bytes()
    emit("before", available_bytes=initial, memory_bytes=memory_bytes())
    for path in CLEANUP_PATHS:
        reclaim(path)
    current = available_bytes()
    budget = swap_budget(initial, current)
    emit("budget", reclaimed_bytes=current - initial, swap_bytes=budget)
    add_swap(budget)
    final = available_bytes()
    emit("after", available_bytes=final, net_freed_bytes=final - initial,
         memory_bytes=memory_bytes())


def main():
    started = time.monotonic()
    try:
        prepare()
    except Exception as error:
        emit("warning", error=type(error).__name__)
    finally:
        emit("complete", duration_seconds=round(time.monotonic() - started, 3))
    # Capacity preparation is best effort. Subsequent gates own their status.
    return 0


if __name__ == "__main__":
    sys.exit(main())
