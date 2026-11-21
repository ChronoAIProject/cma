"""Recover outstanding promotions from the latest serialized staging writers."""
import collections
import json
import os
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

LIMIT = 12
PREFIX = "Verified staging receipts "
WORKFLOWS = {".github/workflows/staging-cd.yml": ("push", "infrastructure"),
             ".github/workflows/redeploy-staging.yml": ("workflow_dispatch", "resolve")}
# The newest writer of each component, read as one fenced snapshot: proofs of
# finished writers, components any listed writer covered, components whose
# newest writer is unfinished, and when the oldest listed writer was created
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
    if job["name"] == workflow[1]:
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
    lane = re.fullmatch(r"Promote ([a-z-]+) / Deploy admitted components / Deploy set", name or "")
    if not lane or lane[1] not in TARGETS | {"contract"}:
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


def snapshot(request, environment):
    """The newest writers' outcomes, or None when a writer started during the read."""
    require(matches(r"[a-z0-9-]+", environment), "Invalid staging environment")
    listing = f"deployments?environment={environment}&per_page="
    rows = request(listing + str(LIMIT))
    require(isinstance(rows, list) and len(rows) <= LIMIT
            and all(row.get("environment") == environment for row in rows), "Invalid staging deployment inventory")
    ids = [row["id"] for row in rows]
    require(ids == sorted(set(ids), reverse=True), "Staging deployment order is uncertain")
    # All automatic and manual mutations acquire the same lock before this
    # environment job starts. Older pages cannot supersede a more recent job.
    remaining, evidence, running = set(TARGETS), {}, set()
    for start in range(0, len(rows), 4):
        for scope, observation_proof in parallel_read(lambda row: observation(row, request), rows[start:start + 4]):
            for key in remaining & scope:
                if observation_proof:
                    evidence[key] = observation_proof
                elif observation_proof is None:
                    running.add(key)
            remaining -= scope
        if not remaining:
            break
    newest = request(listing + "1")
    if not isinstance(newest, list) or [row.get("id") for row in newest[:1]] != ids[:1]:
        return None  # A writer started while the snapshot was being read.
    since = rows[-1].get("created_at") if len(rows) == LIMIT else None
    return Snapshot(evidence, TARGETS - remaining, running, since)


def read_snapshot(token, environment):
    # The evidence read has no write permission; it raises UNAVAILABLE errors.
    io = AllocationIO(seconds=10)
    return snapshot(GitHub(token, runner=io.read_once, retry=io.retry).request, environment)


def main():
    try:
        # The evidence read runs before allocation.
        value = read_snapshot(os.environ["GH_TOKEN"], os.environ["CD_STAGING_ENVIRONMENT"])
    except UNAVAILABLE:
        # Missing evidence can add a promotion for an independently qualified
        # final. It never grants a reservation, a version or admission.
        value = None
        print("Staging controller verification unavailable; final receipts require promotion")
    evidence = {} if value is None else value.proofs
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        output.write("verified=" + json.dumps(evidence, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
