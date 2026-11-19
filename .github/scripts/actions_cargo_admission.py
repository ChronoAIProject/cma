"""Coordinate optional Cargo saves outside the release dependency graph."""

import argparse
import json
import os
from datetime import datetime, timezone
from pathlib import Path

from actions_cache_retention import BUDGET, DEVELOP
from actions_cache_sweep import Actions
from actions_cargo_budget import BudgetRef, grant, save_decision
from actions_cargo_migration import WriterHistory, has_upload, legacy_charge
from staging_cd_build_cache import lookup_key
from staging_cd_cache_context import context
from staging_cd_common import matches, require
from staging_cd_versions import REPOSITORY


def candidates(env):
    releases = json.loads(env["CARGO_PUBLICATIONS"])
    require(isinstance(releases, dict), "Invalid publication matrix")
    keys = []
    for component in ("eg", "am"):
        if component not in releases:
            continue
        tasks = json.loads(releases[component])
        publish = [task for task in tasks if task["mode"] == "publish"]
        if not publish:
            continue
        # The last successful export sets the workflow save key. A different
        # graph or a UTC rollover skips the optional upload for this grant.
        task = publish[-1]
        require(
            task["reservation"]["run_id"] == int(env["GITHUB_RUN_ID"])
            and task["reservation"]["run_attempt"] == int(env["GITHUB_RUN_ATTEMPT"]),
            "Cargo reservation owner differs",
        )
        key = lookup_key(Path.cwd(), component, dict(env, CD_TASKS=json.dumps([task])))
        keys.append(key.archive_key(env))
    return keys


def known_skipped(api, writer):
    run_id, attempt, service = writer
    # The exact attempt stays immutable across retries. In particular,
    # cancellation or an API error never releases a possibly accepted upload.
    run = api.api.request(f"actions/runs/{run_id}/attempts/{attempt}")
    require(
        run["id"] == run_id
        and run["run_attempt"] == attempt
        and run["event"] == "push"
        and run["head_branch"] == "develop"
        and run["head_repository"]["full_name"] == REPOSITORY
        and run["path"] == ".github/workflows/staging-cd.yml",
        "Unexpected grant owner attempt",
    )
    if run["status"] != "completed":
        return False
    jobs = api.list(f"runs/{run_id}/attempts/{attempt}/jobs", "jobs")
    require(jobs, "Missing grant owner jobs")
    return not has_upload(jobs, run, attempt, service)


def reserve(api, ledger, keys, source, history, now):
    previous, account = ledger.read()
    before = json.dumps(account.encode(), sort_keys=True)
    entries = api.entries()
    legacy = legacy_charge(api, history, account.migration, entries, now)
    account.migration["debt"] = legacy
    if legacy is None:
        result = {
            "grants": 0,
            "reason": "historical-writer-can-save",
            "inventory_bytes": sum(e.size for e in entries),
            "outstanding_bytes": sum(account.charges.values()),
            "budget_bytes": BUDGET,
        }
    else:
        result = grant(account, entries, keys, lambda writer: known_skipped(api, writer), legacy)
    if before != json.dumps(account.encode(), sort_keys=True):
        ledger.advance(previous, account, source)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("reserve", "check"))
    args = parser.parse_args()
    env = os.environ
    require(
        env.get("GITHUB_REPOSITORY") == REPOSITORY
        and env.get("GITHUB_REF") == DEVELOP
        and env.get("GITHUB_EVENT_NAME") == "push"
        and matches("[a-f0-9]{40}", env.get("GITHUB_SHA"))
        and matches("[1-9][0-9]*", env.get("GITHUB_RUN_ID"))
        and matches("[1-9][0-9]*", env.get("GITHUB_RUN_ATTEMPT"))
        and env.get("GITHUB_WORKFLOW_REF") == f"{REPOSITORY}/.github/workflows/staging-cd.yml@{DEVELOP}",
        "Cargo budget requires the develop release workflow",
    )
    api = Actions(env["GH_TOKEN"], int(env["GITHUB_RUN_ID"]))
    ledger = BudgetRef(api.api)
    if args.mode == "reserve":
        result = reserve(
            api,
            ledger,
            candidates(env),
            env["GITHUB_SHA"],
            WriterHistory(env["GITHUB_SHA"]),
            datetime.now(timezone.utc),
        )
    else:
        directory = context(env) / "accepted"
        result = save_decision(api, ledger, env, directory, env["CARGO_SAVE_KEY"])
        with Path(env["GITHUB_OUTPUT"]).open("a") as output:
            output.write("allowed=" + ("true" if result["allowed"] else "false") + "\n")
    print(json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
