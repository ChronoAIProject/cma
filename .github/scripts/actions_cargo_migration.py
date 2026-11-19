"""Reconcile uncoordinated develop writers before charging new uploads."""

import subprocess
from datetime import datetime, timedelta, timezone

from actions_cache_retention import CARGO, DEVELOP
from actions_cache_run_history import CD_PATH
from actions_cargo_budget import MAX_CHARGE, owner
from staging_cd_common import matches, require
from staging_cd_versions import REPOSITORY

FIRST_WRITER = "b86270e3c3cdc1454e2cfa74898ef82b79d23051"
# The complete automation trees from the two audited legacy workflows. Their
# stale-tip allocator and attempt-bound publisher prevent a future retry from
# saving. A run already admitted at cutover must still drain.
FENCED_TREES = {"29e024efdf89b68d37e25babef13679df384f07f", "78d630ada9971abc2137a81bcf8c4fd253050bbe"}
SAVE = "Retain accepted Cargo mounts"


def stamp(writer):
    return "%d-%d-%s" % writer


def utc(value):
    t = datetime.fromisoformat(value.replace("Z", "+00:00"))
    require(t.utcoffset() is not None, "Invalid release timestamp")
    return t.astimezone(timezone.utc)


def has_upload(jobs, run, attempt, service):
    """An exact, complete attempt job list distinguishes skipped from uncertain."""
    for job in jobs:
        require(
            job["run_id"] == run["id"]
            and job["run_attempt"] == attempt
            and job["head_sha"] == run["head_sha"]
            and job["status"] == "completed",
            "Incomplete historical job inventory",
        )
    writers = [j for j in jobs if j["name"] == f"Release {service} / Publish"]
    if not writers:
        return False
    require(len(writers) == 1, "Duplicate Cargo writer job")
    steps = [s for s in writers[0]["steps"] if s["name"] == SAVE]
    if not steps:
        # A cancelled runner may have disappeared after the service accepted
        # its bytes. Only an explicitly skipped step settles an existing job.
        return writers[0]["conclusion"] != "skipped"
    require(len(steps) == 1, "Duplicate Cargo save step")
    return not (steps[0]["status"] == "completed" and steps[0]["conclusion"] == "skipped")


class WriterHistory:
    def __init__(self, head):
        require(matches("[a-f0-9]{40}", head), "Invalid policy head")

        def git(*args):
            return subprocess.run(
                ["git", *args], check=True, capture_output=True, text=True, timeout=30
            ).stdout.strip()

        self.git = git
        history = git("rev-list", "--first-parent", "--reverse", head).splitlines()
        self.position = {sha: i for i, sha in enumerate(history)}
        self.first = self.position[FIRST_WRITER]
        introduced = git(
            "log",
            "--first-parent",
            "--diff-filter=A",
            "--format=%H",
            head,
            "--",
            ".github/scripts/actions_cargo_budget.py",
        ).splitlines()
        require(len(introduced) == 1, "Unknown Cargo admission cutover")
        self.cutover = self.position[introduced[0]]
        require(self.first < self.cutover, "Invalid Cargo writer history")

    def era(self, sha):
        i = self.position[sha]
        if i < self.first:
            return "before"
        if i >= self.cutover:
            return "coordinated"
        return "fenced" if self.git("rev-parse", sha + ":.github") in FENCED_TREES else "legacy"


def originals(api, history):
    # Run numbers are immutable. A missing original could be an old writer
    # whose allocation is still rerunnable. Refuse shortened/deleted history.
    runs = api.list("workflows/staging-cd.yml/runs", "workflow_runs")
    by_number = sorted(runs, key=lambda r: r["run_number"], reverse=True)
    require(any(r["id"] == api.run_id for r in by_number), "Current release absent")
    previous = None
    original = []
    for run in by_number:
        require(
            run["path"] == CD_PATH
            and run["event"] == "push"
            and run["head_branch"] == "develop"
            and run["head_repository"]["full_name"] == REPOSITORY
            and matches("[a-f0-9]{40}", run["head_sha"])
            and type(run["id"]) is int
            and run["id"] > 0
            and type(run["run_number"]) is int
            and type(run["run_attempt"]) is int
            and run["run_attempt"] > 0
            and (previous is None or previous == run["run_number"] + 1),
            "Incomplete original release history",
        )
        previous = run["run_number"]
        era = history.era(run["head_sha"])
        if era == "before":
            return original
        if era != "coordinated":
            original.append((run, era))
    raise ValueError("No original release witnesses the start of Cargo uploads")


def legacy_charge(api, history, migration, entries, now):
    visible = {
        stamp(owner(row.key)) for row in entries if row.ref == DEVELOP and row.size and CARGO.fullmatch(row.key)
    }
    if migration["complete"]:
        # Once the historical cohort drains, Actions can expire its run logs.
        # Retain each unresolved charge independently of those logs.
        migration["unsettled"] = sorted(set(migration["unsettled"]) - visible)
        return len(migration["unsettled"]) * MAX_CHARGE
    seen = set(migration["seen"]) | visible
    runs = originals(api, history)
    ids = {r["id"] for r, _ in runs}
    migration["seen"] = sorted(k for k in seen if int(k.partition("-")[0]) in ids)
    for run, era in runs:
        # A historical full retry can reserve a new AM and EG even when an
        # earlier publish-only retry failed. Count from the original creation.
        if run["status"] != "completed" or (era == "legacy" and now <= utc(run["created_at"]) + timedelta(days=30)):
            return None
    pending = []
    for run, _ in runs:
        for attempt in range(1, run["run_attempt"] + 1):
            missing = [s for s in ("am", "eg") if stamp((run["id"], attempt, s)) not in seen]
            if missing:
                jobs = api.list(f"runs/{run['id']}/attempts/{attempt}/jobs", "jobs")
                require(jobs, "Missing historical attempt jobs")
                pending.extend(stamp((run["id"], attempt, s)) for s in missing if has_upload(jobs, run, attempt, s))
    migration.update(complete=True, seen=[], unsettled=sorted(pending))
    return len(pending) * MAX_CHARGE
