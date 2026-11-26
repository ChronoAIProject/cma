"""Trusted admission timestamps; queueing and retries cannot renew the budget."""
from datetime import datetime, timezone
import os
import time

from cd_target import (PRODUCTION, STAGING, TARGETS, branch as target_branch,
                       workflows as target_workflows)
from ci_outcome import error_site, report
from staging_cd_common import matches, require

ADMISSION_SECONDS = 120
BOOTSTRAP_ADMISSION_SECONDS = 45
BOOTSTRAP_TOTAL_SECONDS = 600
RELEASE_TOTAL_SECONDS = 990
# A production rollout takes far longer than staging's release budget. Only a
# production release requests this total; every other request is unchanged.
PRODUCTION_RELEASE_TOTAL_SECONDS = 3600
CONTROLLER_TOTAL_ENV = "CD_CONTROLLER_TOTAL_SECONDS"
TERMINATION_GRACE_SECONDS = 15
# A release controller lives at most its total from admission, then the grace.
WATCHDOG_SECONDS = RELEASE_TOTAL_SECONDS + TERMINATION_GRACE_SECONDS
PRODUCTION_WATCHDOG_SECONDS = PRODUCTION_RELEASE_TOTAL_SECONDS + TERMINATION_GRACE_SECONDS
ADMISSION_FAILURE = "admission-time"


def controller_seconds(mode, target=STAGING):
    require(mode in ("release", "bootstrap"), "Unknown deployment mode")
    require(target in TARGETS, "Unknown deployment target")
    if mode == "bootstrap":
        return BOOTSTRAP_TOTAL_SECONDS
    return PRODUCTION_RELEASE_TOTAL_SECONDS if target == PRODUCTION else RELEASE_TOTAL_SECONDS


def timestamp(now=None):
    return datetime.fromtimestamp(time.time() if now is None else now, timezone.utc).isoformat().replace("+00:00", "Z")


def elapsed(started, now=None):
    require(matches(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,6})?Z", started),
            "Deployment admission timestamp is malformed")
    start = datetime.fromisoformat(started.replace("Z", "+00:00")).timestamp()
    age = (time.time() if now is None else now) - start
    require(age >= 0, "Deployment admission timestamp is in the future")
    return age


def remaining(started, limit=None, now=None, *, mode="release"):
    if limit is None:
        require(mode in ("release", "bootstrap"), "Unknown deployment admission mode")
        limit = BOOTSTRAP_ADMISSION_SECONDS if mode == "bootstrap" else ADMISSION_SECONDS
    left = limit - elapsed(started, now)
    require(left > 0, "Deployment admission budget expired before mutation")
    return left


def run_created(env, request):
    require(matches(r"[1-9][0-9]*", env.get("GITHUB_RUN_ID")), "Deployment run identity is invalid")
    workflows, branch = target_workflows(env), target_branch(env)
    event = env.get("GITHUB_EVENT_NAME")
    require(event in workflows, "Deployment event cannot supply an admission clock")
    run = request("actions/runs/" + env["GITHUB_RUN_ID"])
    require(isinstance(run, dict) and run.get("id") == int(env["GITHUB_RUN_ID"])
            and run.get("event") == event and run.get("head_branch") == branch
            and run.get("head_sha") == env["GITHUB_SHA"]
            and run.get("path") == ".github/workflows/" + workflows[event]
            and isinstance(run.get("repository"), dict)
            and run["repository"].get("full_name") == "ChronoAIProject/cma",
            "Deployment run timestamp is not trusted")
    started = run.get("created_at")
    elapsed(started)
    return started


def dispatch_started(env, request, mode="release"):
    require(env.get("GITHUB_EVENT_NAME") == "workflow_dispatch", "Dispatch timestamp requires a dispatch event")
    started = run_created(env, request)
    remaining(started, mode=mode)
    return started


def main():
    try:
        remaining(os.environ.get("CD_STARTED_AT"), mode=os.environ.get("CD_DEPLOY_MODE", "release"))
    except (ValueError, TypeError, OverflowError) as error:
        # The line is unchanged; the reported check says which bound refused.
        report("deploy-clock", ADMISSION_FAILURE, subject="deploy-admission", site=error_site(error))
        return 30
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
