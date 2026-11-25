"""Sweep rebuildable caches by immutable ID and account for the remaining bytes."""
import argparse
import json
import os
import re
import subprocess
import time
from collections import Counter
from datetime import datetime, timedelta, timezone
from pathlib import Path
from urllib.parse import urlencode
from urllib.request import urlopen

from actions_cache_retention import BUDGET, DEVELOP, PULL, Entry, Policies, Readers, plan, recovery_kind
from actions_cache_run_history import IncompleteHistory, cargo_reader_hold
from actions_cache_report import publish_result, retained_family
from staging_cd_github import GitHub
from staging_cd_versions import REPOSITORY


class InconsistentInventory(ValueError):
    """GitHub's count and pages disagreed; the listing changed while it was read."""


# A listing is re-read from its first page this many times before the sweep
# refuses. Runs change state, and caches are saved, while a listing is read:
# GitHub computes total_count apart from each page. With the develop merge
# queue, a single read of the run states disagreed with itself often enough
# that every sweep from 01:27 to 02:20 UTC on 21 Nov 2026 failed.
INVENTORY_READS = 4
INVENTORY_BACKOFF_SECONDS = (1, 3, 9)


def pages(fetch, field):
    rows = []
    total = None
    page = 1
    while True:
        doc = fetch(page)
        count, values = doc["total_count"], doc[field]
        if type(count) is not int or count < 0 or not isinstance(values, list):
            raise ValueError("inventory page is invalid")
        if total is not None and total != count:
            raise InconsistentInventory("inventory changed while it was read")
        rows.extend(values)
        total = count
        if len(rows) == total:
            break
        if len(rows) > total or len(values) != 100:
            raise InconsistentInventory("incomplete inventory page")
        page += 1
    if len({row["id"] for row in rows}) != total:
        raise InconsistentInventory("duplicate inventory IDs")
    return rows


def consistent(read, sleep=time.sleep):
    """One self-consistent listing, or the last disagreement after bounded re-reads."""
    for attempt in range(INVENTORY_READS):
        try:
            return read()
        except InconsistentInventory:
            if attempt == INVENTORY_READS - 1:
                raise
            sleep(INVENTORY_BACKOFF_SECONDS[attempt])


