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


# GitHub creates a push's workflow run a few seconds after the push, and the
# runs listing can lag further. A head that has just moved therefore often has
# no visible run yet. Run 44965203978 read a head pushed at 00:09:33Z while that
# head's own run, created the same second, was not listed, and failed. Wait for
# the newer head's run within these bounded steps (59 s in all, well inside
# RECHECK_SECONDS) before concluding that it has none.
RUN_WAITS = (2, 4, 8, 15, 30)
ACTIVE = ("queued", "in_progress", "waiting", "requested", "pending")


def develop_head(api):
    current = api.request("git/ref/heads/develop")
    require(isinstance(current, dict) and current.get("ref") == "refs/heads/develop"
            and isinstance(current.get("object"), dict) and current["object"].get("type") == "commit"
            and matches(r"[0-9a-f]{40}", current["object"].get("sha")), "Develop head could not be rechecked")
    return current["object"]["sha"]


def push_runs(api, revision):
    runs = api.request("actions/workflows/staging-cd.yml/runs?event=push&head_sha=" + revision + "&per_page=100")
    require(isinstance(runs, dict) and isinstance(runs.get("workflow_runs"), list), "New push runs could not be read")
    return [run for run in runs["workflow_runs"] if run.get("head_sha") == revision
            and run.get("event") == "push" and run.get("head_branch") == "develop"
            and run.get("path") == ".github/workflows/staging-cd.yml"]


# Time kept after the last wait for that pass's two reads.
READ_RESERVE_SECONDS = 15


def recheck(head, api, *, sleep=time.sleep, clock=time.monotonic):
    deadline = getattr(api, "deadline", None)
    for wait in (0, *RUN_WAITS):
        if wait:
            # Never sleep into the read deadline; refuse with the domain reason
            # instead of a later "GitHub read budget expired".
            if isinstance(deadline, (int, float)) and clock() + wait + READ_RESERVE_SECONDS > deadline:
                break
            sleep(wait)
        # Develop may move again while waiting; the newest head is what matters.
        revision = develop_head(api)
        if revision == head:
            return True
        candidates = push_runs(api, revision)
        if any(run.get("status") in ACTIVE
               or (run.get("status") == "completed" and run.get("conclusion") == "success") for run in candidates):
            return True
        # A run that exists but did not succeed is a real gap; waiting cannot fix it.
        require(not candidates, "Newer merges have no active or successful reconciliation run")
    require(False, "Newer develop head never showed a reconciliation run within the wait")


def main():
    def operation(_masker):
        event_identity(os.environ)
        api = GitHub(os.environ.get("GH_TOKEN"), deadline=time.monotonic() + RECHECK_SECONDS)
        return recheck(json.loads(os.environ["CD_QUEUE"])["head"], api)
    return run_private(operation, None, subject="develop-recheck")


if __name__ == "__main__":
    raise SystemExit(main())
