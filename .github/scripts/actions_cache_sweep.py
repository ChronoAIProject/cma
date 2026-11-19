"""Sweep rebuildable caches by immutable ID and account for the remaining bytes."""
import argparse
import json
import os
import re
import subprocess
from functools import lru_cache
from pathlib import Path
from urllib.parse import urlencode
from urllib.request import urlopen

from actions_cache_retention import BUDGET, DEVELOP, PULL, Entry, Policies, Readers, plan
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
            query = urlencode(dict(filters, page=page, per_page=100))
            return self.api.request(f"actions/{resource}?{query}")
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

    @lru_cache(maxsize=256)
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


def sweep(api, policy, apply=False):
    before = api.entries()
    initial = sum(item.size for item in before)
    readers = api.readers()
    planned = deleted = held = 0
    for action in plan(before, policy):
        item, successor = action.entry, action.successor
        if readers.holds(item.ref) or (PULL.fullmatch(item.ref) and not api.closed_pull(item.ref)):
            held += item.size
            continue
        if apply:
            # Inventory may change during a sweep (native eviction or another
            # build). Never delete by prefix or trust a disappeared replacement.
            if (successor and not api.visible(successor)) or api.visible(item) != item:
                held += item.size
                continue
            readers = api.readers()
            if readers.holds(item.ref):
                held += item.size
                continue
        event = {"id": item.id, "bytes": item.size, "reason": action.reason,
                 "successor": successor.id if successor else None}
        if apply:
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
            "held_candidate_bytes": held, "budget_bytes": BUDGET,
            "over_budget_bytes": max(0, remaining - BUDGET)}


def rollover(api, output):
    # The repository-wide usage endpoint is delayed. Count the same immutable
    # entries as retention immediately before handing an archive to the writer.
    used = sum(e.size for e in api.entries())
    allowed = used <= BUDGET
    output.write("allowed=" + ("true" if allowed else "false") + "\n")
    print(json.dumps({"before_rollover_bytes": used, "budget_bytes": BUDGET, "allowed": allowed}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check-rollover", action="store_true", help="check optional Rust save headroom")
    mode.add_argument("--apply", action="store_true", help="delete eligible entries")
    args = parser.parse_args()
    env = os.environ
    if (args.apply and (env.get("GITHUB_REPOSITORY") != REPOSITORY or env.get("GITHUB_REF") != DEVELOP
                        or env.get("GITHUB_EVENT_NAME") != "schedule"
                        or env.get("GITHUB_WORKFLOW_REF") !=
                        f"{REPOSITORY}/.github/workflows/cache-retention.yml@{DEVELOP}")):
        parser.error("apply requires the scheduled develop retention workflow")
    api = Actions(env["GH_TOKEN"], int(env["GITHUB_RUN_ID"]))
    if args.check_rollover:
        with Path(env["GITHUB_OUTPUT"]).open("a") as output:
            rollover(api, output)
        return 0
    result = sweep(api, Policies.from_source(Path(__file__).resolve().parents[2]), args.apply)
    print(json.dumps(result), flush=True)
    return int(bool(result["over_budget_bytes"]))


if __name__ == "__main__":
    raise SystemExit(main())
