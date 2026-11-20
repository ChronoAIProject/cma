"""Account for new pushes before leaving a reconciled release run."""
import json
import os

from staging_cd_admission import event_identity
from staging_cd_common import matches, require
from staging_cd_github import GitHub
from staging_cd_resolve import output
from staging_cd_runtime import run_private


def recheck(head, api):
    """True once develop has advanced past this run and a newer run covers it; False at the head."""
    current = api.request("git/ref/heads/develop")
    require(isinstance(current, dict) and current.get("ref") == "refs/heads/develop"
            and isinstance(current.get("object"), dict) and current["object"].get("type") == "commit"
            and matches(r"[0-9a-f]{40}", current["object"].get("sha")), "Develop head could not be rechecked")
    revision = current["object"]["sha"]
    if revision == head:
        return False
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
        advanced = recheck(json.loads(os.environ["CD_QUEUE"])["head"], GitHub(os.environ.get("GH_TOKEN")))
        output(os.environ, "advanced", "true" if advanced else "false")
        return True
    return run_private(operation, None)


if __name__ == "__main__":
    raise SystemExit(main())
