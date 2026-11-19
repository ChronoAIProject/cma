"""Sweep rebuildable caches by immutable ID and account for the remaining bytes."""
import argparse
import json
import os
import re
import subprocess
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlencode
from urllib.request import urlopen

from actions_cache_retention import BUDGET, CARGO, CODEQL_BASE, DEVELOP, PULL, Entry, Policies, Readers, plan
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

    def active_runs(self, resource="runs", **filters):
        # A queued run can begin executing between calls. Check executing runs
        # last and include waiting/pending approvals that may have finished jobs.
        for state in ("requested", "queued", "pending", "waiting", "in_progress"):
            yield from self.list(resource, "workflow_runs", status=state, **filters)

    def readers(self):
        return Readers.from_runs(self.active_runs(), self.run_id)

    def sole_rust_writer(self, attempt, source):
        # A new run can always start. Every participating writer, including the
        # newcomer, must observe itself running alone before spending headroom.
        resource = "workflows/staging-cd.yml/runs"
        own = False
        for run in self.active_runs(resource, event="push", branch="develop"):
            if (run["id"] != self.run_id or run["run_attempt"] != attempt or run["head_sha"] != source
                    or run["event"] != "push" or run["head_branch"] != "develop"
                    or run["path"] != ".github/workflows/staging-cd.yml"):
                return False
            if run["status"] == "in_progress":
                own = True
        return own

    def eg_upload(self, attempt, source):
        jobs = self.list(f"runs/{self.run_id}/attempts/{attempt}/jobs", "jobs")
        matches = [j for j in jobs if j.get("name") == "Release eg / Publish"]
        if len(matches) != 1:
            return "unknown"
        job = matches[0]
        if (job.get("status") != "completed" or job.get("head_sha") != source
                or job.get("run_id") != self.run_id or job.get("run_attempt") != attempt):
            return "pending"
        saves = [s for s in job["steps"] if s.get("name") == "Retain accepted Cargo mounts"]
        if len(saves) != 1 or saves[0].get("status") != "completed":
            return "unknown"
        return saves[0].get("conclusion")

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


def sweep(api, policy, apply=False, revision=None):
    if apply:
        api.require_live_policy(revision)
    before = api.entries()
    initial = sum(item.size for item in before)
    readers = api.readers()
    proposed = plan(before, policy)
    planned_pr_states = {}
    today = datetime.now(timezone.utc).date()
    planned = deleted = held = reachable_bytes = 0
    for action in proposed:
        item, successor = action.entry, action.successor
        # Every v2/v3 publisher hashes the current UTC day into every restore
        # prefix. A newly launched historical re-run cannot select yesterday's
        # seed. The active reader census below still protects overnight runs.
        expired_family = CARGO.fullmatch(item.key) and item.created.astimezone(timezone.utc).date() < today
        # A new build can enter between the reader census and DELETE. A quiet
        # census or the age of the retention workflow cannot retire a reachable ID.
        if item.ref == DEVELOP and not (expired_family or CODEQL_BASE.fullmatch(item.key)):
            reachable_bytes += item.size
            held += item.size
            continue
        if PULL.fullmatch(item.ref) and item.ref not in planned_pr_states:
            planned_pr_states[item.ref] = api.closed_pull(item.ref)
        if readers.holds(item.ref) or not planned_pr_states.get(item.ref, True):
            held += item.size
            continue
        if apply:
            # Inventory may change during a sweep (native eviction or another
            # build). Never delete by prefix or trust a disappeared replacement.
            if (successor and not api.visible(successor)) or api.visible(item) != item:
                held += item.size
                continue
            if PULL.fullmatch(item.ref) and not api.closed_pull(item.ref):
                held += item.size
                continue
            api.require_live_policy(revision)
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
            "held_candidate_bytes": held, "legacy_reader_hold_bytes": reachable_bytes, "budget_bytes": BUDGET,
            "over_budget_bytes": max(0, remaining - BUDGET), "bytes_to_admit": max(0, remaining - BUDGET + 1)}


def rollover(api, output, *, after_eg=False, attempt=None, source=None):
    # Same-run EG precedence and cross-run exclusion apply to optional uploads.
    # A contended publisher proceeds immediately with its accepted image.
    peer = api.eg_upload(attempt, source) if after_eg else "skipped"
    # Check contenders before measuring usage: if an earlier writer finished,
    # the admission census must include its completed upload.
    uncontended = api.sole_rust_writer(attempt, source)
    entries = api.entries()
    used = sum(e.size for e in entries)
    visible = peer == "skipped"
    if peer == "success":
        suffix = f"-{api.run_id}-{attempt}"
        visible = any(e.ref == DEVELOP and e.size > 0 and e.key.endswith(suffix) and
                      (m := CARGO.fullmatch(e.key)) and (m[1] or m[3]) == "eg" for e in entries)
    allowed = uncontended and visible and used < BUDGET
    output.write("allowed=" + ("true" if allowed else "false") + "\n")
    print(json.dumps({"before_rollover_bytes": used, "budget_bytes": BUDGET, "allowed": allowed,
                      "sole_rust_writer": uncontended, "eg_save_observation": peer if after_eg else "independent"}))


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
    mode.add_argument("--check-rollover", action="store_true", help="check optional Rust save headroom")
    mode.add_argument("--apply", action="store_true", help="delete eligible entries")
    args = parser.parse_args()
    env = os.environ
    if args.apply and not authorized_completion(env):
        parser.error("apply requires a completed develop build in the protected retention workflow")
    api = Actions(env["GH_TOKEN"], int(env["GITHUB_RUN_ID"]))
    if args.check_rollover:
        component, sibling = env["CD_COMPONENT"], env["EG_CACHE_WRITER"]
        if component not in ("am", "eg") or sibling not in ("true", "false") or (sibling == "true" and component != "am"):
            parser.error("invalid Rust cache writer assignment")
        with Path(env["GITHUB_OUTPUT"]).open("a") as output:
            rollover(api, output, after_eg=sibling == "true", attempt=int(env["GITHUB_RUN_ATTEMPT"]),
                     source=env["GITHUB_SHA"])
        return 0
    result = sweep(api, Policies.from_source(Path(__file__).resolve().parents[2]), args.apply, env.get("GITHUB_SHA"))
    print(json.dumps(result), flush=True)
    return int(bool(result["bytes_to_admit"]))


if __name__ == "__main__":
    raise SystemExit(main())
