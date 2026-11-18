"""Trusted admission timestamps; queueing and retries cannot renew the budget."""
from datetime import datetime, timezone
import os
import time

from staging_cd_common import matches, require

ADMISSION_SECONDS = 45
TOTAL_SECONDS = 600
TERMINATION_GRACE_SECONDS = 15
WATCHDOG_SECONDS = TOTAL_SECONDS + TERMINATION_GRACE_SECONDS


def timestamp(now=None):
    return datetime.fromtimestamp(time.time() if now is None else now, timezone.utc).isoformat().replace("+00:00", "Z")


def elapsed(started, now=None):
    require(matches(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,6})?Z", started),
            "Deployment admission timestamp is malformed")
    start = datetime.fromisoformat(started.replace("Z", "+00:00")).timestamp()
    age = (time.time() if now is None else now) - start
    require(age >= 0, "Deployment admission timestamp is in the future")
    return age


def remaining(started, limit=ADMISSION_SECONDS, now=None):
    left = limit - elapsed(started, now)
    require(left > 0, "Deployment admission budget expired before mutation")
    return left


def run_created(env, request):
    require(matches(r"[1-9][0-9]*", env.get("GITHUB_RUN_ID")), "Deployment run identity is invalid")
    workflows = {"push": "staging-cd.yml", "workflow_dispatch": "redeploy-staging.yml"}
    event = env.get("GITHUB_EVENT_NAME")
    require(event in workflows, "Deployment event cannot supply an admission clock")
    run = request("actions/runs/" + env["GITHUB_RUN_ID"])
    require(isinstance(run, dict) and run.get("id") == int(env["GITHUB_RUN_ID"])
            and run.get("event") == event and run.get("head_branch") == "develop"
            and run.get("head_sha") == env["GITHUB_SHA"]
            and run.get("path") == ".github/workflows/" + workflows[event]
            and isinstance(run.get("repository"), dict)
            and run["repository"].get("full_name") == "ChronoAIProject/cma",
            "Deployment run timestamp is not trusted")
    started = run.get("created_at")
    elapsed(started)
    return started


def dispatch_started(env, request):
    require(env.get("GITHUB_EVENT_NAME") == "workflow_dispatch", "Dispatch timestamp requires a dispatch event")
    started = run_created(env, request)
    remaining(started)
    return started


def main():
    try:
        remaining(os.environ.get("CD_STARTED_AT"))
    except (ValueError, TypeError, OverflowError):
        print("FAIL: deployment not admitted; invalid or expired admission time")
        return 30
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