class Actions:
    def __init__(self, token, run_id, sleep=time.sleep):
        self.api = GitHub(token)
        self.run_id = run_id
        self.sleep = sleep

    def list(self, resource, field, **filters):
        def fetch(page):
            query = urlencode(dict(filters, page=page, per_page=100)).replace("..", "%2E%2E")
            doc = self.api.request(f"actions/{resource}?{query}")
            if field == "workflow_runs" and filters and doc["total_count"] >= 1000:
                raise IncompleteHistory("filtered workflow history reached the result limit")
            return doc
        # Every re-read starts at page one, so rows from two listings never mix.
        return consistent(lambda: pages(fetch, field), self.sleep)

    def entries(self, **filters):
        return [Entry.parse(row) for row in self.list("caches", "actions_caches", **filters)]

    def active_runs(self):
        # A queued run can begin executing between calls. Check executing runs
        # last and include waiting/pending approvals that may have finished jobs.
        runs = []
        for state in ("requested", "queued", "pending", "waiting", "in_progress"):
            runs.extend(self.list("runs", "workflow_runs", status=state))
        return runs

    def readers(self):
        return Readers.from_runs(self.active_runs(), self.run_id)

    def legacy_readers_expired(self):
        # A re-run uses the original workflow SHA for 30 days. Its readers can
        # bypass the new lock. A completed-build retention event older than
        # that window witnesses deployment of this workflow on default develop.
        cutoff = datetime.now(timezone.utc) - timedelta(days=30)
        query = urlencode({"event": "workflow_run", "branch": "develop", "per_page": 1,
                           "created": "<" + cutoff.isoformat(timespec="seconds")})
        data = self.api.request("actions/workflows/cache-retention.yml/runs?" + query)
        runs = data["workflow_runs"]
        if not isinstance(runs, list) or len(runs) > 1:
            raise ValueError("invalid retention adoption evidence")
        return bool(runs and runs[0]["event"] == "workflow_run" and runs[0]["head_branch"] == "develop"
                    and runs[0]["path"] == ".github/workflows/cache-retention.yml"
                    and runs[0]["head_repository"]["full_name"] == REPOSITORY
                    and datetime.fromisoformat(runs[0]["created_at"].replace("Z", "+00:00")) < cutoff)

    def require_live_policy(self, revision):
        if not isinstance(revision, str) or not re.fullmatch(r"[a-f0-9]{40}", revision):
            raise ValueError("invalid retention policy revision")
        # The checkout is immutable, but a scheduled run may start well after
        # its ref was resolved. Checking the public ref needs no contents grant.
        result = subprocess.run(
            ["git", "-c", "credential.helper=", "ls-remote", "--exit-code",
             "https://github.com/ChronoAIProject/cma.git", DEVELOP],
            check=True, capture_output=True, text=True, timeout=20,
            env={"PATH": os.environ["PATH"], "GIT_TERMINAL_PROMPT": "0"})
        if result.stdout.strip() != f"{revision}\t{DEVELOP}":
            raise ValueError("develop changed since the retention policy was loaded")

    def closed_pull(self, ref):
        # The repository is public. Reading public PR state needs no credential
        # and keeps pull-requests permission out of the destructive job.
        number = int(PULL.fullmatch(ref)[1])
        with urlopen(f"https://api.github.com/repos/{REPOSITORY}/pulls/{number}", timeout=20) as response:
            doc = json.load(response)
        if (doc["number"] != number or doc["base"]["repo"]["full_name"] != REPOSITORY
                or doc["state"] not in ("open", "closed")):
            raise ValueError("invalid public PR identity or state")
        return doc["state"] == "closed" and bool(doc["closed_at"])

    def visible(self, entry):
        # A PR may choose arbitrary key text. Only trusted develop key patterns
        # go into query parameters, and the mutation always uses the numeric ID.
        filters = {"ref": entry.ref}
        if entry.ref == DEVELOP:
            filters["key"] = entry.key
        return next((row for row in self.entries(**filters)
                     if row.id == entry.id and row.key == entry.key and row.ref == entry.ref
                     and row.version == entry.version and row.size == entry.size), None)

    def delete(self, cache_id):
        if type(cache_id) is not int or cache_id <= 0:
            raise ValueError("invalid cache ID")
        result = subprocess.run(
            ["gh", "api", "--hostname", "github.com", "--include", "--method", "DELETE",
             f"repos/{REPOSITORY}/actions/caches/{cache_id}"],
            env=self.api.env, capture_output=True, text=True, timeout=60)
        status = result.stdout.partition("\n")[0].strip().split()
        if len(status) >= 2 and re.fullmatch(r"HTTP/[0-9.]+", status[0]):
            if status[1] == "204" and result.returncode == 0:
                return True
            if status[1] == "404" and result.returncode == 1:
                return False
        raise RuntimeError(f"cache deletion failed for id={cache_id}")


