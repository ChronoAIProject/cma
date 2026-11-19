"""Bounded discovery of develop release runs by immutable creation time."""
from datetime import datetime, time, timedelta, timezone

from staging_cd_versions import REPOSITORY

CD_PATH = ".github/workflows/staging-cd.yml"
# An original workflow may be retried for 30 days. An active attempt can then
# run for up to 35 days including queue and approval time.
ACTIVE_HORIZON = timedelta(days=65)


class IncompleteHistory(ValueError):
    """A filtered Actions search can return at most 1,000 workflow runs."""


def windows(first, end):
    while end > first:
        start = max(first, end - timedelta(days=7))
        yield f"{start:%Y-%m-%dT%H:%M:%SZ}..{end:%Y-%m-%dT%H:%M:%SZ}"
        end = start


def release_runs(api, first, end):
    for window in windows(first, end):
        runs = api.list("workflows/staging-cd.yml/runs", "workflow_runs",
                        event="push", branch="develop", created=window)
        for run in runs:
            if (run["path"] != CD_PATH or run["event"] != "push" or run["head_branch"] != "develop"
                    or run["head_repository"]["full_name"] != REPOSITORY
                    or type(run["run_attempt"]) is not int or run["run_attempt"] < 1):
                raise IncompleteHistory("invalid develop release run identity")
            yield run


def cargo_readers_finished(api, day, now):
    # A retry that starts after this UTC boundary computes a different Cargo
    # family, even at a historical source SHA. Earlier attempts must be terminal.
    end = datetime.combine(day + timedelta(days=1), time.min, timezone.utc)
    try:
        return all(run["status"] == "completed" for run in release_runs(api, now - ACTIVE_HORIZON, end))
    except IncompleteHistory:
        return False
