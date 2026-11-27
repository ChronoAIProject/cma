"""Recover outstanding promotions from the latest serialized staging writers."""
import collections
import itertools
import re
import subprocess

from staging_cd_allocation_io import AllocationIO, parallel_read
from staging_cd_common import matches, require
from staging_cd_compatibility import GROUP
from staging_cd_github import GitHub
from staging_cd_receipts import sha256
from staging_cd_set import private_document, release_set
from staging_cd_targets import TARGETS
from staging_cd_versions import REPOSITORY

PAGE_SIZE = 12
FENCE_SECONDS = 2
PREFIX = "Verified staging receipts "
PROTECTED_WEB_JOB = "Publish protected Web / Protect Web rollout / "
WEB_OBSERVER_JOB = PROTECTED_WEB_JOB + "Read serving Web"
WORKFLOWS = {".github/workflows/staging-cd.yml": ("push", "infrastructure"),
             ".github/workflows/redeploy-staging.yml": ("workflow_dispatch", "resolve")}
# The newest writer of each component, read as one fenced snapshot: proofs of
# finished writers, components any listed writer covered, components whose
# newest writer is unfinished, and when the oldest examined writer was created
# (None when the listing holds every deployment).
Snapshot = collections.namedtuple("Snapshot", "proofs observed running since")
# Failures that leave the controller outcome unknown.
UNAVAILABLE = (OSError, ValueError, KeyError, TypeError, AttributeError, subprocess.SubprocessError)


def proof(components):
    return ",".join(sorted(components)) + " " + sha256(components)


def proven_components(evidence, latest, history):
    result = set()
    require(isinstance(evidence, dict) and evidence.keys() <= TARGETS, "Invalid controller proof map")
    for component, row in evidence.items():
        require(isinstance(row, dict) and set(row) == {"writer", "stamp"} and isinstance(row["stamp"], str)
                and matches(r"[0-9a-f]{40}", row["writer"]), "Invalid controller proof")
        stamp = row["stamp"]
        names = stamp.partition(" ")[0]
        keys = names.split(",")
        if component not in keys or not set(keys) <= latest.keys():
            continue
        receipts = {key: latest[key] for key in keys}
        if row["writer"] not in history.positions or any(history.position(r["source"]) > history.position(row["writer"])
                                                        for r in receipts.values()):
            continue
        # A previous mixed-component stamp cannot prove a standalone two-phase Web release.
        if len(receipts) > 1 and "asset_continuity" in receipts.get("web", {}):
            continue
        request = private_document(release_set(receipts))
        if proof(request["components"]) == stamp:
            result.add(component)
    return result


def observation(deployment, request):
    # A writer without a usable result invalidates any earlier success for its
    # scope. An unidentified writer may have affected every component.
    blocked = (TARGETS, "")
    number, source = deployment["id"], deployment["sha"]
    require(type(number) is int and number > 0 and matches(r"[0-9a-f]{40}", source),
            "Invalid staging deployment identity")
    statuses = request(f"deployments/{number}/statuses?per_page=6")
    if not isinstance(statuses, list) or len(statuses) > 6:
        return blocked
    # GitHub can mark an older environment deployment inactive after another
    # component deploys. That bookkeeping status is not a controller attempt.
    status = next((row for row in statuses if row.get("state") != "inactive"), {})
    link = re.fullmatch(r"https://github\.com/" + REPOSITORY + r"/actions/runs/([1-9][0-9]*)/job/([1-9][0-9]*)",
                        status.get("log_url") or "")
    if not link:
        return blocked
    run_id, job_id = map(int, link.groups())
    job = request(f"actions/jobs/{job_id}")
    if job.get("id") != job_id or job.get("run_id") != run_id or job.get("head_sha") != source:
        return blocked
    run = request(f"actions/runs/{run_id}/attempts/{job['run_attempt']}")
    workflow = WORKFLOWS.get(run.get("path"))
    if (not workflow or run.get("id") != run_id or run.get("run_attempt") != job["run_attempt"]
            or run.get("repository", {}).get("full_name") != REPOSITORY
            or run.get("head_repository", {}).get("full_name") != REPOSITORY or run.get("event") != workflow[0]
            or run.get("head_sha") != source or run.get("head_branch") != "develop" or deployment.get("ref") != "develop"):
        return blocked
    if job["name"] == workflow[1] or (run["event"] == "push" and job["name"] == WEB_OBSERVER_JOB):
        return set(), ""
    scope = TARGETS
    if run["event"] == "push":
        scope = push_scope(job["name"])
        if scope is None:
            return blocked
    elif job["name"] != "deploy / Deploy set":
        return blocked
    return scope, job_proof(job, scope, source)