def sweep(api, policy, apply=False, revision=None, *, recovery_only=False):
    if apply:
        api.require_live_policy(revision)
    before = api.entries()
    initial = sum(item.size for item in before)
    now = datetime.now(timezone.utc)
    today = now.date()
    proposed = plan(before, policy)
    readers = None if recovery_only else api.readers()
    legacy = not recovery_only and any(a.entry.ref == DEVELOP for a in proposed) and not api.legacy_readers_expired()
    planned_pr_states = {}
    held_days = {}
    holds = Counter()
    retained = Counter()
    for row in before:
        retained[retained_family(row.key)] += row.size
    planned = deleted = legacy_bytes = 0
    for action in proposed:
        item, successor = action.entry, action.successor
        day = item.created.astimezone(timezone.utc).date()
        kind = recovery_kind(action, today, policy)
        if recovery_only and not kind:
            holds["outside-proven-recovery-scope"] += item.size
            continue
        if legacy and item.ref == DEVELOP and not kind:
            legacy_bytes += item.size
            holds["historical-reader"] += item.size
            continue
        if PULL.fullmatch(item.ref) and item.ref not in planned_pr_states:
            planned_pr_states[item.ref] = api.closed_pull(item.ref)
        if (readers and readers.holds(item.ref)) or not planned_pr_states.get(item.ref, True):
            holds["reader-or-open-pr"] += item.size
            continue
        if apply or recovery_only:
            if ((successor and not api.visible(successor))
                    or (action.spare and not api.visible(action.spare))
                    or api.visible(item) != item):
                holds["candidate-or-successor-changed"] += item.size
                continue
            if recovery_only and kind == "prior-day-v4-seed":
                reason = held_days.get(day) or cargo_reader_hold(api, day, now)
                if reason:
                    held_days[day] = reason
                    holds[reason] += item.size
                    continue
            elif not recovery_only:
                readers = api.readers()
            if (readers and readers.holds(item.ref)) or (PULL.fullmatch(item.ref) and not api.closed_pull(item.ref)):
                holds["reader-or-open-pr"] += item.size
                continue
        event = {"id": item.id, "bytes": item.size, "reason": kind or action.reason,
                 "successor": successor.id if successor else None}
        if apply:
            api.require_live_policy(revision)
            print(json.dumps(dict(event, action="delete-intent")), flush=True)
            removed = api.delete(item.id)
            deleted += item.size if removed else 0
            event["action"] = "deleted" if removed else "already-absent"
        else:
            event["action"] = "would-delete"
        planned += item.size
        retained[retained_family(item.key)] -= item.size
        print(json.dumps(event), flush=True)
    projected = initial - planned
    after = None
    if apply:
        after_rows = api.entries()
        after = sum(item.size for item in after_rows)
        retained.clear()
        for row in after_rows:
            retained[retained_family(row.key)] += row.size
    remaining = after if apply else projected
    observed = after if apply else initial
    status = "preview"
    if apply:
        status = "under-target" if observed < BUDGET else "target-unmet"
        if initial >= BUDGET and observed < BUDGET:
            status = "recovered"
    if holds["reader-history-incomplete"]:
        status = "proof-incomplete"
    return {"mode": "apply" if apply else "dry-run", "status": status, "before_bytes": initial, "after_bytes": after,
            "observed_bytes": observed, "observed_bytes_to_admit": max(0, observed - BUDGET + 1),
            "projected_bytes": projected, "projected_bytes_to_admit": max(0, projected - BUDGET + 1),
            "remaining_bytes": remaining, "deleted_bytes": deleted, "planned_bytes": planned,
            "held_candidate_bytes": sum(holds.values()), "held_by_reason": dict(holds),
            "retained_by_family": dict(retained), "legacy_reader_hold_bytes": legacy_bytes, "budget_bytes": BUDGET,
            "over_budget_bytes": max(0, remaining - BUDGET), "bytes_to_admit": max(0, remaining - BUDGET + 1)}


def authorized_completion(env):
    if (env.get("GITHUB_REPOSITORY") != REPOSITORY or env.get("GITHUB_REF") != DEVELOP
            or env.get("GITHUB_EVENT_NAME") != "workflow_run" or env.get("GITHUB_WORKFLOW_REF") !=
            f"{REPOSITORY}/.github/workflows/cache-retention.yml@{DEVELOP}"):
        return False
    event = json.loads(Path(env["GITHUB_EVENT_PATH"]).read_text())
    run = event["workflow_run"]
    return (event["action"] == "completed" and run["status"] == "completed" and run["event"] == "push"
            and run["path"] in {".github/workflows/staging-cd.yml", ".github/workflows/ci-omarchy.yml",
                                ".github/workflows/migration-harness.yml"}
            and run["head_branch"] == "develop" and run["head_repository"]["full_name"] == REPOSITORY)


def main():
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--preview-recovery", action="store_true", help="preview the protected recovery sweep")
    mode.add_argument("--apply-recovery", action="store_true", help="retire protected-scope candidates and verify actual usage")
    args = parser.parse_args()
    env = os.environ
    try:
        if (args.apply_recovery or env.get("GITHUB_EVENT_NAME") == "workflow_run") and not authorized_completion(env):
            parser.error("This mode requires a trusted completed develop build")
        api = Actions(env["GH_TOKEN"], int(env["GITHUB_RUN_ID"]))
        result = sweep(api, Policies.from_source(Path(__file__).resolve().parents[2]), args.apply_recovery,
                       env.get("GITHUB_SHA"), recovery_only=args.apply_recovery or args.preview_recovery)
    except (OSError, KeyError, TypeError, ValueError, RuntimeError, subprocess.SubprocessError) as exc:
        result = {"status": "execution-error", "phase": "apply" if args.apply_recovery else "preview",
                  "error_class": type(exc).__name__}
    return publish_result(result, env)


if __name__ == "__main__":
    raise SystemExit(main())
