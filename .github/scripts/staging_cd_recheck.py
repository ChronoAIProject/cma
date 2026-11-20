"""Account for new pushes before leaving a reconciled release run."""
import json
import os
import time

from staging_cd_admission import event_identity
from staging_cd_common import matches, require
from staging_cd_github import GitHub
from staging_cd_runtime import run_private

# The job allows five minutes. Reads refuse cleanly after this budget instead of
# being killed with the job; checkout and startup take well under a minute.
RECHECK_SECONDS = 180


def recheck(head, api):
    current = api.request("git/ref/heads/develop")
    require(isinstance(current, dict) and current.get("ref") == "refs/heads/develop"
            and isinstance(current.get("object"), dict) and current["object"].get("type") == "commit"
            and matches(r"[0-9a-f]{40}", current["object"].get("sha")), "Develop head could not be rechecked")
    revision = current["object"]["sha"]
    if revision == head:
        return True
    runs = api.request("actions/workflows/staging-cd.yml/runs?event=push&head_sha=" + revision + "&per_page=100")
    require(isinstance(runs, dict) and isinstance(runs.get("workflow_runs"), list), "New push runs could not be read")
    candidates = [run for run in runs["workflow_runs"] if run.get("head_sha") == revision
                  and run.get("event") == "push" and run.get("head_branch") == "develop"
                  and run.get("path") == ".github/workflows/staging-cd.yml"]
    require(any(run.get("status") in ("queued", "in_progress", "waiting", "requested", "pending")
                or (run.get("status") == "completed" and run.get("conclusion") == "success") for run in candidates),
            "Newer merges have no active or successful reconciliation run")
    return True


def main():
    def operation(_masker):
        event_identity(os.environ)
        api = GitHub(os.environ.get("GH_TOKEN"), deadline=time.monotonic() + RECHECK_SECONDS)
        return recheck(json.loads(os.environ["CD_QUEUE"])["head"], api)
    return run_private(operation, None)


if __name__ == "__main__":
    raise SystemExit(main())
