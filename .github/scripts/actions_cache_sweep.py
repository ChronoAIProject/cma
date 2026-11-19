"""Sweep rebuildable caches by immutable ID and account for the remaining bytes."""
import argparse
import json
import os
import re
import subprocess
from datetime import datetime, timedelta, timezone
from pathlib import Path
from urllib.parse import urlencode
from urllib.request import urlopen

from actions_cache_retention import BUDGET, CARGO, CODEQL_BASE, DEVELOP, PULL, Entry, Policies, Readers, plan
from actions_cache_run_history import IncompleteHistory, cargo_readers_finished
from staging_cd_github import GitHub
from staging_cd_versions import REPOSITORY


def pages(fetch, field):
    rows = []
    total = None
    page = 1
    while True:
        doc = fetch(page)
        count, values = doc["total_count"], doc[field]
        if (type(count) is not int or count < 0 or not isinstance(values, list)
                or total is not None and total != count):
            raise ValueError("inventory changed or is incomplete")
        rows.extend(values)
        total = count
        if len(rows) == total:
            break
        if len(rows) > total or len(values) != 100:
            raise ValueError("incomplete inventory page")
        page += 1
    if len({row["id"] for row in rows}) != total:
        raise ValueError("duplicate inventory IDs")
    return rows


class Actions:
    def __init__(self, token, run_id):
        self.api = GitHub(token)
        self.run_id = run_id

    def list(self, resource, field, **filters):
        def fetch(page):
            query = urlencode(dict(filters, page=page, per_page=100)).replace("..", "%2E%2E")
            doc = self.api.request(f"actions/{resource}?{query}")
            if field == "workflow_runs" and filters and doc["total_count"] >= 1000:
                raise IncompleteHistory("filtered workflow history reached the result limit")
            return doc
        return pages(fetch, field)

    def entries(self, **filters):
        return [Entry.parse(row) for row in self.list("caches", "actions_caches", **filters)]

    def readers(self):
        # A queued run can begin executing between calls. Check executing runs
        # last and include waiting/pending approvals that may have finished jobs.
        runs = []
        for state in ("requested", "queued", "pending", "waiting", "in_progress"):
            runs.extend(self.list("runs", "workflow_runs", status=state))
        return Readers.from_runs(runs, self.run_id)

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


def sweep(api, policy, apply=False, revision=None, *, expired_cargo_only=False):
    if apply:
        api.require_live_policy(revision)
    before = api.entries()
    initial = sum(item.size for item in before)
    now = datetime.now(timezone.utc)
    today = now.date()
    proposed = plan(before, policy)
    if expired_cargo_only:
        proposed = [a for a in proposed if a.entry.ref == DEVELOP and CARGO.fullmatch(a.entry.key)
                    and a.entry.created.astimezone(timezone.utc).date() < today and a.successor is not None]
    readers = None if expired_cargo_only else api.readers()
    legacy = not expired_cargo_only and any(a.entry.ref == DEVELOP for a in proposed) and not api.legacy_readers_expired()
    planned_pr_states = {}
    held_days = set()
    planned = deleted = held = legacy_bytes = 0
    for action in proposed:
        item, successor = action.entry, action.successor
        # Every v4 publisher hashes the current UTC day into every restore
        # prefix. A newly launched historical re-run cannot select yesterday's
        # seed. The active reader census below still protects overnight runs.
        expired_family = CARGO.fullmatch(item.key) and item.created.astimezone(timezone.utc).date() < today
        if legacy and item.ref == DEVELOP and not (expired_family or CODEQL_BASE.fullmatch(item.key)):
            legacy_bytes += item.size
            held += item.size
            continue
        if PULL.fullmatch(item.ref) and item.ref not in planned_pr_states:
            planned_pr_states[item.ref] = api.closed_pull(item.ref)
        if (readers and readers.holds(item.ref)) or not planned_pr_states.get(item.ref, True):
            held += item.size
            continue
        if apply or expired_cargo_only:
            # Inventory may change during a sweep (native eviction or another
            # build). Never delete by prefix or trust a disappeared replacement.
            if (successor and not api.visible(successor)) or api.visible(item) != item:
                held += item.size
                continue
            if expired_cargo_only:
                day = item.created.astimezone(timezone.utc).date()
                if day in held_days or not cargo_readers_finished(api, day, now):
                    held_days.add(day)
                    held += item.size
                    continue
            else:
                readers = api.readers()
            if (readers and readers.holds(item.ref)) or (PULL.fullmatch(item.ref) and not api.closed_pull(item.ref)):
                held += item.size
                continue
        event = {"id": item.id, "bytes": item.size, "reason": action.reason,
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
        print(json.dumps(event), flush=True)
    remaining = sum(item.size for item in api.entries()) if apply else initial - planned
    return {"mode": "apply" if apply else "dry-run", "before_bytes": initial,
            "remaining_bytes": remaining, "deleted_bytes": deleted, "planned_bytes": planned,
            "held_candidate_bytes": held, "legacy_reader_hold_bytes": legacy_bytes, "budget_bytes": BUDGET,
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
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--preview-expired-cargo", action="store_true", help="preview the protected Cargo sweep")
    mode.add_argument("--prune-expired-cargo", action="store_true", help="retire only unreachable develop Cargo seeds")
    args = parser.parse_args()
    env = os.environ
    if args.prune_expired_cargo and not authorized_completion(env):
        parser.error("Cargo pruning requires a trusted completed develop build")
    api = Actions(env["GH_TOKEN"], int(env["GITHUB_RUN_ID"]))
    if env.get("GITHUB_EVENT_NAME") == "workflow_run" and not authorized_completion(env):
        parser.error("report requires a completed develop build in the protected retention workflow")
    result = sweep(api, Policies.from_source(Path(__file__).resolve().parents[2]), args.prune_expired_cargo,
                   env.get("GITHUB_SHA"), expired_cargo_only=args.prune_expired_cargo or args.preview_expired_cargo)
    print(json.dumps(result), flush=True)
    return int(bool(result["bytes_to_admit"]))


if __name__ == "__main__":
    raise SystemExit(main())
