"""Require uninterrupted evidence for every possible develop Cargo reader."""
from datetime import datetime, time, timedelta, timezone

from staging_cd_versions import REPOSITORY

CD_PATH = ".github/workflows/staging-cd.yml"
# Original 117 (44549034862, develop 32d9a42d) already existed before the v4
# protocol was proposed. Its retries keep the original workflow and namespace.
FIRST_POSSIBLE_V4_RUN = 118
# A retry can start within 30 days and can remain active for up to 35 days.
ACTIVE_HORIZON = timedelta(days=65)


class IncompleteHistory(ValueError):
    """GitHub's search cap or missing run evidence prevents an ownership proof."""


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
                    or type(run["id"]) is not int or run["id"] <= 0
                    or type(run["run_number"]) is not int or run["run_number"] <= 0
                    or type(run["run_attempt"]) is not int or run["run_attempt"] < 1):
                raise IncompleteHistory("invalid develop release run identity")
            yield run


def cargo_readers_finished(api, day, now):
    boundary = datetime.combine(day + timedelta(days=1), time.min, timezone.utc)
    oldest = now - ACTIVE_HORIZON
    if boundary <= oldest:
        return True
    # Include a week before the age bound so a live installation can advance
    # the lower witness as GitHub retires historical run records.
    first = FIRST_POSSIBLE_V4_RUN
    after = None
    history = {}
    active = set()
    try:
        for run in release_runs(api, oldest - timedelta(days=7), now):
            number = run["run_number"]
            created = datetime.fromisoformat(run["created_at"].replace("Z", "+00:00"))
            if created.tzinfo is None or created > now:
                raise IncompleteHistory("invalid original release time")
            identity = (run["id"], created)
            if history.setdefault(number, identity) != identity:
                raise IncompleteHistory("conflicting release run number")
            # Overlapping pages/windows can see different attempts. Retain
            # an observed nonterminal reader for the duration of this census.
            if run["status"] != "completed":
                active.add(number)
            if created < oldest:
                first = max(first, number + 1)
            if created >= boundary:
                after = min(number, after) if after is not None else number
    except (KeyError, TypeError, ValueError):
        # Incomplete and malformed observations cannot authorize an ID deletion.
        return False
    if after is None:
        return False
    times = [history[n][1] for n in sorted(history)]
    if times != sorted(times) or after < first or after - first > len(history):
        return False
    # A known post-midnight original closes the upper end, including a
    # missing latest overnight reader. A consistent API count can hide gaps.
    return all(n in history and n not in active for n in range(first, after))
