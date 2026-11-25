#!/usr/bin/env python3
"""Sample runner headroom for the isolated Omarchy timing experiment."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

PERIOD = 2
MAX_GAP = 12
SETTLE = 15


def wait_for(path, worker=None):
    deadline = time.monotonic() + SETTLE
    while not path.is_file():
        if worker is not None and worker.poll() is not None:
            raise RuntimeError(f"Sampler exited before producing {path.name}")
        if time.monotonic() >= deadline:
            raise RuntimeError(f"Timed out waiting for {path.name}")
        time.sleep(0.1)


def read_sample():
    memory = dict(
        (name.rstrip(":"), int(value))
        for name, value, *_ in map(str.split, Path("/proc/meminfo").read_text().splitlines())
    )
    root_free = shutil.disk_usage("/").free // 1024
    artifact_free = shutil.disk_usage(os.environ["RUNNER_TEMP"]).free // 1024
    return time.monotonic(), memory["MemTotal"] - memory["MemAvailable"], root_free, artifact_free


def sample(directory):
    with (directory / "headroom.tsv").open("x", buffering=1) as samples:
        while True:
            stopping = (directory / "sampler-stop").exists()
            print(*read_sample(), sep="\t", file=samples)
            if stopping:
                break
            (directory / "sampler-ready").touch()
            time.sleep(PERIOD)
    (directory / "sampler-finished").touch()


def start(directory):
    with (directory / "sampler.log").open("x") as log:
        worker = subprocess.Popen(
            [sys.executable, __file__, "sample", str(directory)],
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
    try:
        wait_for(directory / "sampler-ready", worker)
        (directory / "comparison-start").write_text(str(time.monotonic()))
    except BaseException:
        worker.terminate()
        worker.wait(timeout=SETTLE)
        raise


def summarize(started, ended, samples):
    if ended <= started or len(samples) < 2:
        raise ValueError("Interval and at least two samples are required")
    ticks = [sample[0] for sample in samples]
    if not (started - MAX_GAP <= ticks[0] <= started < ended <= ticks[-1] <= ended + MAX_GAP):
        raise ValueError("Samples do not cover the start and end of the interval")
    if any(not 0 < right - left <= MAX_GAP for left, right in zip(ticks, ticks[1:])):
        raise ValueError("Samples have an invalid timestamp or an internal gap")
    if any(min(sample[1:]) <= 0 for sample in samples):
        raise ValueError("A runner resource sample is invalid or exhausted")
    return (
        round(ended - started),
        max(sample[1] for sample in samples),
        min(sample[2] for sample in samples),
        min(sample[3] for sample in samples),
    )


def finish(directory):
    ended = time.monotonic()
    (directory / "sampler-stop").touch()
    wait_for(directory / "sampler-finished")
    started = float((directory / "comparison-start").read_text())
    samples = []
    for row in (directory / "headroom.tsv").read_text().splitlines():
        tick, memory, root, artifact = row.split("\t")
        samples.append((float(tick), int(memory), int(root), int(artifact)))
    metrics = summarize(started, ended, samples)
    (directory / "comparison-totals.txt").write_text(" ".join(map(str, metrics)) + "\n")


def cache(directory):
    caches = {}
    for name in ("OMARCHY", "TRIVY"):
        requested, matched, hit, outcome = (
            os.environ[f"{name}_{field}"] for field in ("REQUESTED", "MATCHED", "HIT", "OUTCOME")
        )
        if not requested or hit not in ("true", "false", "") or outcome != "success":
            raise ValueError(f"Cache restoration evidence is incomplete for {name}")
        if (hit == "true" and matched != requested) or (name == "OMARCHY" and hit != "true" and matched):
            raise ValueError(f"Cache restoration evidence disagrees for {name}")
        caches[name.lower()] = {"requested": requested, "matched": matched, "hit": hit, "outcome": outcome}
    (directory / "comparison-caches.json").write_text(json.dumps(caches, indent=2) + "\n")


if __name__ == "__main__":
    actions = {"sample": sample, "start": start, "finish": finish, "cache": cache}
    actions[sys.argv[1]](Path(sys.argv[2]).resolve())