def push_scope(name):
    """The components a develop push run's deploy job writes, or None for any other job."""
    name = name or ""
    protected = name.startswith(PROTECTED_WEB_JOB)
    lane = re.fullmatch(r"Promote ([a-z-]+) / Deploy admitted components(?: again)? / Deploy set",
                        name.removeprefix(PROTECTED_WEB_JOB))
    if not lane or lane[1] not in TARGETS | {"contract"}:
        return None
    if protected and lane[1] != "web":
        return None
    return GROUP if lane[1] == "contract" else {lane[1]}


def job_proof(job, scope, source):
    """A deploy job's controller proof; blank without a usable result, None while unfinished."""
    if job["status"] != "completed":
        return None
    if job["conclusion"] == "cancelled":
        return ""
    steps = job["steps"]
    launch = [index for index, step in enumerate(steps) if step["name"] == "Coordinate the immutable deployment set"
              and step["conclusion"] in ("success", "failure")]
    if len(launch) != 1:
        return ""
    stamps = [step["name"][len(PREFIX):] for step in steps[launch[0] + 1:] if step["name"].startswith(PREFIX)
              and step["conclusion"] == "success"]
    if len(stamps) != 1:
        return ""
    names, _, digest = stamps[0].partition(" ")
    keys = names.split(",")
    if keys != sorted(set(keys)) or not set(keys) <= scope or not matches(r"[0-9a-f]{64}", digest):
        return ""
    return {"writer": source, "stamp": stamps[0]}


def inventory(request, environment):
    value = snapshot(request, environment)
    return {} if value is None else value.proofs


def deployment_pages(request, listing, environment):
    previous = None
    for page in itertools.count(1):
        suffix = "" if page == 1 else f"&page={page}"
        query = f"{listing}{PAGE_SIZE}{suffix}"
        rows = request(query)
        require(isinstance(rows, list) and len(rows) <= PAGE_SIZE
                and all(row.get("environment") == environment for row in rows), "Invalid staging deployment inventory")
        ids = [row["id"] for row in rows]
        require(ids == sorted(set(ids), reverse=True), "Staging deployment order is uncertain")
        if previous is not None:
            previous_query, previous_ids = previous
            require(not ids or ids[0] < previous_ids[-1], "Staging deployment order is uncertain")
            # An insertion or deletion can shift the page offset and skip a
            # writer. Confirm the preceding page still meets this boundary.
            require([row["id"] for row in request(previous_query)] == previous_ids,
                    "Staging deployment page changed while reading")
        yield rows
        if len(rows) < PAGE_SIZE:
            break
        previous = query, ids


def snapshot(request, environment, *, fence_request=None):
    """Read each lane's newest writer, keeping only the contiguous prefix examined."""
    require(matches(r"[a-z0-9-]+", environment), "Invalid staging environment")
    listing = f"deployments?environment={environment}&per_page="
    remaining, evidence, running = set(TARGETS), {}, set()
    newest_ids, since = None, None
    try:
        for rows in deployment_pages(request, listing, environment):
            if newest_ids is None:
                newest_ids = [row["id"] for row in rows[:1]]
            # All automatic and manual mutations acquire the same lock before
            # these jobs start. An earlier write cannot supersede a newer one.
            for start in range(0, len(rows), 8):
                batch = rows[start:start + 8]
                for scope, result in parallel_read(lambda row: observation(row, request), batch):
                    for component in remaining & scope:
                        if result:
                            evidence[component] = result
                        elif result is None:
                            running.add(component)
                    remaining -= scope
                since = batch[-1].get("created_at", "")
                if not remaining:
                    break
            if len(rows) < PAGE_SIZE:
                since = None  # Every older deployment has been listed.
                break
            if not remaining:
                break
    except UNAVAILABLE:
        if since is None:
            raise
        # A timed-out or unreadable older page does not erase proof from the
        # newer prefix. It cannot establish an outcome for an unseen lane.
    newest = (fence_request or request)(listing + "1")
    if not isinstance(newest, list) or [row.get("id") for row in newest[:1]] != newest_ids:
        return None  # A writer started while the snapshot was being read.
    return Snapshot(evidence, TARGETS - remaining, running, since)


def read_snapshot(token, environment, *, seconds=10):
    # Reserve time for rechecking the newest writer even when searching older pages
    # spends the scan budget. Both reads use the same read-only permissions.
    def client(budget):
        io = AllocationIO(seconds=budget)
        return GitHub(token, runner=io.read_once, retry=io.retry).request

    return snapshot(client(seconds - FENCE_SECONDS), environment,
                    fence_request=lambda path: client(FENCE_SECONDS)(path))


def available_proofs(token, environment):
    try:
        value = read_snapshot(token, environment)
    except UNAVAILABLE:
        # Missing evidence can add a promotion for an independently qualified
        # final. It never grants a reservation, a version or admission.
        print("Staging controller verification unavailable; final receipts require promotion")
        return {}
    return {} if value is None else value.proofs
