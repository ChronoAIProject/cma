"""Avoid repository-wide cache work after a proven idle staging run."""
import json
import os
import subprocess
import time
from pathlib import Path

from actions_cache_sweep import authorized_completion
from staging_cd_common import matches
from staging_cd_github import GitHub
from staging_cd_versions import DEPLOYABLES

# These control jobs do not produce hosted image caches. The plan may hand
# off to a newer push before allocation, with reporting still succeeding.
CONTROL_OUTCOMES = {
    "plan": ("success",),
    "allocate": ("success", "skipped"),
    # Builds use the job's local BuildKit cache and publish only image evidence.
    "Publish journey probe": ("success",),
    "Report component outcomes": ("success",),
}
PROVEN_SKIP = frozenset({
    "infrastructure", "Publish protected Web", "Promote contract", "Signal verified image publication",
    *(f"Release {component}" for component in DEPLOYABLES),
    *(f"Promote {component}" for component in DEPLOYABLES),
})


def idle_attempt(inventory, run_id, attempt, source):
    if not isinstance(inventory, dict) or not isinstance(inventory.get("jobs"), list):
        return False
    jobs = inventory["jobs"]
    if type(inventory.get("total_count")) is not int or inventory["total_count"] != len(jobs):
        return False
    ids, names = set(), set()
    for job in jobs:
        if not isinstance(job, dict):
            return False
        name, job_id = job.get("name"), job.get("id")
        if (not isinstance(name, str) or not name or name in names
                or type(job_id) is not int or job_id <= 0 or job_id in ids
                or type(job.get("run_id")) is not int or job["run_id"] != run_id
                or type(job.get("run_attempt")) is not int or job["run_attempt"] != attempt
                or job.get("head_sha") != source or job.get("status") != "completed"):
            return False
        if job.get("conclusion") not in CONTROL_OUTCOMES.get(name, ("skipped",)):
            return False
        names.add(name)
        ids.add(job_id)
    return (CONTROL_OUTCOMES.keys() | PROVEN_SKIP) <= names


def needs_maintenance(env, client=GitHub):
    try:
        if not authorized_completion(env):
            return True
        run = json.loads(Path(env["GITHUB_EVENT_PATH"]).read_text())["workflow_run"]
        if run["path"] != ".github/workflows/staging-cd.yml" or run.get("conclusion") != "success":
            return True
        run_id, attempt, source = run.get("id"), run.get("run_attempt"), run.get("head_sha")
        if (type(run_id) is not int or run_id <= 0 or type(attempt) is not int or attempt <= 0
                or not matches(r"[a-f0-9]{40}", source)):
            return True
        # The exact attempt matters for reruns. A generic jobs listing can
        # otherwise report a different attempt with a different disposition.
        inventory = client(env["GH_TOKEN"]).request(
            f"actions/runs/{run_id}/attempts/{attempt}/jobs?per_page=100",
            attempts=2, deadline=time.monotonic() + 15,
        )
        return not idle_attempt(inventory, run_id, attempt, source)
    except (OSError, KeyError, TypeError, ValueError, AttributeError, subprocess.SubprocessError):
        # An optional shortcut only takes effect with complete positive proof.
        return True


def main(env):
    needed = needs_maintenance(env)
    with Path(env["GITHUB_OUTPUT"]).open("a") as output:
        output.write(f"needed={'true' if needed else 'false'}\n")
    print("Cache maintenance decision: " + ("maintain" if needed else "no staging component work"), flush=True)


if __name__ == "__main__":
    main(os.environ)
